# Prototype 0.1 through ShardMeld 2.0 measured results

Date: 2026-08-30  
Machine phase: local synthetic fixture plus verified public real files  
Evidence status: measured prototype behavior, not a general savings claim

## Real adjacent-version result: SQLite 3.53.3 -> 3.53.4

The first public real-file benchmark uses adjacent official SQLite amalgamation
releases. Both extracted `sqlite3.c` files were checked against the SHA3-256
values in SQLite's release history. The current 3.53.4 ZIP also matched the
checksum on SQLite's download page.

| Input representation | Profile S reuse | Profile M reuse | Profile L reuse |
|---|---:|---:|---:|
| Extracted `sqlite3.c` | **92.5635%** | **86.9488%** | **62.8247%** |
| Original ZIP archive | **1.7824%** | **0.0000%** | **0.0000%** |

With profile M, the extracted-source experiment reused 8,273,479 local bytes,
fetched 1,241,862 bytes across a real loopback connection in 15 chunks, and
rebuilt the exact 9,515,341-byte target. Final SHA-256 verification passed.

The contrast is the main finding: the prototype can reuse substantial regions
between related uncompressed files, while ordinary archive compression can
erase nearly all byte-level reuse. Existing BT payloads must therefore be
measured in their actual distributed form; source-file performance cannot be
projected onto compressed archives.

The v0.3 bridge then mapped the profile-M plan onto a standard single-file v1
torrent with 256 KiB pieces. Of 37 BT pieces, 25 were reconstructed entirely
from the old SQLite file and passed their torrent SHA-1 hashes. Those pieces
represent 6,369,613 bytes (66.9405% of the target) that a future BT engine could
mark complete immediately. The remaining 12 pieces include 11 partially local
pieces and require 1,241,862 genuinely missing bytes in total.

## Prototype 0.4: external BitTorrent peer interoperability

ShardMeld connected directly to qBittorrent 5.0.5 over the standard TCP
peer-wire protocol. The successful test used a disposable qBittorrent profile
bound only to `127.0.0.1`, with DHT, PeX, Local Peer Discovery, and port
forwarding disabled.

| Measurement | Result |
|---|---:|
| Remote pieces advertised | 37 / 37 |
| Pieces needing BT blocks | 12 |
| Standard 16 KiB block requests | 87 |
| Theoretical CDC-missing payload | 1,241,862 bytes |
| Actual peer payload | 1,425,408 bytes |
| Alignment redundancy | 183,546 bytes |
| Payload avoided vs full download | 8,089,933 bytes (85.0199%) |
| Final SHA-256 verified | Yes |

This crosses the first ecosystem boundary: the bytes came from a mature,
unchanged third-party BitTorrent client, not ShardMeld's research server. It is
still direct-peer interoperability rather than a complete swarm client.

## Prototype 0.5: tracker discovery to qBittorrent

ShardMeld 0.5 announced to a loopback-only HTTP tracker using the standard raw
20-byte info-hash and peer ID parameters. qBittorrent 5.0.5 independently
registered itself with that tracker. ShardMeld received the compact peer entry
`127.0.0.1:45990`, connected without a manually supplied peer address, and
repeated the exact v0.4 transfer result.

| Measurement | Result |
|---|---:|
| Peers discovered | 1 |
| Selected peer | `127.0.0.1:45990` |
| Remote pieces advertised | 37 / 37 |
| Actual peer payload | 1,425,408 bytes |
| Final SHA-256 verified | Yes |

The qBittorrent embedded tracker was evaluated first, but it listened on all
interfaces even though the torrent session was bound to loopback. It was shut
down before transfer. The successful run used the experiment-only ShardMeld
tracker bound explicitly to `127.0.0.1`; both qBittorrent and ShardMeld sent
their own standard announces to it. This proves HTTP tracker discovery and
external-peer interoperability, not production tracker completeness.

## Prototype 0.6: UDP tracker and multitracker failover

The v1 torrent retained the same info-hash while adding two BEP 12 tiers. Tier
0 pointed at a deliberately unavailable HTTP tracker. Tier 1 pointed at an
experiment-only BEP 15 UDP tracker bound to `127.0.0.1:45993`. qBittorrent
5.0.5 registered with the UDP tracker; ShardMeld recorded the HTTP refusal,
completed the UDP connect and announce exchanges, discovered qBittorrent, and
rebuilt the exact target.

| Measurement | Result |
|---|---:|
| Tier 0 HTTP result | Connection refused |
| Tier 1 UDP result | 1 peer returned |
| Selected peer | `127.0.0.1:45990` |
| Remote pieces advertised | 37 / 37 |
| Actual peer payload | 1,425,408 bytes |
| Final SHA-256 verified | Yes |

This run proves tracker-tier failover plus the base UDP tracker protocol against
an unchanged external BitTorrent client. It does not prove public tracker
behavior, BEP 41 authentication/path extensions, or DHT.

Full inputs, checksums, tables, JSON evidence, and limits are recorded in
`experiments/sqlite-3.53.3-to-3.53.4/README.md`.

## Prototype 0.7: pipelined blocks and verified restart

The final signed 0.7.0 package used a 16-request window against qBittorrent
5.0.5. A loopback-only transparent proxy limited the peer-to-client stream so
the process could be terminated deterministically after one complete Piece was
verified and persisted. No final output existed at interruption time.

On restart, ShardMeld rehashed the saved Piece, skipped its five 16 KiB block
requests, downloaded the remaining 82 blocks, removed the completed sidecars,
and reproduced the exact target SHA-256.

| Measurement | Result |
|---|---:|
| Request window | 16 |
| Resumed verified Pieces | 1 |
| Resumed Piece bytes | 262,144 |
| Payload avoided by resume | 81,920 bytes |
| Remaining peer payload | 1,343,488 bytes |
| Final SHA-256 verified | Yes |

The automated suite additionally proves reverse-order block responses and
rejects a one-byte corruption inside a Piece previously marked complete.

## Prototype 0.8: concurrent multi-peer Piece scheduling

The final signed 0.8.0 package received two loopback endpoints from a static
HTTP tracker. Each endpoint was an independent qBittorrent 5.0.5 process with
all non-tracker discovery disabled. Both contributed simultaneously.

| Measurement | Peer A | Peer B | Total |
|---|---:|---:|---:|
| Pieces verified | 6 | 6 | 12 |
| Block requests | 43 | 44 | 87 |
| Payload bytes | 704,512 | 720,896 | 1,425,408 |

The final SHA-256 matched. A separate automated failure case proves that a
partially responding peer is evicted after five seconds without a message and
its unverified Piece is reassigned to another peer.

## Prototype 0.9: rarest-first and speed-adaptive work claiming

Peers now register availability before a batch starts. The queue selects the
compatible Piece with the lowest registered availability count. Faster peers
claim more jobs simply by returning to the queue sooner; no self-reported speed
is trusted.

The signed-package SQLite run used two full qBittorrent seeds, so every missing
Piece had equal rarity. Each peer completed six Pieces. The report records
Piece order and measured payload rates of 6,418,445 B/s and 7,133,817 B/s, and
the final SHA-256 matched.

Separate deterministic tests prove the claims that require asymmetric peers:
unique Pieces 0 and 3 are selected before shared Pieces, and a zero-delay peer
claims more of eight jobs than a peer delaying every block by 5 ms.

## Prototype 0.10: integrity-preserving Endgame

When the ordinary queue becomes empty, one idle compatible worker may duplicate
an active final Piece. Network bytes are assembled in per-worker memory rather
than written concurrently to the shared partial file. Each buffer must pass the
torrent SHA-1; a serialized commit then accepts only the first winner. A loser
that observes completion sends CANCEL for every remaining in-flight request.

The signed-package SQLite run again used two unmodified qBittorrent 5.0.5
seeds. It triggered one real Endgame duplicate. Both peers were fast enough to
finish their competing work before a wire CANCEL was useful, so the second
verified result was discarded and 163,840 extra payload bytes were measured.
The final file remained byte-identical.

| Measurement | Result |
|---|---:|
| Real qBittorrent peers connected | 2 |
| Endgame duplicate Pieces | 1 |
| Losing jobs discarded | 1 |
| Wire CANCEL messages in symmetric real run | 0 |
| Network payload | 1,589,248 bytes |
| Final SHA-256 verified | Yes |

A deterministic TCP test makes the loser respond once after the other Peer has
won. ShardMeld then sends 15 standard CANCEL messages for the remaining 16 KiB
requests, commits exactly one Piece, and reproduces the target. The real run
proves unchanged qBittorrent interoperability and safe duplicate discard; the
controlled test proves the observable CANCEL path.

## ShardMeld 1.0: stable evidence contract

The first release baseline adds `shardmeld-report` version 1 fields to direct
Peer and Tracker transfer reports and a `capabilities` command that explicitly
separates implemented features from deferred work. The release binary repeated
the two-qBittorrent SQLite experiment: two peers connected, each committed six
Pieces, one losing Endgame job was discarded, and the final SHA-256 matched.

| Measurement | Result |
|---|---:|
| Engine version | 1.0.0 |
| Report format | `shardmeld-report` |
| Report version | 1 |
| Automated tests | 32 passed |
| Real qBittorrent peers | 2 |
| Final target verified | Yes |

## ShardMeld 1.1: magnet entry with trusted local metadata

The signed package parsed the public sample's base32 v1 `btih`, obtained the
loopback Tracker URL from the magnet, and rejected any metadata whose v1 info
hash would differ. With the matching local `.torrent`, it discovered two
unchanged qBittorrent 5.0.5 peers and rebuilt the exact target.

| Measurement | Result |
|---|---:|
| Magnet hash encoding | base32 |
| Bound info hash | `cbfe49f2c4d44a6a4823ebfa8c829351755d90bb` |
| Tracker source | magnet `tr` parameter |
| Peers discovered / connected | 2 / 2 |
| Network payload | 1,589,248 bytes |
| Final target verified | Yes |

Unit coverage separately proves uppercase hex, base32 equivalence, duplicate
Tracker removal, conflicting-info-hash rejection, and unsupported Tracker
scheme rejection. This is local metadata binding, not BEP 9 exchange.

## ShardMeld 1.2: verified upload to qBittorrent

An unchanged qBittorrent 5.0.5 process downloaded the full public SQLite target
from ShardMeld's new upload listener. ShardMeld verified the source SHA-256 and
all 37 torrent Piece SHA-1 values before it listened.

| Measurement | Result |
|---|---:|
| Standard block requests | 581 |
| Payload uploaded | 9,515,341 bytes |
| Protocol errors | 0 |
| Receiver output verified | Yes |

## ShardMeld 2.0: on-demand index seeding

The complete target was excluded from the authorized index. Instead, the index
contained 136 separate files, each holding one CDC material chunk. ShardMeld
preflight-verified and advertised 37/37 Pieces, reconstructed requested Pieces
from those locations, and served all 581 standard requests to qBittorrent.

| Measurement | Result |
|---|---:|
| Indexed files / chunks | 136 / 136 |
| Advertised Pieces | 37 / 37 |
| Payload uploaded | 9,515,341 bytes |
| On-demand source reads | 172 chunks / 12,862,697 bytes |
| Protocol errors | 0 |
| Receiver byte comparison | Identical |

This controlled material layout proves the indexed upload path, not a storage
savings ratio. The complete target was intentionally transformed into separate
material files for the test.

## Fixture

- Local source: deterministic 16 MiB `base-v1.bin`.
- Target: the source plus a 65,537-byte insertion and a 262,144-byte modified region.
- Target size: 16,842,753 bytes.
- Target SHA-256: `f437a9a4e82b28ff74486d73d0d1814ea7d2a4ee22f81361a2b1ad49dc231881`.

## End-to-end smoke result (profile M)

| Measurement | Result |
|---|---:|
| Local indexed bytes | 16,777,216 |
| Index chunks | 199 |
| SQLite bytes after checkpoint | 102,400 |
| Target chunks | 198 |
| Matched chunks | 192 |
| Locally reusable payload | 16,361,172 bytes |
| Missing payload staged | 481,581 bytes |
| Synthetic reuse ratio | 97.1407% |
| Rebuilt bytes | 16,842,753 |
| Final SHA-256 verified | Yes |

The rebuilt output used 16,361,172 bytes read from the indexed original file and 481,581 bytes from the test-only missing-chunk directory. Its SHA-256 exactly matched the descriptor.

## Profile comparison

| Profile | Min / Avg / Max | Source chunks | Target chunks | Reusable bytes | Missing bytes | Reuse ratio | SQLite bytes |
|---|---|---:|---:|---:|---:|---:|---:|
| S | 8 / 32 / 128 KiB | 386 | 387 | 16,507,448 | 335,305 | 98.0092% | 131,072 |
| M | 16 / 64 / 256 KiB | 199 | 198 | 16,361,172 | 481,581 | 97.1407% | 102,400 |
| L | 64 / 256 / 1024 KiB | 47 | 47 | 15,538,828 | 1,303,925 | 92.2582% | 65,536 |

This synthetic result shows the expected trade-off: smaller chunks reused more of the modified target but produced more index entries. It does not establish that profile S is best on real data.

## Prototype 0.2 loopback network result

Two independent `shardmeld` processes communicated over `127.0.0.1:45981`:

| Measurement | Result |
|---|---:|
| Missing chunk requests | 6 |
| Chunks served | 6 |
| Payload sent over peer connection | 481,581 bytes |
| Unavailable requests | 0 |
| Connection errors | 0 |
| Local bytes used during rebuild | 16,361,172 bytes |
| Network-fetched bytes used | 481,581 bytes |
| Rebuilt target size | 16,842,753 bytes |
| Final SHA-256 verified | Yes |

The readiness probe accounts for the server report showing two TCP connections. Only the second connection carried the six chunk requests.

## Automated verification

Thirty-seven automated tests passed, including:

- exact rebuild after insertion and modification;
- 100% identical-file reuse;
- stale-source rejection;
- cross-file reconstruction locations;
- descriptor/index profile mismatch rejection;
- corrupted missing-chunk rejection;
- output overwrite refusal;
- pruning removed source files during reindex.
- exact reconstruction under all three named chunk profiles.
- missing-chunk transfer over a real loopback TCP connection followed by exact rebuild;
- corrupted server chunk refusal;
- default refusal to listen on a non-loopback address.
- exact local reconstruction mapped to verified v1 BT pieces;
- mixed complete/incomplete BT-piece planning for a shifted target;
- rejection of a locally reconstructed piece with the wrong torrent SHA-1;
- explicit rejection of multi-file v1 metadata in the current single-file bridge.
- selective standard 16 KiB peer-wire requests followed by exact rebuild;
- corrupted standard peer data rejected without persisting an output.
- compact IPv4/IPv6 and dictionary tracker peer responses parsed.
- tracker failure reasons rejected.
- raw 20-byte tracker identifiers percent-encoded byte-for-byte.
- private tracker query strings redacted from persisted reports.
- automatic fallback from an unavailable first peer to a working second peer.
- BEP 12 tier parsing without changing the torrent info-hash.
- BEP 15 UDP connect/announce request and response validation.
- fallback from an unavailable HTTP tier to a working UDP tier and peer.
- concurrent rarest-first selection with asymmetric availability.
- automatic work shifting toward a faster Peer.
- stalled-Peer completion through an Endgame duplicate.
- fifteen standard CANCEL messages for a losing duplicated Piece.
- refusal to seed a corrupted complete source.
- exact standard block upload from a verified complete file.
- exact on-demand Piece reconstruction from separate indexed material files.

The first CDC implementation failed the shifted-file test at roughly 15.8% reuse because it retained old byte history with a rotate operation. The implementation was corrected to discard old high bits, after which the full suite and smoke flow passed. This is included as development evidence, not hidden as a successful first attempt.

## What this does not prove

- No personal directory was scanned. The only real inputs were explicitly
  downloaded public SQLite release files.
- The v0.2 network measurements used ShardMeld's minimal loopback research
  transport. The later v0.4-v0.6 experiments separately used qBittorrent and
  loopback trackers; none used DHT, magnet exchange, or public swarms.
- The v0.3 test proves standard v1 metainfo parsing and piece-hash compatibility,
  v0.4 proves direct interoperability with qBittorrent 5.0.5, and v0.5 proves
  HTTP tracker discovery plus sequential peer fallback, while v0.6 proves UDP
  tracker discovery and multitracker tier fallback.
- DHT discovery and magnet metadata exchange are still not implemented.
  Endgame is Piece-level rather than block-level. Upload is loopback-first and
  lacks production choking, automatic announce, and NAT traversal.
- `missing_payload` excludes future protocol overhead and retries.
- Neither the synthetic 97.1407% result nor the SQLite result may be
  generalized to arbitrary files.
- The ZIP negative control shows that compressed payloads may offer effectively
  no savings even when their decompressed versions are highly related.
- The Gear CDC implementation is an experimental baseline, not a proven optimal algorithm.

## Delivered binary

- Platform: macOS Apple Silicon (`arm64`).
- Version: `shardmeld 2.0.0`.
- Ad-hoc signed: yes.
- SHA-256: `3b1948c64ec67ba346e4d67fe48efadbcf0d48328a8608484d31e8e35c536894`.
