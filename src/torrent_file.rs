use crate::bencode;
use crate::peers::PEER_ID_LEN;
use std::{net::SocketAddrV4, path::Path};

use anyhow::Context as _;
use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
use sha1::{Digest as _, Sha1};

use super::{peers::EXTENSIONS, peers::HASH_LEN, peers::PeerInfo};

const INFO: &[u8] = b"info";
const LENGTH: &[u8] = b"length";
const ANNOUNCE: &[u8] = b"announce";
const PIECE_LENGTH: &[u8] = b"piece length";
const PIECES: &[u8] = b"pieces";
const NAME: &[u8] = b"name";

// const INTERVAL: &[u8] = b"interval";
const PEERS: &[u8] = b"peers";

const SELF_PEER_ID: [u8; PEER_ID_LEN] = *b"idkICanPickAnything!";
const PROTOCOL: &str = "BitTorrent protocol";

pub struct TrackerResponse {
    pub peers: Vec<SocketAddrV4>,
}
pub(crate) struct TorrentInfo {
    announce: String,
    name: Option<String>,
    length: u32,
    piece_len: u32,
    piece_hashes: Vec<[u8; HASH_LEN]>,
    info_hash: [u8; HASH_LEN],
}

impl TorrentInfo {
    pub(crate) fn new(val: bencode::Value) -> anyhow::Result<Self> {
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
        let piece_hashes_bytes = info
            .remove(PIECES)
            .and_then(|v| v.try_into_string())
            .context("Missing pieces")?;
        let piece_hashes = chunked_vec_in_place(piece_hashes_bytes)?;
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

    pub(crate) fn self_peer_info(&self) -> PeerInfo {
        PeerInfo {
            id: SELF_PEER_ID,
            info_hash: self.info_hash,
            protocol: PROTOCOL.to_string(),
            extension: EXTENSIONS,
        }
    }

    pub(crate) fn num_pieces(&self) -> u32 {
        self.piece_hashes.len() as u32
    }

    pub(crate) fn piece_len(&self, piece_index: u32) -> u32 {
        let num_pieces = self.num_pieces();
        if piece_index == num_pieces - 1 {
            let full_pieces_len = self.piece_len * (num_pieces - 1);
            self.length - full_pieces_len
        } else {
            self.piece_len
        }
    }

    pub(crate) async fn tracker_peers(&self) -> anyhow::Result<TrackerResponse> {
        let encoded_hash = percent_encode(&self.info_hash, NON_ALPHANUMERIC).to_string();
        let query_str = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("peer_id", str::from_utf8(&SELF_PEER_ID).unwrap())
            .append_pair("port", "6881")
            .append_pair("uploaded", "0")
            .append_pair("downloaded", "0")
            .append_pair("left", &self.length.to_string())
            .append_pair("compact", "1")
            .finish();
        let announce = &self.announce;
        let url = format!("{announce}?info_hash={encoded_hash}&{query_str}");

        let response = reqwest::get(url).await?.bytes().await?;
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

    pub(crate) fn piece_hash(&self, idx: u32) -> [u8; HASH_LEN] {
        self.piece_hashes[idx as usize]
    }
}

fn chunked_vec_in_place<T, const N: usize>(mut arr: Vec<T>) -> anyhow::Result<Vec<[T; N]>> {
    anyhow::ensure!(
        arr.len().is_multiple_of(N),
        "Attempted to chunk a Vec with len {} into chunks of len {}",
        arr.len(),
        N
    );
    arr.shrink_to_fit();
    let chunked =
        unsafe { Vec::from_raw_parts(arr.as_mut_ptr().cast(), arr.len() / N, arr.capacity() / N) };
    std::mem::forget(arr);
    Ok(chunked)
}

pub(crate) fn print_torrent_summary(val: TorrentInfo) {
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

pub(crate) fn read_torrent_file(path: impl AsRef<Path>) -> anyhow::Result<TorrentInfo> {
    let torrent_bytes = std::fs::read(path.as_ref())?;
    TorrentInfo::new(bencode::parse(&torrent_bytes)?)
}
