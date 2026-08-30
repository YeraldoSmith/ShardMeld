# qBittorrent 5.0.5 tracker-discovery interoperability

Date: 2026-08-30  
ShardMeld: 0.5.0  
External peer: qBittorrent 5.0.5

## Result

qBittorrent registered the v1 torrent info-hash
`cbfe49f2c4d44a6a4823ebfa8c829351755d90bb` with an HTTP tracker bound to
`127.0.0.1:45991`. ShardMeld read the announce URL from the torrent, sent its
own started announce, parsed the returned compact peer list, and automatically
connected to qBittorrent at `127.0.0.1:45990`.

| Measurement | Result |
|---|---:|
| Peers discovered | 1 |
| Peer attempts | 1 |
| qBittorrent pieces advertised | 37 / 37 |
| Fully local pieces | 25 / 37 |
| Locally available CDC bytes | 8,273,479 |
| Genuinely missing bytes | 1,241,862 |
| 16 KiB block requests | 87 |
| Actual network payload | 1,425,408 |
| Alignment redundancy | 183,546 |
| Rebuilt target bytes | 9,515,341 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

## Isolation and safety evidence

- qBittorrent peer TCP and UDP sockets were bound to `127.0.0.1:45990`.
- DHT, PeX, Local Peer Discovery, and port forwarding were disabled.
- qBittorrent's embedded tracker was tested first. It listened on all
  interfaces at port 45991 despite the loopback torrent-session setting, so it
  was stopped before any transfer.
- The successful run used the experiment-only `local_http_tracker` helper,
  which refuses non-loopback bind addresses and listened only on
  `127.0.0.1:45991`.
- qBittorrent announced `started`; ShardMeld announced `started` and `stopped`;
  qBittorrent announced `stopped` during clean shutdown.
- After the experiment, qBittorrent, the helper tracker, and all three test
  ports were confirmed closed.

## Evidence

- `fetch-report.json`: tracker discovery, selected peer, peer-wire counters,
  qBittorrent peer ID/reserved bytes, and final verification.
- `final-packaged-fetch-report.json`: the same end-to-end run performed with
  the final signed `dist/shardmeld-macos-arm64` v0.5.0 deliverable.
- `rebuilt-via-tracker.bin`: retained reconstructed output.
- `rebuilt-final-packaged-v05.bin`: retained output from the final packaged-
  binary run.
- `sqlite3-v05-local-tracker.torrent`: v1 metadata used by both clients; its
  `info` dictionary retains info-hash
  `cbfe49f2c4d44a6a4823ebfa8c829351755d90bb`.

This is evidence for HTTP tracker discovery and unchanged third-party peer-wire
interoperability. It is not evidence for UDP trackers, DHT, magnet links,
concurrent swarm scheduling, upload, or arbitrary-file savings.
