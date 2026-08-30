# qBittorrent 5.0.5 interoperability result

Date: 2026-08-30  
ShardMeld: 0.4.0  
Third-party peer: qBittorrent 5.0.5 for macOS  
Torrent info-hash: `cbfe49f2c4d44a6a4823ebfa8c829351755d90bb`

## Isolation

qBittorrent ran with a separate disposable profile and only the public SQLite
test torrent. The successful run was bound to `127.0.0.1:45990`; DHT, PeX,
Local Peer Discovery, and port forwarding were disabled. The process was
stopped cleanly after the transfer, and the port was confirmed closed.

The first disposable-profile launch exposed qBittorrent's defaults: it enabled
DHT and listened on all interfaces. That instance was stopped before the
interop transfer. The profile was then explicitly restricted as described
above. No user qBittorrent profile or personal torrent was read.

## Standard peer-wire result

ShardMeld connected directly to qBittorrent and completed the standard v1 flow:

1. `BitTorrent protocol` handshake and matching 20-byte info-hash;
2. qBittorrent bitfield advertising 37 of 37 pieces;
3. interested / unchoke state transition;
4. 87 standard request messages, each at most 16 KiB;
5. piece responses written into the locally reconstructed target;
6. all 37 torrent SHA-1 piece hashes and final SHA-256 verified.

| Measurement | Result |
|---|---:|
| Target bytes | 9,515,341 |
| Bytes already available through CDC | 8,273,479 |
| Genuinely missing bytes | 1,241,862 |
| BT pieces requested | 12 of 37 |
| BT block requests | 87 |
| Actual BT payload received | 1,425,408 |
| 16 KiB alignment redundancy | 183,546 |
| Payload avoided versus full download | 8,089,933 (85.0199%) |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Exact verification | Yes |

Actual wire payload is larger than the theoretical missing payload because a
standard peer serves aligned BT request blocks, not arbitrary CDC byte ranges.
This difference is retained in the report instead of being counted as savings.

## Evidence

- `fetch-report.json`: ShardMeld's measured peer, piece, block, byte, and hash data.
- `rebuilt-from-qbittorrent.bin`: exact reconstructed SQLite 3.53.4 target.
- `packaged-fetch-report.json`: the same transfer repeated with the signed
  `dist/shardmeld-macos-arm64` deliverable.
- `rebuilt-packaged-v04.bin`: output produced by the packaged binary.

This proves direct interoperability with one mature BT peer. It does not yet
include tracker announces, DHT discovery, magnet metadata exchange, multiple
peers, upload/seeding, resume state, multi-file torrents, or BT v2/hybrid.
