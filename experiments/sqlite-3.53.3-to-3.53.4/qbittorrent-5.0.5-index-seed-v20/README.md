# ShardMeld 2.0 index-seed to qBittorrent verification

Date: 2026-08-31  
ShardMeld: final ad-hoc-signed 2.0.0 package  
External receiver: qBittorrent 5.0.5

## Result

The authorized index contained 136 separate CDC material files, not the
complete `sqlite3.c` target. Preflight reconstructed and SHA-1-verified all 37
torrent Pieces. ShardMeld advertised those Pieces, rebuilt each requested
Piece on demand from indexed material, and answered an unchanged qBittorrent
client using standard 16 KiB peer-wire blocks.

| Measurement | Result |
|---|---:|
| Indexed material files / chunks | 136 / 136 |
| Indexed material bytes | 9,515,341 |
| Index database bytes | 151,552 |
| Verified Pieces advertised | 37 / 37 |
| Successful handshakes | 1 |
| Standard block requests | 581 |
| Payload uploaded | 9,515,341 bytes |
| On-demand source reads | 172 chunks / 12,862,697 bytes |
| Protocol errors | 0 |
| Download SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Direct byte comparison | Identical |

The material files were deliberately generated from the public target for a
controlled interoperability test, so this run proves the indexed on-demand
upload path, not storage savings. The complete target was never indexed.
Partially reconstructable Pieces are not advertised. This loopback run does
not prove DHT, public-swarm behavior, production choking, NAT traversal,
multi-file torrents, or BT v2/hybrid support.

## Evidence

- `index-report.json`: exact authorized material inventory and index size.
- `bt-plan.json`: per-Piece local reconstruction and SHA-1 verification.
- `final-packaged-seed-report.json`: repeated counters from the final signed
  `dist/shardmeld-macos-arm64` binary.
- `downloaded-by-qbittorrent-final.bin`: receiver output from the final-package
  repetition.
