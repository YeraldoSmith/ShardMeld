# qBittorrent 5.0.5 UDP multitracker interoperability

Date: 2026-08-30  
ShardMeld: 0.6.0  
External peer: qBittorrent 5.0.5

## Result

The test torrent used a BEP 12 `announce-list` with two tiers:

1. `http://127.0.0.1:45994/announce` — intentionally unavailable.
2. `udp://127.0.0.1:45993/announce` — an experiment-only BEP 15 tracker.

Both qBittorrent and ShardMeld skipped the unavailable HTTP tracker and
registered with the UDP tracker. ShardMeld completed the UDP connection-ID and
announce exchanges, received qBittorrent at `127.0.0.1:45990`, and performed
the standard peer-wire transfer without a manually supplied peer address.

| Measurement | Result |
|---|---:|
| Info-hash | `cbfe49f2c4d44a6a4823ebfa8c829351755d90bb` |
| Tracker attempts | 2 |
| Tier 0 | HTTP connection refused |
| Tier 1 | UDP success, 1 peer |
| qBittorrent pieces advertised | 37 / 37 |
| Fully local pieces | 25 / 37 |
| Locally available CDC bytes | 8,273,479 |
| Genuinely missing bytes | 1,241,862 |
| Standard block requests | 87 |
| Actual network payload | 1,425,408 |
| Alignment redundancy | 183,546 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

## Isolation

- qBittorrent peer TCP/UDP traffic was bound to `127.0.0.1:45990`.
- The UDP tracker was bound only to `127.0.0.1:45993`.
- DHT, PeX, Local Peer Discovery, embedded Tracker, and port forwarding were
  disabled in the disposable qBittorrent profile.
- TCP port 45994 intentionally had no listener, producing an immediate and
  recorded first-tier failure.
- ShardMeld and qBittorrent both sent UDP started/stopped events.
- All helper processes and test ports were closed after the run.

## Evidence

- `sqlite3-v06-multitracker.torrent`: the exact tiered v1 metadata.
- `fetch-report.json`: tracker attempts, selected peer, peer-wire counters, and
  final hash verification.
- `rebuilt-via-udp.bin`: reconstructed output from the development-binary run.
- `final-packaged-fetch-report.json`: the same external flow performed with the
  final signed `dist/shardmeld-macos-arm64` v0.6.0 deliverable.
- `rebuilt-final-packaged-v06.bin`: reconstructed output from the final package.

This is evidence for BEP 12 tier fallback, base BEP 15 UDP discovery, and
unchanged qBittorrent peer-wire interoperability. It is not evidence for
public trackers, UDP retransmission under loss, BEP 41 extensions, DHT, magnet
metadata exchange, upload, concurrent swarm scheduling, or general savings.
