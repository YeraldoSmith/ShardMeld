# qBittorrent 5.0.5 multi-peer interoperability

Date: 2026-08-30  
ShardMeld: 0.8.0  
External peers: two isolated qBittorrent 5.0.5 processes

## Result

Two independent qBittorrent profiles were bound to `127.0.0.1:45990` and
`127.0.0.1:45992`. DHT, PeX, Local Peer Discovery, embedded trackers, and port
forwarding were disabled in both profiles. Both verified and seeded the same
9,515,341-byte SQLite target.

A loopback-only static HTTP tracker returned both endpoints to the final signed
`dist/shardmeld-macos-arm64` package. ShardMeld connected to both at the same
time and assigned different missing Pieces from its shared scheduler.

| Measurement | Peer A | Peer B | Total |
|---|---:|---:|---:|
| Address | `127.0.0.1:45990` | `127.0.0.1:45992` | 2 peers |
| Pieces verified | 6 | 6 | 12 |
| Block requests | 43 | 44 | 87 |
| Payload | 704,512 bytes | 720,896 bytes | 1,425,408 bytes |

| Final measurement | Result |
|---|---:|
| Concurrent peer limit | 4 |
| Connected peers | 2 |
| Contributing peers | 2 |
| Reassigned Pieces | 0 |
| Local CDC bytes | 8,273,479 |
| Genuinely missing bytes | 1,241,862 |
| Alignment redundancy | 183,546 bytes |
| Final target bytes | 9,515,341 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

The automated suite separately proves complementary peer availability and a
failure path: a peer returns one 16 KiB block, then stalls. After five seconds
its Piece is returned to the queue, another peer downloads the whole Piece,
and the report counts both the reassignment and the duplicated 16 KiB.

## Evidence

- `final-packaged-multipeer-report.json`: per-peer connections, Pieces, block
  requests, payload totals, shared-scheduler counters, and final hashes.
- `rebuilt-final-packaged-v08.bin`: exact reconstructed output.

This proves concurrent Piece acquisition from two unchanged external BT
clients on loopback. It does not prove public-swarm behavior, rarest-first
scheduling, adaptive peer scoring, DHT, magnet exchange, or upload/seeding by
ShardMeld.
