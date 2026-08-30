# SQLite 3.53.3 -> 3.53.4 real-file benchmark

Date: 2026-08-30  
ShardMeld results through: 2.0.0  
Platform: macOS Apple Silicon

This experiment uses two adjacent official SQLite amalgamation releases. It
compares the extracted `sqlite3.c` files, then repeats the same comparison on
the original ZIP archives as a negative control.

## Inputs and verification

| Input | Bytes | SHA-256 | Official SHA3-256 check |
|---|---:|---|---|
| `sqlite3.c` 3.53.3 | 9,514,279 | `87497ab605bedd0dbee27a209c1eeff8c89b229b13f921a7efdbb81a13f779fd` | `28e484abdaa43630e34040ef6ed92be973a1ad54107803d8af5145b889c23ed7` |
| `sqlite3.c` 3.53.4 | 9,515,341 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` | `67f423e9ebbbdc473cbc4772c872ee6b89f31fde4ed0279a5c25d5f65c043a16` |
| amalgamation ZIP 3.53.3 | 2,945,929 | `646421e12aac110282ef8cc68f1a62d4bb15fc7b8f09da0b53e29ee690500431` | `d45c688a8cb23f68611a894a756a12d7eb6ab6e9e2468ca70adbeab3808b5ab9` |
| amalgamation ZIP 3.53.4 | 2,946,650 | `1e71ddf93849c6a6ecf58b827c0692073d2dd7ee40196158068f7b29f422e87d` | `628a44cfe82c66aed1ccbbe85a562d2e33ebe64b3288981ed76285612227934e` |

Download URLs:

- `https://www.sqlite.org/2026/sqlite-amalgamation-3530300.zip`
- `https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip`

The two `sqlite3.c` SHA3-256 values match SQLite's release-history entries.
The 3.53.4 ZIP SHA3-256 matches the current official download page. The older
ZIP SHA3-256 is recorded here as a locally measured value; it is not presented
as an independently published checksum.

## Extracted source result

| Profile | Target chunks | Matched | Reusable bytes | Missing bytes | Reuse | Index bytes | Index time |
|---|---:|---:|---:|---:|---:|---:|---:|
| S (8/32/128 KiB) | 274 | 257 | 8,807,728 | 707,613 | **92.5635%** | 110,592 | 29 ms |
| M (16/64/256 KiB) | 136 | 121 | 8,273,479 | 1,241,862 | **86.9488%** | 90,112 | 32 ms |
| L (64/256/1024 KiB) | 33 | 24 | 5,977,989 | 3,537,352 | **62.8247%** | 65,536 | 30 ms |

For profile M, 15 missing chunks totalling 1,241,862 bytes were transferred
between independent loopback processes. Reconstruction used 8,273,479 local
bytes and produced the exact 9,515,341-byte target with SHA-256
`b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189`.

## BitTorrent v1 compatibility bridge

A standard single-file v1 torrent was generated for the verified 3.53.4
`sqlite3.c` using a 262,144-byte piece length. ShardMeld parsed its bencoded
`info` dictionary, calculated info-hash
`cbfe49f2c4d44a6a4823ebfa8c829351755d90bb`, and mapped the profile-M CDC plan
onto 37 BT pieces.

| Measurement | Result |
|---|---:|
| Total BT pieces | 37 |
| Fully local and SHA-1 verified | **25** |
| Not yet complete | 12 |
| Partially local pieces | 11 |
| All locally covered bytes | 8,273,479 (86.9488%) |
| Bytes inside fully reconstructable pieces | 6,369,613 (66.9405%) |
| Genuinely missing bytes | 1,241,862 |

This proves the local compatibility boundary: 25 pieces could be marked
complete by a future BT engine without downloading them. It does not yet prove
tracker, DHT, magnet, peer-wire, upload, or swarm interoperability.

## qBittorrent 5.0.5 interoperability

Prototype 0.4 subsequently connected to an isolated qBittorrent 5.0.5 instance
using the standard TCP peer-wire protocol. qBittorrent advertised all 37
pieces. ShardMeld requested 87 aligned blocks across the 12 incomplete pieces,
received 1,425,408 bytes, and combined them with 8,273,479 locally available
bytes. All piece SHA-1 hashes and the final target SHA-256 passed.

Because BT requests are 16 KiB aligned, 183,546 transferred bytes overlapped
data that was already available locally. Even with that honest wire-level
cost, the actual peer payload was only 14.9801% of the 9,515,341-byte target.

Full evidence is in `qbittorrent-5.0.5-interop-v04/`.

Prototype 0.5 repeated this result without a manually supplied peer address.
Both qBittorrent and ShardMeld announced the same info-hash to a loopback-only
HTTP tracker. ShardMeld parsed the compact peer response, discovered
`127.0.0.1:45990`, and produced the same payload counts and final SHA-256.
Evidence is in `qbittorrent-5.0.5-tracker-interop-v05/`.

Prototype 0.6 added two multitracker tiers without changing the info-hash. The
first HTTP tier was deliberately unavailable. ShardMeld continued to the
second, completed a BEP 15 UDP tracker exchange, discovered qBittorrent, and
again produced the same payload counts and final SHA-256. Evidence is in
`qbittorrent-5.0.5-udp-multitracker-v06/`.

Prototype 0.7 changed the single-peer transfer loop from one outstanding block
to a 16-block pipeline and added Piece-granular restart state. In the packaged
qBittorrent test, the process was terminated after one 262,144-byte Piece had
passed SHA-1. Restart reverified that Piece, avoided five requests totaling
81,920 bytes, fetched the remaining 82 blocks, and reproduced the same target
SHA-256. Evidence is in `qbittorrent-5.0.5-pipeline-resume-v07/`.

Prototype 0.8 used two independent qBittorrent 5.0.5 processes. The packaged
client connected to both concurrently; each external peer supplied six missing
Pieces. Their respective payloads were 704,512 and 720,896 bytes, totaling the
same 1,425,408 aligned bytes and exact final SHA-256. Evidence is in
`qbittorrent-5.0.5-multipeer-v08/`.

Prototype 0.9 adds a bitfield-registration barrier, rarest-first ordering,
work-conserving speed adaptation, and per-peer task/throughput reporting. The
real two-qBittorrent run again split 12 missing Pieces evenly and reproduced
the target exactly. Synthetic availability and latency cases separately prove
the rarity ordering and fast-peer load shift. Evidence is in
`qbittorrent-5.0.5-adaptive-v09/`.

Prototype 0.10 adds integrity-preserving Piece-level Endgame. Each competing
Peer assembles into a private buffer, verifies SHA-1, and competes for one
serialized commit. The real two-qBittorrent run triggered one duplicate and
discarded the losing verified job without altering the final file. The peers
were too fast for a wire CANCEL to save traffic, so a controlled TCP case
separately verifies fifteen standard CANCEL messages. Evidence is in
`qbittorrent-5.0.5-endgame-v010/`.

ShardMeld 1.0 freezes `shardmeld-report` version 1 and exposes a machine-readable
capabilities inventory. The signed 1.0 package repeated the two-qBittorrent
experiment with the versioned envelope at both report levels and reproduced
the same exact target. Evidence is in `qbittorrent-5.0.5-release-v1/`.

ShardMeld 1.1 accepted the sample's base32 v1 magnet, used its percent-encoded
Tracker URL, required an exact info-hash match from the local `.torrent`
metadata, connected both qBittorrent peers, and reproduced the target. Evidence
is in `qbittorrent-5.0.5-magnet-v11/`.

ShardMeld 1.2 reversed the peer-wire direction: an unchanged qBittorrent 5.0.5
process downloaded the full target from ShardMeld's verified file seed. In
ShardMeld 2.0, the complete target was excluded from the index and represented
by 136 separate CDC material files. ShardMeld advertised 37/37 verified Pieces,
reconstructed them on demand, and served qBittorrent a byte-identical output.
Evidence is in `qbittorrent-5.0.5-upload-v12/` and
`qbittorrent-5.0.5-index-seed-v20/`.

## Compressed ZIP negative control

| Profile | Target chunks | Matched | Reusable bytes | Missing bytes | Reuse |
|---|---:|---:|---:|---:|---:|
| S | 70 | 2 | 52,521 | 2,894,129 | **1.7824%** |
| M | 37 | 0 | 0 | 2,946,650 | **0.0000%** |
| L | 7 | 0 | 0 | 2,946,650 | **0.0000%** |

This is an important product boundary: content-defined chunking can recover
large unchanged regions in the unpacked content, but separately compressed
archives may change almost everywhere at the byte level. ShardMeld must not
claim comparable savings for arbitrary existing torrent payloads.

## Evidence map

- `raw-sqlite3-c/{s,m,l}/compare-report.json`: source-file comparison plans.
- `compressed-zip/{s,m,l}/compare-report.json`: compressed negative control.
- `raw-sqlite3-c/m/verify-report.json`: offline exact-rebuild proof.
- `network-raw-m/fetch-report.json`: bytes fetched by the receiving process.
- `network-raw-m/server-report.json`: bytes served by the peer process.
- `network-raw-m/verify-report.json`: network-assisted exact-rebuild proof.
- `bt-v1-bridge-m/sqlite3.c.torrent`: standard single-file v1 test metadata.
- `bt-v1-bridge-m/bt-plan-report.json`: per-piece local coverage and SHA-1 proof.
- `qbittorrent-5.0.5-interop-v04/fetch-report.json`: third-party peer-wire result.
- `qbittorrent-5.0.5-tracker-interop-v05/fetch-report.json`: automatic tracker
  discovery plus third-party peer-wire result.
- `qbittorrent-5.0.5-udp-multitracker-v06/fetch-report.json`: HTTP-tier failure,
  UDP-tier success, and third-party peer-wire result.
- `qbittorrent-5.0.5-pipeline-resume-v07/final-packaged-resume-report.json`:
  verified restart, skipped blocks, remaining payload, and final hash.
- `qbittorrent-5.0.5-multipeer-v08/final-packaged-multipeer-report.json`:
  two external contributors, per-peer Piece and payload totals, and final hash.
- `qbittorrent-5.0.5-adaptive-v09/final-packaged-adaptive-report.json`:
  per-peer Piece order, active time, throughput, scheduler mode, and final hash.
- `qbittorrent-5.0.5-endgame-v010/final-packaged-endgame-report.json`:
  real Endgame duplicate, single-winner discard, payload, and final hash.
- `qbittorrent-5.0.5-release-v1/release-v1-report.json`: stable report version,
  external Peer evidence, scheduler counters, and final hash.
- `qbittorrent-5.0.5-magnet-v11/magnet-v11-report.json`: base32 magnet binding,
  magnet Tracker discovery, external Peers, and final hash.
- `qbittorrent-5.0.5-upload-v12/seed-report.json`: ShardMeld-to-qBittorrent
  full-file upload counters and verified identity.
- `qbittorrent-5.0.5-index-seed-v20/final-packaged-seed-report.json`: repetition
  using the exact signed 2.0 delivery binary, including indexed on-demand Piece
  reconstruction and real qBittorrent receiver counters.

The downloaded inputs are intentionally kept outside the deliverable. The
reports, descriptors, indexes, missing chunks, and rebuilt outputs are retained
for reproducibility.
