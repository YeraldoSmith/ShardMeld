# qBittorrent 5.0.5 safe Endgame interoperability

Date: 2026-08-31  
ShardMeld: 0.10.0  
External peers: two isolated qBittorrent 5.0.5 processes

## Result

The final signed package received two qBittorrent endpoints from a
loopback-only static HTTP tracker. Both peers owned all Pieces. ShardMeld
completed the normal rarest-first queue, duplicated one active tail Piece in
Endgame, accepted only the first independently SHA-1-verified buffer, discarded
the losing verified job, and reproduced the exact target.

| Measurement | Peer A | Peer B | Total |
|---|---:|---:|---:|
| Address | `127.0.0.1:45990` | `127.0.0.1:45992` | 2 peers |
| Pieces committed | 6 | 6 | 12 |
| Block requests | 55 | 42 | 97 |
| Payload | 901,120 bytes | 688,128 bytes | 1,589,248 bytes |
| Endgame jobs discarded | 1 | 0 | 1 |

| Final measurement | Result |
|---|---:|
| Endgame enabled | Yes |
| Endgame duplicate Pieces | 1 |
| Wire CANCEL messages | 0 |
| Local CDC bytes | 8,273,479 |
| Genuinely missing bytes | 1,241,862 |
| Final target bytes | 9,515,341 |
| Final SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Verified | Yes |

The two real loopback seeds were so fast that both competing Piece buffers
finished before a CANCEL could save network traffic. This run therefore proves
real-client compatibility, duplicate scheduling, single-winner publication,
redundant-payload accounting, and exact reconstruction. It does not by itself
prove the wire-CANCEL path.

A deterministic TCP integration test delays the duplicate Peer after accepting
all sixteen requests. Once the other Peer commits, the loser returns one block;
ShardMeld observes completion and sends fifteen standard CANCEL messages. The
test asserts all fifteen messages, one committed Piece, one discarded job, and
an exact final file.

## Evidence

- `final-packaged-endgame-report.json`: peer IDs, Endgame counters, Piece order,
  payload, and final hashes.
- `rebuilt-final-packaged-v010.bin`: exact reconstructed output.
