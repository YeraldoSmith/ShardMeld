# ShardMeld（拾构）

[![CI](https://github.com/YeraldoSmith/ShardMeld/actions/workflows/ci.yml/badge.svg)](https://github.com/YeraldoSmith/ShardMeld/actions/workflows/ci.yml)

Copyright © 2026 YeraldoSmith  
Licensed under the GNU Affero General Public License v3.0 only.

ShardMeld reconstructs target data from user-authorized local files and identifies only the bytes that are genuinely missing.

Taking over development? Start with [`HANDOFF.md`](HANDOFF.md) for the verified
baseline, code map, invariants, next priorities, and release checklist.

> Reconstruct first. Transfer only what's missing.  
> 先重建，只传缺失。

Prototype 0.1 through 0.10 build the verified reconstruction and BitTorrent download engine. ShardMeld 1.0 freezes the first machine-readable report contract, 1.1 adds validated v1 magnet entry with trusted local metadata, and 1.2 adds verified full-file upload seeding. ShardMeld 2.0 can advertise and serve standard v1 Pieces reconstructed on demand from the authorized CDC index, without requiring a complete target file in that index. Peer metadata exchange and DHT remain deferred, as do background scanning and GUI work.

当前交付状态：离线重建、v1 BT Piece 映射、Tracker、多 Peer、断点续传、稀有 Piece 优先、安全 Endgame、v1 magnet 本地元数据绑定、完整文件做种和本地索引按需重建做种均已实现，37 项自动化测试通过。2.0 最终包已从 136 个分散材料文件对应的索引动态发布 37 个标准 BT Pieces，让未修改的 qBittorrent 5.0.5 下载出逐字节一致的 9,515,341 字节目标。

## Run the delivered macOS binary

```bash
./dist/shardmeld-macos-arm64 --help
./dist/shardmeld-macos-arm64 capabilities
```

The delivered binary is an ad-hoc-signed Apple Silicon (`arm64`) Mach-O executable. Its SHA-256 is recorded in `dist/SHA256SUMS`.

## Build

```bash
cargo build --release
```

## Minimal flow

```bash
shardmeld index --source ./local-materials --db ./index.db --profile m
shardmeld describe --target ./target.bin --out ./target.meld --profile m
shardmeld compare --descriptor ./target.meld --db ./index.db --json ./compare.json
shardmeld stage-missing --descriptor ./target.meld --target ./target.bin --db ./index.db --out-dir ./missing
shardmeld rebuild --descriptor ./target.meld --db ./index.db --missing-source ./missing --out ./rebuilt.bin --json ./rebuild.json
shardmeld verify --descriptor ./target.meld --file ./rebuilt.bin
```

`stage-missing` is a test-only stand-in for a future network source. It exports only the chunks absent from the local index.

## Minimal network flow

On the machine serving an explicitly staged chunk directory:

```bash
shardmeld serve-chunks --source ./missing --bind 127.0.0.1:45872
```

On the receiver:

```bash
shardmeld fetch-missing --descriptor ./target.meld --db ./index.db --peer 127.0.0.1:45872 --out-dir ./fetched
shardmeld rebuild --descriptor ./target.meld --db ./index.db --missing-source ./fetched --out ./rebuilt.bin
```

The v0.2 transport verifies every chunk at both ends. It is a research transport without encryption, authentication, peer discovery, or swarm scheduling and must not be confused with the future BitTorrent layer.

## BitTorrent v1 bridge

```bash
shardmeld bt-plan \
  --torrent ./target.torrent \
  --descriptor ./target.meld \
  --db ./index.db \
  --json ./bt-plan.json
```

`bt-plan` currently accepts standard v1 single-file torrents only. A piece is
reported as fully local only after ShardMeld reconstructs every byte from the
authorized index and verifies the torrent's SHA-1 piece hash. The command does
not contact trackers or peers and does not modify the torrent.

Directly fetch the missing aligned blocks from a known standard peer:

```bash
shardmeld bt-fetch-peer \
  --torrent ./target.torrent \
  --descriptor ./target.meld \
  --db ./index.db \
  --peer 127.0.0.1:45990 \
  --out ./rebuilt.bin \
  --json ./bt-fetch.json
```

`bt-fetch-peer` implements the basic TCP handshake, bitfield/HAVE availability,
interested/unchoke state, pipelined 16 KiB requests, and out-of-order piece
responses. It connects only to the explicitly supplied peer address.

Automatically process the torrent's multitracker tiers, announce through
HTTP(S) or UDP, and try the returned peers:

```bash
shardmeld bt-fetch-tracker \
  --torrent ./target.torrent \
  --descriptor ./target.meld \
  --db ./index.db \
  --out ./rebuilt.bin \
  --json ./bt-tracker-fetch.json
```

`--tracker` overrides the embedded tracker tiers. v0.10 supports BEP 12
`announce-list`, BEP 15 UDP connect/announce, compact IPv4, compact IPv6, and
dictionary peer lists. Up to four discovered peers receive different whole
Piece jobs concurrently. Failed jobs return to the shared queue for another
compatible peer; five seconds without a peer message is treated as a stall.
Peers in one four-peer batch register their bitfields before work begins. The
scheduler chooses the lowest-availability compatible Piece first. A peer that
finishes quickly immediately claims another job, so capacity shifts toward
faster peers without trusting an unverified speed claim.
When the normal queue empties while a Piece is still active, one compatible
idle peer may duplicate that final Piece. Each worker reconstructs and SHA-1
verifies into a private memory buffer; a serialized commit admits only the
first verified result. A losing worker sends standard CANCEL messages for its
remaining in-flight blocks as soon as it observes the winner. Reports expose
duplicate jobs, cancelled jobs, CANCEL count, and redundant payload.
Started sessions receive a best-effort stopped announce. Interrupted transfers
retain adjacent `.shardmeld-partial` and `.shardmeld-resume.json` files. On the
next run, every completed Piece is rehashed; valid Pieces are skipped and
damaged Pieces are downloaded again. The final output is published only after
all Piece SHA-1 hashes and the target SHA-256 pass. Tracker query strings are
redacted from reports because private trackers commonly place credentials in
announce URLs.

Bind a standard v1 magnet to a trusted local metadata file and use its tracker
parameters:

```bash
shardmeld bt-fetch-magnet \
  --magnet 'magnet:?xt=urn:btih:...&tr=...' \
  --metadata ./target.torrent \
  --descriptor ./target.meld \
  --db ./index.db \
  --out ./rebuilt.bin \
  --json ./magnet-fetch.json
```

The metadata info hash must equal the magnet `btih`. Both 40-character hex and
32-character base32 v1 hashes are accepted. This is a validated magnet entry
path, not BEP 9 metadata exchange: the local `.torrent` file is still required.

Seed a complete verified file to standard v1 clients:

```bash
shardmeld bt-seed-file \
  --torrent ./target.torrent \
  --descriptor ./target.meld \
  --file ./target.bin
```

Or advertise only the Pieces that can be reconstructed and SHA-1-verified from
the authorized CDC index, rebuilding requested Pieces in memory on demand:

```bash
shardmeld bt-seed-index \
  --torrent ./target.torrent \
  --descriptor ./target.meld \
  --db ./index.db
```

Both seed commands bind to loopback by default. `bt-seed-file` verifies the
entire file and every Piece before listening. `bt-seed-index` performs a
preflight Piece reconstruction and advertises no partially available Piece.

## Safety boundaries

- Only directories explicitly passed to `index` are scanned.
- Symbolic links are not followed.
- Indexed payload is never copied into the SQLite database.
- Rebuild validates every chunk and the final SHA-256.
- Existing output files are not overwritten.
- The SQLite index and JSON reports can contain local file paths; keep them private unless paths are sanitized.
- The chunk server binds to loopback by default; non-loopback exposure requires an explicit flag.
- Both BT seed commands also bind to loopback by default. Non-loopback use is an explicit trusted-network choice.

See `docs/PROTOTYPE.md` for scope, metrics, and known limitations.
Measured synthetic results are in `experiments/RESULTS.md`.
The first verified public real-file benchmark is in `experiments/sqlite-3.53.3-to-3.53.4/README.md`.
The qBittorrent interoperability result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-interop-v04/README.md`.
The automatic Tracker discovery result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-tracker-interop-v05/README.md`.
The UDP and multitracker failover result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-udp-multitracker-v06/README.md`.
The pipelining and verified process-restart result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-pipeline-resume-v07/README.md`.
The two-peer qBittorrent result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-multipeer-v08/README.md`.
The rarest-first and per-peer throughput result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-adaptive-v09/README.md`.
The safe Endgame result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-endgame-v010/README.md`.
The 1.0 stable-report release result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-release-v1/README.md`.
The 1.1 magnet-binding result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-magnet-v11/README.md`.
The 1.2 qBittorrent download-from-ShardMeld result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-upload-v12/README.md`.
The 2.0 indexed on-demand seed result is in `experiments/sqlite-3.53.3-to-3.53.4/qbittorrent-5.0.5-index-seed-v20/README.md`.

## Copyright and license

Copyright © 2026 YeraldoSmith.

ShardMeld is licensed under the [GNU Affero General Public License v3.0 only](LICENSE)
(`AGPL-3.0-only`). If you modify the program and make it available for users
to interact with over a network, the AGPL requires offering those users the
corresponding source code under the same license terms. See `LICENSE` for the
complete legal text.
