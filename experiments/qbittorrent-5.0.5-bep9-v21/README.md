# ShardMeld 2.1 BEP 9 interoperability verification

Date: 2026-09-19

ShardMeld: final ad-hoc-signed `2.1.0` macOS arm64 package

Package SHA-256: `a28d5d3b0e4f084e425f4d31dcaa782b30926a94cafac81608d9c4fdb4f29b74`

External peer: unchanged qBittorrent `5.0.5` for macOS

## Result

qBittorrent ran with a disposable profile and the public SQLite fixture. Its
peer listener was confirmed as only `127.0.0.1:46019`; DHT, PeX, and Local Peer
Discovery were disabled. No personal qBittorrent profile was read.

The packaged ShardMeld binary completed the BEP 10 extension handshake,
negotiated qBittorrent's `ut_metadata` extension ID `2`, requested the raw BEP 9
metadata, and received one 811-byte metadata piece. The assembled `info`
dictionary hashed to
`cbfe49f2c4d44a6a4823ebfa8c829351755d90bb`, exactly matching the magnet. It
then parsed the single-file torrent name as `sqlite3.c`.

qBittorrent was terminated cooperatively after the run, and the TCP listener
was confirmed closed. The automated suite separately covers metadata spanning
multiple 16 KiB pieces and rejects an info-hash mismatch.

## Evidence boundary

This proves direct BEP 10/BEP 9 metadata interoperability with an unchanged
external client and exact info-hash verification by the delivered binary. The
metadata Peer address was supplied explicitly. It does not prove DHT, PEX,
automatic metadata-Peer discovery, public-swarm behavior, or v2 `btmh`
metadata.

## Files

- `packaged-metadata-report.json`: versioned report written by the delivered
  ShardMeld binary.
