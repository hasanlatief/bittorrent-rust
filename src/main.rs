#![feature(string_from_utf8_lossy_owned)]

use std::{io::Write, net::SocketAddr};

use nom::{IResult, Parser as _, bytes::streaming::take};

use anyhow::Context as _;

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
};

mod bencode;

mod message;

use message::{BitfieldMsg, Message, PieceMsg, RequestMsg};

mod cli;
use cli::Command;

mod torrent_file;

use torrent_file::{TorrentInfo, read_torrent_file};

const MAX_BLOCK_SIZE: u32 = 1024 * 16;
const PEER_ID_LEN: usize = 20;
const EXTENSION_LEN: usize = 8;
const EXTENSIONS: [u8; EXTENSION_LEN] = [0; EXTENSION_LEN];

const HASH_LEN: usize = 20;
struct PeerInfo {
    id: [u8; PEER_ID_LEN],
    info_hash: [u8; HASH_LEN],
    protocol: String,
    extension: [u8; EXTENSION_LEN],
}

impl PeerInfo {
    fn as_handshake(&self) -> Vec<u8> {
        let mut result = Vec::new();
        result.push(self.protocol.len().try_into().unwrap());
        result.extend_from_slice(self.protocol.as_bytes());
        result.extend_from_slice(&self.extension);
        result.extend_from_slice(&self.info_hash);
        result.extend_from_slice(&self.id);
        result
    }

    fn parse(bytes: &[u8]) -> IResult<&[u8], PeerInfo> {
        let (rest, protcol_str_len) = take(1_usize).map(|s: &[u8]| s[0]).parse(bytes)?;
        let (rest, protocol) = take(protcol_str_len)
            .map_res(|protcol_bytes: &[u8]| String::from_utf8(protcol_bytes.to_vec()))
            .parse(rest)?;
        let (rest, (extension, info_hash, peer_id)) =
            (take(EXTENSION_LEN), take(HASH_LEN), take(PEER_ID_LEN)).parse(rest)?;
        Ok((
            rest,
            PeerInfo {
                id: *peer_id.as_array().unwrap(),
                info_hash: *info_hash.as_array().unwrap(),
                protocol,
                extension: *extension.as_array().unwrap(),
            },
        ))
    }

    async fn parse_from_connection<R: tokio::io::AsyncBufRead + Unpin>(
        reader: &mut R,
    ) -> anyhow::Result<Self> {
        loop {
            let buf = reader.fill_buf().await?;
            anyhow::ensure!(
                !buf.is_empty(),
                "Peer prematurely closed connection during handshake"
            );
            match Self::parse(buf) {
                Ok((rest, v)) => {
                    let consumed = buf.len() - rest.len();
                    reader.consume(consumed);
                    return Ok(v);
                }
                Err(e) => match e {
                    nom::Err::Incomplete(_) => {
                        continue;
                    }
                    nom::Err::Error(e) | nom::Err::Failure(e) => {
                        anyhow::bail!("Failed to parse handshake from peer {e:?}");
                    }
                },
            }
        }
    }
}

struct Peer {
    info: PeerInfo,
    read: BufReader<OwnedReadHalf>,
    write: BufWriter<OwnedWriteHalf>,
}

impl Peer {
    async fn recv(&mut self) -> anyhow::Result<Message> {
        self.write.flush().await?;
        let len = self.read.read_u32().await? - 1; // -1 for the kind byte
        let kind_num = self.read.read_u8().await?;
        let mut reader = (&mut self.read).take(len as u64);
        Ok(match kind_num {
            0 => Message::Choke,
            1 => Message::Unchoke,
            2 => Message::Interested,
            3 => Message::NotInterested,
            4 => todo!(),
            5 => Message::Bitfield(
                BitfieldMsg::parse(&mut reader)
                    .await
                    .context("Failed to parse bitfield message")?,
            ),
            6 => Message::Request(
                RequestMsg::parse(&mut reader)
                    .await
                    .context("Failed to parse request message")?,
            ),
            7 => Message::Piece(
                PieceMsg::parse(&mut reader)
                    .await
                    .context("Failed to parse piece message")?,
            ),
            8 => todo!(),
            id => anyhow::bail!("The message id byte ({id}) was outside the supported range."),
        })
    }

    async fn send(&mut self, msg: Message) -> anyhow::Result<()> {
        let len: u32 = msg.len();
        self.write.write_u32(len).await?;
        self.write.write_u8(msg.id()).await?;
        self.write.write_all(&msg.payload()).await?;
        Ok(())
    }

    async fn handshake_with(
        ip: impl Into<SocketAddr>,
        torrent: &TorrentInfo,
    ) -> anyhow::Result<Peer> {
        let (peer_read, peer_write) = tokio::net::TcpSocket::new_v4()?
            .connect(ip.into())
            .await?
            .into_split();
        let self_info = torrent.self_peer_info();
        let mut read = BufReader::new(peer_read);
        let mut write = BufWriter::new(peer_write);
        write.write_all(&self_info.as_handshake()).await?;
        write.flush().await?;
        let info = PeerInfo::parse_from_connection(&mut read).await?;
        Ok(Peer { info, read, write })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command = cli::parse();
    unsafe {
        // makes the codecrafter tester show stderr because it stupidly swallows it by default
        libc::dup2(libc::STDOUT_FILENO, libc::STDERR_FILENO);
    }
    match command {
        Command::Decode(args) => {
            let encoded_value = args.bencoded.as_bytes();
            let decoded_value: serde_json::Value = bencode::parse(encoded_value)?.into();
            println!("{}", decoded_value);
        }
        Command::Info(args) => {
            let torrent = read_torrent_file(args.torrent_path)?;
            torrent_file::print_torrent_summary(torrent);
        }
        Command::Peers(args) => {
            let torrent = read_torrent_file(args.torrent_path)?;
            let response = torrent.tracker_peers().await?;
            for peer in response.peers {
                let ip = peer.ip();
                let port = peer.port();
                println!("{ip}:{port}")
            }
        }
        Command::Handshake(args) => {
            let torrent = read_torrent_file(args.torrent_path)?;
            let peer = Peer::handshake_with(args.peer, &torrent).await?;
            print!("Peer ID: ");
            for byte in peer.info.id {
                print!("{byte:02x}");
            }
            println!()
        }
        Command::DownloadPiece(args) => {
            let torrent = read_torrent_file(args.torrent_path)?;
            let mut output_file = std::fs::File::create(args.output_file_path)?;
            let piece_len = torrent.piece_len(args.piece_index);
            let response = torrent.tracker_peers().await?;
            let peer_ip = response.peers[0]; // use the first peer
            let mut peer = Peer::handshake_with(peer_ip, &torrent).await?;
            println!("Waiting for bitfield.");
            match peer.recv().await {
                Ok(msg) => match msg {
                    Message::Bitfield(_) => (), // Do nothing for now since we are guaranteed all peers have all pieces
                    m => anyhow::bail!("Received an unexpected message: {m:?}"),
                },
                Err(e) => anyhow::bail!("Failed to receive bitfield message: {e}."),
            }
            println!("Sending interested message.");
            peer.send(Message::Interested).await?;
            println!("Waiting for unchoke message.");
            match peer.recv().await {
                Ok(msg) => match msg {
                    Message::Unchoke => (),
                    m => anyhow::bail!("Received an unexpected message: {m:?}"),
                },
                Err(e) => anyhow::bail!("Failed to receive unchoke message: {e}"),
            }
            println!("Requesting piece.");
            let mut byte_idx = 0;
            let remainder_block_size = piece_len % MAX_BLOCK_SIZE;
            while byte_idx < piece_len - remainder_block_size {
                peer.send(Message::Request(RequestMsg::new(
                    args.piece_index,
                    byte_idx,
                    MAX_BLOCK_SIZE,
                )))
                .await?;
                byte_idx += MAX_BLOCK_SIZE;
            }
            if remainder_block_size > 0 {
                peer.send(Message::Request(RequestMsg::new(
                    args.piece_index,
                    piece_len - remainder_block_size,
                    remainder_block_size,
                )))
                .await?;
            }
            println!("Receiving piece.");
            let mut data = vec![0; piece_len as usize];
            let mut blocks_received = 0;
            let block_count = piece_len.div_ceil(MAX_BLOCK_SIZE);
            while blocks_received < block_count {
                match peer
                    .recv()
                    .await
                    .context("Something went wrong while downloading a piece")?
                {
                    Message::Piece(piece) => {
                        let block = piece.block();
                        data[(piece.byte_idx() as usize)..][..block.len()].copy_from_slice(block);
                        blocks_received += 1;
                    }
                    msg => {
                        eprintln!("Unexpected message while downloading piece: {msg:?}");
                        continue;
                    }
                }
            }
            output_file.write_all(&data)?;
        }
    }
    Ok(())
}
