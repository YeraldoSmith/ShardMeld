# ShardMeld 2.0 completed, interrupt, and upload hardening

Date: 2026-09-06  
ShardMeld: final ad-hoc-signed `2.0.0` package

## Packaged end-to-end result

An independent loopback HTTP Tracker introduced the packaged ShardMeld
downloader to the packaged ShardMeld seed. The downloader started with an empty
authorized index and fetched all 9,515,341 bytes in 581 standard 16 KiB-or-less
requests. All 37 Piece SHA-1 hashes and the final SHA-256 passed.

The Tracker observed a downloader `started` with `left=9515341`, followed by
`completed` and `stopped` with `downloaded=9515341` and `left=0`. The seed
reported `uploaded=9515341` in its `stopped` announce. All five lifecycle
requests succeeded.

## Interrupt result

In a separate packaged-binary run, Ctrl-C stopped a seed with no connected
peers. The command exited successfully, wrote `shutdown_requested=true`, and
the Tracker observed successful `started` and `stopped` events using the actual
ephemeral listener port.

## Bug found by the smoke test

The first end-to-end run completed the handshake but timed out before any block
request. A short shutdown polling timeout could consume part of a framed peer
message and then restart parsing at the wrong byte. The reader now preserves
partial progress across timeout polls. The same packaged flow then transferred
and verified the complete fixture.

Code review also found that UDP Tracker announces had swapped the binary
`downloaded` and `uploaded` counter positions. The implementation and tests now
decode and verify the BEP 15 field offsets.

## Concurrent upload coverage

Automated tests hold one peer open while a second peer completes its handshake
and becomes unchoked. This passes for both complete-file and index-reconstructed
seeds. Index workers open independent SQLite connections. The worker pool is
bounded at four active upload peers.

These results prove the controlled loopback lifecycle, exact packaged transfer,
cooperative Ctrl-C handling, and two-peer concurrency. They do not prove public
Tracker reachability, NAT traversal, public-swarm throughput, periodic Tracker
renewal, rate limiting, a mature choking policy, or forced-kill recovery.

## Evidence

- `completed-smoke.json`: exact transfer and Tracker counters.
- `ctrl-c-smoke.json`: cooperative interrupt and stopped announce.

