# ShardMeld 1.1 magnet-to-qBittorrent verification

Date: 2026-08-31  
ShardMeld: 1.1.0  
External peers: two isolated qBittorrent 5.0.5 processes

## Result

The final signed package parsed this sample's standard base32 v1 magnet hash,
decoded its percent-encoded HTTP Tracker parameter, and bound it to a trusted
local `.torrent` only after the metadata info hash matched exactly. The magnet
Tracker returned two qBittorrent peers; both connected and contributed to the
verified reconstruction.

| Measurement | Result |
|---|---:|
| Magnet encoding | 32-character base32 `btih` |
| Info hash | `cbfe49f2c4d44a6a4823ebfa8c829351755d90bb` |
| Tracker | `http://127.0.0.1:45995/announce` |
| Peers discovered / connected | 2 / 2 |
| Network payload | 1,589,248 bytes |
| Final target bytes | 9,515,341 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

This proves a real magnet entry path through unchanged external peers. It does
not prove metadata exchange: the matching local v1 `.torrent` was deliberately
required and verified before any Tracker or Peer connection.

## Evidence

- `magnet-v11-report.json`: versioned Tracker/Peer transfer report and hashes.
- `rebuilt-magnet-v11.bin`: byte-identical reconstructed output.
