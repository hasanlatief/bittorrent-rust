use std::{net::SocketAddrV4, path::PathBuf};

use clap::{Args, Parser as _};

#[derive(clap::Parser)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    Decode(DecodeArgs),
    Info(InfoArgs),
    Peers(PeersArgs),
    Handshake(HandshakeArgs),
    #[command(alias = "download_piece")]
    DownloadPiece(DownloadPieceArgs),
}

#[derive(Args)]
pub struct DecodeArgs {
    pub bencoded: String,
}

#[derive(Args)]
pub struct InfoArgs {
    pub torrent_path: PathBuf,
}

#[derive(Args)]
pub struct PeersArgs {
    pub torrent_path: PathBuf,
}

#[derive(Args)]
pub struct HandshakeArgs {
    pub torrent_path: PathBuf,
    pub peer: SocketAddrV4,
}

#[derive(Args)]
pub struct DownloadPieceArgs {
    #[arg(short = 'o')]
    pub output_file_path: PathBuf,
    pub torrent_path: PathBuf,
    pub piece_index: u32,
}

pub fn parse() -> Command {
    Cli::parse().command
}
