# ShardMeld 2.0 seed heartbeat and aggregate rate-limit verification

Date: 2026-09-18

ShardMeld: final ad-hoc-signed `2.0.0` macOS arm64 package

Package SHA-256: `d1d0ea4f19c989b3c65f29e734983d7fa2edbf3c7549b54c39d5cb3bb1dd74cd`

## Packaged aggregate-rate result

The packaged file seed served a packaged ShardMeld downloader through the
standard BitTorrent v1 peer wire with an aggregate limit of 8,388,608 payload
bytes per second. The receiver started with an empty authorized index and
downloaded all 9,515,341 bytes in 581 blocks. All 37 Piece SHA-1 hashes and the
final SHA-256 passed. The seed reported zero protocol errors, the
`fifo-block-fair` scheduler, and 1,516,014 microseconds of accumulated throttle
wait. The complete downloader process took 1.95 seconds, including handshake,
Piece verification, final SHA-256, report writing, and process startup.

This rate is an upper bound shared across all active upload workers. It is not a
claim of exact wall-clock shaping or production congestion control.

## Packaged Tracker heartbeat result

An independent loopback HTTP Tracker returned a one-second announce interval.
It observed three requests from the packaged seed in this order:

1. `event=started`;
2. a regular re-announce with the `event` parameter omitted;
3. `event=stopped` after cooperative Ctrl-C.

The seed report records the same lifecycle as `started`, `update`, and
`stopped`; every request succeeded. The reported listener port matched the
actual bound seed port.

## Automated concurrency and shutdown coverage

The full 72-test suite includes two simultaneous upload peers sharing a single
16 KiB/s limiter. Two 16 KiB blocks require at least 850 ms wall time and both
peers receive valid data. Separate tests cover the index-reconstructed seed
path, reject a zero-byte rate, and prove that cooperative shutdown interrupts a
queued throttle reservation in less than one second even when its nominal wait
would be several hours.

## Evidence boundary

These results prove packaged loopback transfer, configured aggregate shaping,
FIFO block reservation, interval-driven HTTP Tracker renewal, and interruptible
shutdown. They do not prove public-Tracker reachability, NAT traversal,
public-swarm throughput, exact operating-system socket bandwidth, or a mature
tit-for-tat/optimistic-unchoke policy. The earlier unchanged-qBittorrent 5.0.5
interoperability result remains in the indexed-seed experiment.

## Files

- `rate-limit-smoke.json`: sanitized packaged transfer and limiter evidence.
- `heartbeat-seed-report.json`: packaged seed lifecycle report.
- `tracker-observed.json`: fields observed by the independent loopback Tracker.
