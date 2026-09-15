# BitTorrent Client (Rust)

Async BitTorrent client built from scratch in Rust: custom bencode parser, HTTP tracker, peer wire protocol, and concurrent multi-peer downloads over Tokio.

**Stack:** Tokio, nom, SHA-1 piece hashes, pipelined 16 KiB block requests, work-queue across all tracker peers (failed pieces requeued).

## Commands

```
cargo run -- decode <bencoded>                          # bencode → JSON
cargo run -- info <file.torrent>                        # tracker URL, length, info hash, piece hashes
cargo run -- peers <file.torrent>                       # compact tracker announce → ip:port list
cargo run -- handshake <file.torrent> <ip:port>         # TCP handshake, print peer ID
cargo run -- download-piece -o <out> <file.torrent> <i> # one piece (SHA-1 verified)
cargo run -- download -o <out> <file.torrent>           # full file, parallel peers
```
