# qBittorrent 5.0.5 adaptive scheduler interoperability

Date: 2026-08-31  
ShardMeld: 0.9.0  
External peers: two isolated qBittorrent 5.0.5 processes

## Result

The final signed package received two qBittorrent endpoints from a
loopback-only static HTTP tracker. Both peers registered their standard BT
bitfields before the scheduler released work. Both owned every Piece in this
real sample, so all requested Pieces had equal rarity; the run verifies the
registration barrier, concurrent scheduling, per-peer task history, throughput
measurement, and exact reconstruction. It is not presented as a real-file
rarest-first comparison.

| Measurement | Peer A | Peer B | Total |
|---|---:|---:|---:|
| Address | `127.0.0.1:45990` | `127.0.0.1:45992` | 2 peers |
| Pieces verified | 6 | 6 | 12 |
| Piece order | 0, 15, 29, 31, 33, 35 | 4, 8, 24, 30, 32, 34 | — |
| Block requests | 42 | 45 | 87 |
| Payload | 688,128 bytes | 737,280 bytes | 1,425,408 bytes |
| Active transfer time | 107,211 µs | 103,350 µs | — |
| Measured payload rate | 6,418,445 B/s | 7,133,817 B/s | — |

| Final measurement | Result |
|---|---:|
| Selection strategy | `rarest-first` |
| Peer stall timeout | 5 seconds |
| Local CDC bytes | 8,273,479 |
| Genuinely missing bytes | 1,241,862 |
| Final target bytes | 9,515,341 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

Two deterministic automated scenarios validate what this symmetric real run
cannot:

- Peer A owns Pieces 0/1/2 and Peer B owns 1/2/3. Their first assignments are
  the uniquely available Pieces 0 and 3, proving rarest-first selection after
  the registration barrier.
- Two peers own all eight Pieces, but one delays every block by 5 ms. The fast
  peer completes more Pieces, transfers more payload, and reports higher
  measured throughput, proving work-conserving speed adaptation.

## Evidence

- `final-packaged-adaptive-report.json`: peer IDs, Piece order, active transfer
  times, throughput, scheduler configuration, payload, and final hashes.
- `rebuilt-final-packaged-v09.bin`: exact reconstructed output.

Endgame duplicate requests are intentionally excluded until request
cancellation can prevent competing writes to the same Piece.
