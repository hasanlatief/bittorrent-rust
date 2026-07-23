#![feature(string_from_utf8_lossy_owned)]
use std::{env, net::SocketAddrV4, path::Path, str::FromStr};

use nom::{
    IResult, Parser as _,
    bytes::streaming::{tag, take},
    character::{char, complete::digit1},
    combinator::{opt, recognize},
    sequence::preceded,
};
use sha1::{Digest, Sha1};

use percent_encoding::{NON_ALPHANUMERIC, percent_encode};

use anyhow::{Context as _, anyhow};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

mod bencode;

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
const INTERVAL: &[u8] = b"interval";
const PEERS: &[u8] = b"peers";

struct TorrentInfo {
    announce: String,
    length: usize,
    name: Option<String>,
    piece_length: usize,
    piece_hashes: Vec<u8>,
    info_hash: [u8; 20],
}

impl TorrentInfo {
    fn new(val: bencode::Value) -> anyhow::Result<Self> {
        let mut map = val.try_into_dict().context("Value was not a dictionary")?;
        let announce = map
            .remove(ANNOUNCE)
            .and_then(|v| v.try_into_string())
            .context("Missing announce")?
            .try_into()?;
        let mut info_hash = [0; 20];
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
        let piece_hashes = info
            .remove(PIECES)
            .and_then(|v| v.try_into_string())
            .context("Missing pieces")?;
        let piece_length = info
            .remove(PIECE_LENGTH)
            .and_then(|v| v.try_into_integer())
            .context("Missing piece length")?
            .try_into()?;
        Ok(TorrentInfo {
            announce,
            length,
            name,
            piece_length,
            piece_hashes,
            info_hash,
        })
    }
}

fn print_torrent_summary(val: TorrentInfo) -> anyhow::Result<()> {
    let TorrentInfo {
        announce,
        length,
        name: _,
        piece_length,
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
    let (chunks, rem) = piece_hashes.as_chunks::<20>();
    if !rem.is_empty() {
        return Err(anyhow!("Piece hashes length was not a multiple of 20"));
    }
    for hash in chunks {
        for byte in hash {
            print!("{byte:02x}");
        }
        println!();
    }
    Ok(())
}

struct TrackerResponse {
    peers: Vec<SocketAddrV4>,
}

const PEER_ID: [u8; 20] = *b"idkICanPickAnything!";
const EXTENSIONS: [u8; 8] = [0; 8];

async fn tracker_get_request(torrent: &TorrentInfo) -> anyhow::Result<TrackerResponse> {
    let client = reqwest::Client::new();

    let encoded_hash = percent_encode(&torrent.info_hash, NON_ALPHANUMERIC).to_string();

    let query_str = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("peer_id", str::from_utf8(&PEER_ID).unwrap())
        .append_pair("port", "6881")
        .append_pair("uploaded", "0")
        .append_pair("downloaded", "0")
        .append_pair("left", &torrent.length.to_string())
        .append_pair("compact", "1")
        .finish();

    let url = format!(
        "{}?info_hash={}&{}",
        torrent.announce, encoded_hash, query_str
    );

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
                let ip = bytes[0..4].as_array().copied().unwrap().into();
                let port = u16::from_be_bytes([bytes[4], bytes[5]]);
                SocketAddrV4::new(ip, port)
            })
            .collect(),
    })
}

fn read_torrent_file(path: impl AsRef<Path>) -> anyhow::Result<TorrentInfo> {
    let torrent_bytes = std::fs::read(path.as_ref())?;
    TorrentInfo::new(bencode::parse(&torrent_bytes)?)
}

struct PeerInfo {
    id: [u8; 20],
    info_hash: [u8; 20],
    protocol: String,
    extension: [u8; 8],
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
}

fn parse_handshake(bytes: &[u8]) -> IResult<&[u8], PeerInfo> {
    let (rest, protcol_str_len) = take(1_usize).map(|s: &[u8]| s[0]).parse(bytes)?;
    let (rest, protocol) = take(protcol_str_len)
        .map_res(|protcol_bytes: &[u8]| String::from_utf8(protcol_bytes.to_vec()))
        .parse(rest)?;
    let (rest, (extension, info_hash, peer_id)) =
        (take(8_usize), take(20_usize), take(20_usize)).parse(rest)?;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let command = args[1].as_str();

    match command {
        "decode" => {
            let encoded_value = args[2].as_bytes();
            let decoded_value: serde_json::Value = bencode::parse(encoded_value)?.into();
            println!("{}", decoded_value);
        }
        "info" => {
            let torrent = read_torrent_file(&args[2])?;
            print_torrent_summary(torrent)?;
        }
        "peers" => {
            let torrent = read_torrent_file(&args[2])?;
            let response = tracker_get_request(&torrent).await?;
            for peer in response.peers {
                let ip = peer.ip();
                let port = peer.port();
                println!("{ip}:{port}")
            }
        }
        "handshake" => {
            let torrent = read_torrent_file(&args[2])?;
            let (peer_read, peer_write) = tokio::net::TcpSocket::new_v4()?
                .connect(args[3].parse()?)
                .await?
                .into_split();
            let mut peer_read = BufReader::new(peer_read);
            let mut peer_write = BufWriter::new(peer_write);
            let self_info = PeerInfo {
                id: PEER_ID,
                info_hash: torrent.info_hash,
                protocol: String::from("BitTorrent protocol"),
                extension: EXTENSIONS,
            };
            peer_write.write_all(&self_info.as_handshake()).await?;
            peer_write.flush().await?;
            let mut buf = Vec::new();
            let peer = loop {
                let n = peer_read.read_buf(&mut buf).await?;
                if n == 0 {
                    return Err(anyhow!(
                        "Peer prematurely closed connection during handshake"
                    ));
                }
                match parse_handshake(&buf) {
                    Ok((_, v)) => break v,
                    Err(e) => match e {
                        nom::Err::Incomplete(_) => {
                            continue;
                        }
                        nom::Err::Error(e) | nom::Err::Failure(e) => {
                            return Err(anyhow!("Failed to parse handshake from peer {:?}", e));
                        }
                    },
                }
            };
            print!("Peer ID: ");
            for byte in peer.id {
                print!("{byte:02x}");
            }
            println!()
        }
        _ => println!("unknown command: {}", args[1]),
    }
    Ok(())
}
