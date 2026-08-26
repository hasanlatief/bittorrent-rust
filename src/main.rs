#![feature(string_from_utf8_lossy_owned)]

use std::{
    io::Write,
    net::{SocketAddr, SocketAddrV4},
    path::Path,
    str::FromStr,
};

use nom::{
    IResult, Parser as _,
    bytes::streaming::take,
    character::{char, complete::digit1},
    combinator::{opt, recognize},
};
use sha1::{Digest, Sha1};

use percent_encoding::{NON_ALPHANUMERIC, percent_encode};

use anyhow::Context as _;

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
};

mod bencode;

mod message;

use message::{BitfieldMsg, Message, RequestMsg};

mod cli;
use cli::Command;

use crate::message::PieceMsg;

fn parse_number<I: FromStr>(s: &[u8]) -> IResult<&[u8], I> {
    recognize((opt(char('-')), digit1))
        // SAFETY: The parser only maps if it matches [-0-9] which are all ASCII and valid UTF8
        .map(|bytes| unsafe { str::from_utf8_unchecked(bytes) })
        .map_res(str::parse::<I>)
        .parse(s)
}

const INFO: &[u8] = b"info";
const LENGTH: &[u8] = b"length";
const ANNOUNCE: &[u8] = b"announce";
const PIECE_LENGTH: &[u8] = b"piece length";
const PIECES: &[u8] = b"pieces";
const NAME: &[u8] = b"name";
// const INTERVAL: &[u8] = b"interval";
const PEERS: &[u8] = b"peers";

const MAX_BLOCK_SIZE: u32 = 1024 * 16;
struct TorrentInfo {
    announce: String,
    name: Option<String>,
    length: u32,
    piece_len: u32,
    piece_hashes: Vec<[u8; HASH_LEN]>,
    info_hash: [u8; HASH_LEN],
}

impl TorrentInfo {
    fn new(val: bencode::Value) -> anyhow::Result<Self> {
        let mut map = val.try_into_dict().context("Value was not a dictionary")?;
        let announce = map
            .remove(ANNOUNCE)
            .and_then(|v| v.try_into_string())
            .context("Missing announce")?
            .try_into()?;
        let mut info_hash = [0; HASH_LEN];
        let mut info = map
            .remove(INFO)
            .inspect(|v| {
                info_hash = Sha1::digest(v.seralize()).into();
            })
            .and_then(|v| v.try_into_dict())
            .context("Missing info")?;
        let length = info
            .remove(LENGTH)
            .and_then(|v| v.try_into_integer())
            .context("Missing length")?
            .try_into()?;
        let name = info
            .remove(NAME)
            .and_then(|v| v.try_into_string())
            .and_then(|s| String::from_utf8(s).ok());
        let mut piece_hashes_bytes = info
            .remove(PIECES)
            .and_then(|v| v.try_into_string())
            .context("Missing pieces")?;
        anyhow::ensure!(
            piece_hashes_bytes.len() % HASH_LEN == 0,
            "Torrent piece hashes were incomplete."
        );
        let piece_hashes = unsafe {
            Vec::from_raw_parts(
                piece_hashes_bytes.as_mut_ptr().cast(),
                piece_hashes_bytes.len() / HASH_LEN,
                piece_hashes_bytes.capacity() / HASH_LEN,
            )
        };
        std::mem::forget(piece_hashes_bytes);
        let piece_len = info
            .remove(PIECE_LENGTH)
            .and_then(|v| v.try_into_integer())
            .context("Missing piece length")?
            .try_into()?;
        Ok(TorrentInfo {
            announce,
            length,
            name,
            piece_len,
            piece_hashes,
            info_hash,
        })
    }

    fn self_peer_info(&self) -> PeerInfo {
        PeerInfo {
            id: PEER_ID,
            info_hash: self.info_hash,
            protocol: String::from(PROTOCOL),
            extension: EXTENSIONS,
        }
    }
    fn piece_len(&self, piece_index: u32) -> u32 {
        let num_pieces = self.piece_hashes.len() as u32;
        if piece_index == num_pieces - 1 {
            let full_pieces_len = self.piece_len * (num_pieces - 1);
            self.length - full_pieces_len
        } else {
            self.piece_len
        }
    }

    async fn handshake_with(&self, ip: impl Into<SocketAddr>) -> anyhow::Result<Peer> {
        let (peer_read, peer_write) = tokio::net::TcpSocket::new_v4()?
            .connect(ip.into())
            .await?
            .into_split();
        let self_info = self.self_peer_info();
        let mut read = BufReader::new(peer_read);
        let mut write = BufWriter::new(peer_write);
        write.write_all(&self_info.as_handshake()).await?;
        write.flush().await?;
        let info = PeerInfo::parse_from_connection(&mut read).await?;
        Ok(Peer { info, read, write })
    }

    async fn tracker_peers(&self) -> anyhow::Result<TrackerResponse> {
        let encoded_hash = percent_encode(&self.info_hash, NON_ALPHANUMERIC).to_string();

        let query_str = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("peer_id", str::from_utf8(&PEER_ID).unwrap())
            .append_pair("port", "6881")
            .append_pair("uploaded", "0")
            .append_pair("downloaded", "0")
            .append_pair("left", &self.length.to_string())
            .append_pair("compact", "1")
            .finish();
        let announce = &self.announce;
        let url = format!("{announce}?info_hash={encoded_hash}&{query_str}");

        let client = reqwest::Client::new();
        let response = client.get(url).send().await?.bytes().await?;
        let map = bencode::parse(&response)?
            .try_into_dict()
            .context("Missing peers in response")?;
        let peers = map
            .get(PEERS)
            .and_then(|v| v.as_string())
            .context("Missing peers in response")?;
        Ok(TrackerResponse {
            peers: peers
                .as_chunks::<6>()
                .0
                .iter()
                .map(|bytes| {
                    let ip = [bytes[0], bytes[1], bytes[2], bytes[3]].into();
                    let port = u16::from_be_bytes([bytes[4], bytes[5]]);
                    SocketAddrV4::new(ip, port)
                })
                .collect(),
        })
    }
}

fn print_torrent_summary(val: TorrentInfo) {
    let TorrentInfo {
        announce,
        length,
        name: _,
        piece_len: piece_length,
        piece_hashes,
        info_hash,
    } = val;
    println!("Tracker URL: {announce}");
    println!("Length: {length}");
    print!("Info Hash: ");
    for byte in info_hash {
        print!("{byte:02x}")
    }
    println!();
    println!("Piece Length: {piece_length}");
    for hash in piece_hashes {
        for byte in hash {
            print!("{byte:02x}");
        }
        println!();
    }
}

fn read_torrent_file(path: impl AsRef<Path>) -> anyhow::Result<TorrentInfo> {
    let torrent_bytes = std::fs::read(path.as_ref())?;
    TorrentInfo::new(bencode::parse(&torrent_bytes)?)
}

struct TrackerResponse {
    peers: Vec<SocketAddrV4>,
}

const PEER_ID_LEN: usize = 20;
const PEER_ID: [u8; PEER_ID_LEN] = *b"idkICanPickAnything!";

const EXTENSION_LEN: usize = 8;
const EXTENSIONS: [u8; EXTENSION_LEN] = [0; EXTENSION_LEN];
const PROTOCOL: &str = "BitTorrent protocol";

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
            if buf.is_empty() {
                anyhow::bail!("Peer prematurely closed connection during handshake");
            }
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command = cli::parse();
    unsafe {
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
            print_torrent_summary(torrent);
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
            let peer = torrent.handshake_with(args.peer).await?;
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
            let mut peer = torrent.handshake_with(peer_ip).await?;
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
            peer.write.flush().await?;
            println!("Waiting for unchoke message.");
            match peer.recv().await {
                Ok(msg) => match dbg!(&msg) {
                    Message::Unchoke => (),
                    m => anyhow::bail!("Received an unexpected message: {m:?}"),
                },
                Err(e) => anyhow::bail!("Failed to receive unchoke message: {e}"),
            }
            println!("Requesting pieces.");
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
            peer.write.flush().await?;
            println!("Receiving piece.");
            let mut data = vec![0; piece_len as usize];
            let mut blocks_received = 0;
            let block_count = piece_len.div_ceil(MAX_BLOCK_SIZE);
            while blocks_received < block_count {
                match peer
                    .recv()
                    .await
                    .context("Something went wrong while waiting for a piece")?
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
