#![feature(string_from_utf8_lossy_owned)]

use std::{fs::File, io::Write, os::unix::fs::FileExt, sync::Arc};

mod bencode;

mod message;

mod cli;
use cli::Command;

mod torrent_file;
use torrent_file::read_torrent_file;

mod peers;
use peers::Peer;

const MAX_BLOCK_SIZE: u32 = 1024 * 16;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command = cli::parse();
    unsafe {
        // makes the codecrafters tester show stderr because it stupidly swallows it by default
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
            let torrent = read_torrent_file(args.download_args.torrent_path)?;
            let mut output_file = File::create(args.download_args.output_file_path)?;
            let response = torrent.tracker_peers().await?;
            let peer_ip = response.peers[0]; // use the first peer
            let mut peer = Peer::handshake_with(peer_ip, &torrent).await?;
            let token = peer.setup_download().await?;
            let data = peer
                .download_piece(token, &torrent, args.piece_index)
                .await?;
            output_file.write_all(&data)?;
        }
        Command::Download(args) => {
            let torrent = Arc::new(read_torrent_file(args.torrent_path)?);
            let num_pieces = torrent.num_pieces();
            let output_file = Arc::new(tokio::sync::Mutex::new(File::create(
                args.output_file_path,
            )?));
            let response = torrent.tracker_peers().await?;
            let (work_queue_tx, work_queue_rx) = async_channel::bounded(num_pieces as usize);
            for i in 0..num_pieces {
                work_queue_tx.send(i).await.unwrap();
            }
            let task_handles: Vec<_> = response
                .peers
                .into_iter()
                .map(|peer_ip| {
                    tokio::spawn({
                        let tx = work_queue_tx.clone();
                        let rx = work_queue_rx.clone();
                        let torrent = Arc::clone(&torrent);
                        let file = Arc::clone(&output_file);
                        async move {
                            let mut peer = Peer::handshake_with(peer_ip, torrent.as_ref()).await?;
                            let token = peer.setup_download().await?;
                            loop {
                                let Ok(piece_idx) = rx.try_recv() else {
                                    return Result::<(), anyhow::Error>::Ok(());
                                };
                                match peer.download_piece(token, &torrent, piece_idx).await {
                                    Ok(piece) => file
                                        .lock()
                                        .await
                                        .write_all_at(
                                            &piece,
                                            (piece_idx * torrent.piece_len(0)) as u64,
                                        )
                                        .unwrap(),
                                    Err(e) => {
                                        eprintln!("{e}");
                                        tx.send(piece_idx).await.unwrap();
                                    }
                                }
                            }
                        }
                    })
                })
                .collect();
            for handle in task_handles {
                handle.await??;
            }
        }
    }
    Ok(())
}
