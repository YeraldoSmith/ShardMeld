# qBittorrent 5.0.5 pipelining and verified resume

Date: 2026-08-30  
ShardMeld: 0.7.0  
External peer: qBittorrent 5.0.5

## Result

The final signed `dist/shardmeld-macos-arm64` binary connected through a
loopback-only throttling proxy to an unchanged qBittorrent peer. The proxy
forwarded the standard TCP peer-wire byte stream and limited only the peer to
client direction to 65,536 bytes per second so the interruption was
deterministic.

ShardMeld was terminated with `SIGTERM` immediately after the first complete
Piece had passed SHA-1 and its resume state was persisted. At that point the
adjacent partial and resume files existed, while the requested final output did
not. The resume JSON marked only Piece 0 complete.

The same final binary then restarted against qBittorrent directly. It rehashed
Piece 0 from the partial file, accepted it, and did not request its five
standard blocks again.

| Measurement | Result |
|---|---:|
| Request window | 16 blocks |
| Persisted and reverified Pieces | 1 |
| Reverified Piece bytes | 262,144 |
| Standard blocks avoided after restart | 5 |
| Peer payload avoided after restart | 81,920 bytes |
| Remaining block requests | 82 |
| Remaining qBittorrent payload | 1,343,488 bytes |
| Final target bytes | 9,515,341 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

The automated suite separately sends a full 16-block request window and
returns those blocks in reverse order. It also changes one byte inside a Piece
marked complete; startup revalidation rejects that Piece and downloads it
again. Together these checks establish request pipelining, out-of-order
response handling, verified resume, and damaged-partial recovery.

## Evidence

- `interruption-evidence.json`: exact interruption state and resumed counters.
- `final-packaged-resume-report.json`: final peer identity, pipeline, resume,
  network, and hash counters from the signed package.
- `verified-resume-v07.bin`: the exact reconstructed target.
- `verified-interrupted-run.log`: intentionally empty because the process was
  terminated before it could emit a success report.

This does not establish multi-peer concurrent downloading, retry scheduling,
DHT, magnet metadata exchange, upload/seeding, or behavior on public swarms.
