# ShardMeld 2.0 seed Tracker lifecycle verification

Date: 2026-09-06  
ShardMeld: final ad-hoc-signed `2.0.0` package

## Result

The packaged Apple Silicon binary verified the retained 9,515,341-byte SQLite
fixture and all 37 v1 Pieces before announcing to an independent loopback HTTP
Tracker process. The Tracker observed `event=started` with the seed's actual
ephemeral listener port and `left=0`, followed by `event=stopped` on clean
exit. Both responses were accepted and recorded in the seed report.

| Measurement | Result |
|---|---:|
| Source SHA-256 verified | Yes |
| Pieces verified before announce | 37 |
| Listener reported to Tracker | `127.0.0.1:56612` |
| Started announces | 1 successful |
| Stopped announces | 1 successful |
| Tracker failures | 0 |
| Seed protocol errors | 0 |

A separate automated test uses a deliberately unavailable Tracker and proves
that the failure is recorded without disabling direct peer seeding. Private
Tracker query strings are retained in the request but redacted from reports.

A pre-existing complete seed does not send BitTorrent's `completed` event;
that event is reserved for an incomplete downloader becoming complete.
`stopped` is currently best-effort and requires a clean seed command exit.

This controlled loopback run proves HTTP announce encoding and seed lifecycle
wiring. It does not prove public-Tracker reachability, NAT traversal, or signal-
safe stopped delivery after forced termination.

## Evidence

- `seed-report.json`: packaged-binary verification and lifecycle results.
- `tracker-observed.json`: fields observed by the independent HTTP Tracker.
