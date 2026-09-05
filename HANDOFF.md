# ShardMeld development handoff

This file is the short path for a new developer or coding agent taking over the
repository. The canonical repository is
<https://github.com/YeraldoSmith/ShardMeld>, branch `main`.

## Current verified baseline

- Product version: `2.0.0`.
- Stable machine-readable envelope: `shardmeld-report`, version `1`.
- Automated verification: 37 tests.
- GitHub Actions uses the pinned Rust `1.97.1` toolchain and runs formatting,
  strict clippy, all tests, and a locked release build for every push and pull
  request. Pinning prevents a new stable Clippy lint from invalidating an
  otherwise unchanged verified release.
- Fresh-clone verification completed with Rust/Cargo `1.97.1` using locked
  dependencies. This is a tested toolchain, not a declared minimum supported
  Rust version.
- Delivered binary: ad-hoc-signed macOS Apple Silicon executable in `dist/`.
- External interoperability: unchanged qBittorrent `5.0.5` downloaded a
  9,515,341-byte target from the final packaged ShardMeld 2.0 index seed; all
  37 Pieces and the final SHA-256 verified.

Run this immediately after cloning:

```bash
git clone https://github.com/YeraldoSmith/ShardMeld.git
cd ShardMeld
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
./target/release/shardmeld --version
./target/release/shardmeld capabilities
(cd dist && shasum -a 256 -c SHA256SUMS)
```

The checked-in `dist/shardmeld-macos-arm64` runs only on Apple Silicon macOS.
The Rust source is the canonical implementation; rebuild it for other targets.

## Repository map

- `crates/meld-cli/src/main.rs` — all user-facing commands and report output.
- `crates/meld-core/src/chunker.rs` — deterministic Gear CDC chunking.
- `crates/meld-core/src/index.rs` — authorized SQLite `hash -> path/offset`
  index; payload is not copied into the database.
- `crates/meld-core/src/workflow.rs` — compare, stage, rebuild, and final
  verification flows.
- `crates/meld-core/src/bittorrent.rs` — v1 metainfo parsing and local
  CDC-to-Piece planning.
- `crates/meld-core/src/bt_peer.rs` — download peer wire, resume, four-peer
  scheduling, rarest-first, and safe Piece-level Endgame.
- `crates/meld-core/src/bt_tracker.rs` — HTTP(S), UDP, and multitracker
  discovery.
- `crates/meld-core/src/magnet.rs` — v1 magnet parsing and trusted local
  metadata binding.
- `crates/meld-core/src/bt_seed.rs` — verified complete-file seeding and 2.0
  on-demand index Piece seeding.
- `crates/meld-core/src/capabilities.rs` — canonical implemented/deferred/limit
  inventory. Update this whenever scope changes.
- `crates/meld-core/tests/` — protocol and reconstruction integration tests.
- `experiments/RESULTS.md` — measured history and evidence boundaries.
- `experiments/sqlite-3.53.3-to-3.53.4/` — real-file and qBittorrent evidence.
- `scripts/` — reproducible local smoke and profile runners.

The repository intentionally includes retained experiment outputs, so a fresh
checkout is much larger than the source alone. `target/`, `/work`, SQLite
indexes, partial files, and local build state are ignored.

## Shipped scope

The authoritative list is produced by `shardmeld capabilities`. ShardMeld 2.0
currently implements:

- explicitly authorized CDC indexing and exact SHA-256 reconstruction;
- standard single-file BitTorrent v1 parsing and Piece verification;
- direct Peer, HTTP(S)/UDP Tracker, and multitracker download paths;
- Piece-granular resume, up to four download peers, rarest-first assignment,
  work-conserving scheduling, and safe Piece-level Endgame with CANCEL;
- v1 hexadecimal/base32 magnet entry when matching local `.torrent` metadata
  is supplied;
- verified full-file upload seeding;
- on-demand upload of Pieces reconstructed from the authorized local index.

Explicitly deferred: DHT, BEP 9 magnet metadata exchange, PEX, BT v2/hybrid,
multi-file torrents, and GUI work. Upload connections are currently serial and
upload Tracker registration is manual.

## Invariants that must not regress

1. Never advertise or commit a Piece until its reconstructed bytes match the
   torrent SHA-1.
2. Never publish the final download output until every Piece and the target
   descriptor SHA-256 verify.
3. Competing Endgame workers write to private buffers; only one verified result
   may commit.
4. Index only paths explicitly authorized by the user, never follow symbolic
   links, and never copy indexed payload into SQLite.
5. Refuse to overwrite an existing rebuild output.
6. Loopback is the default for all research and seed listeners; non-loopback
   exposure requires an explicit flag.
7. Redact Tracker query strings in persisted reports because private Trackers
   can embed credentials.
8. Treat `.meld` as unsigned metadata. Do not claim publisher authentication.
9. Keep measured results, protocol counters, and future claims separate. A
   controlled loopback result is not evidence of public-swarm behavior.
10. Before committing public experiment reports, replace local absolute paths
    and scan for credentials.

## Recommended next work

The lowest-risk continuation of the BT-compatibility strategy is:

1. add automatic started/completed/stopped Tracker announces for upload seeds;
2. support concurrent upload connections with bounded queues, rate limits, and
   an explicit fairness/choking policy;
3. add tests with partial index availability and mixed external seeders;
4. then consider BEP 9 metadata exchange and DHT;
5. address multi-file v1 and BT v2/hybrid before building a GUI.

Do not promote one of these items to an implemented capability until an
automated test and, where interoperability is claimed, an unchanged external
client run both pass.

## Release checklist

1. Update the workspace version in `Cargo.toml`.
2. Keep the peer ID prefix in `bt_peer.rs` and HTTP User-Agent in
   `bt_tracker.rs` consistent with the release.
3. Update `capabilities.rs`, README, prototype limits, and measured results.
4. Run formatting, locked clippy, all tests, and a locked release build.
5. Build and ad-hoc sign the exact `dist/shardmeld-macos-arm64` artifact.
6. Regenerate `dist/CAPABILITIES.json` and `dist/SHA256SUMS` from that artifact.
7. Repeat the promised qBittorrent interoperability flow using the exact final
   `dist` binary, not only `target/release/shardmeld`.
8. Preserve the report and receiver output under `experiments/`; state exactly
   what the run does and does not prove.
9. Sanitize local paths, run a credential scan, and ensure no tracked file
   exceeds GitHub's size limit.

## License and ownership

Copyright © 2026 YeraldoSmith. The project is licensed under
`AGPL-3.0-only`; see `LICENSE` and `COPYRIGHT`.
