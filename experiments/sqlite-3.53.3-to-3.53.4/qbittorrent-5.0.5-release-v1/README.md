# ShardMeld 1.0 qBittorrent release verification

Date: 2026-08-31  
ShardMeld: 1.0.0  
External peers: two isolated qBittorrent 5.0.5 processes

## Result

The final signed 1.0 package exposed `shardmeld-report` version 1 at both the
Tracker report and nested transfer-report levels. It discovered two unchanged
qBittorrent peers through the loopback-only static tracker, connected to both,
accepted six committed Pieces from each, discarded one losing Endgame job, and
rebuilt the exact public SQLite target.

| Measurement | Result |
|---|---:|
| Engine version | `1.0.0` |
| Report format | `shardmeld-report` |
| Report version | 1 |
| Peers discovered / connected | 2 / 2 |
| Contributing peers | 2 |
| Endgame duplicate / discarded | 1 / 1 |
| Network payload | 1,589,248 bytes |
| Final target bytes | 9,515,341 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

Both real peers were full local seeds and responded extremely quickly. The
run verifies the stable report envelope and preserves the v0.10 real-client
Endgame result. The controlled v0.10 test remains the evidence for fifteen
observable CANCEL messages.

## Evidence

- `release-v1-report.json`: versioned report envelope, external Peer IDs,
  scheduler counters, payload, and final hash.
- `rebuilt-release-v1.bin`: byte-identical reconstructed output.
