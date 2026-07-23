use std::{net::SocketAddr, path::PathBuf};

use clap::Parser as _;

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

#[derive(clap::Args)]
pub struct DecodeArgs {
    pub bencoded: String,
}

#[derive(clap::Args)]
pub struct InfoArgs {
    pub torrent_path: PathBuf,
}

#[derive(clap::Args)]
pub struct PeersArgs {
    pub torrent_path: PathBuf,
}

#[derive(clap::Args)]
pub struct HandshakeArgs {
    pub torrent_path: PathBuf,
    pub peer: SocketAddr,
}

#[derive(clap::Args)]
pub struct DownloadPieceArgs {
    #[arg(short = 'o')]
    pub output_file_path: PathBuf,
    pub torrent_path: PathBuf,
    pub piece_index: u64,
}

pub fn get_args() -> Command {
    Cli::parse().command
}
