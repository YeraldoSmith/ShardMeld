# Prototype 0.1 through ShardMeld 2.0

## Scope

Prototype 0.1 measures local byte reuse and proves exact reconstruction.
Prototype 0.2 adds a deliberately minimal ShardMeld peer protocol. Prototype
0.3 parses v1 single-file torrent metadata and proves the
local-CDC-to-BT-piece mapping. Prototype 0.4 implements the minimum standard TCP
peer-wire path and has interoperated with qBittorrent 5.0.5. Prototype 0.5 adds
HTTP(S) tracker announces, BEP 23 compact peer parsing, traditional dictionary
peer parsing, and sequential failover across discovered peers.
Prototype 0.6 parses BEP 12 `announce-list` tiers, shuffles trackers within a
tier for each session, tries tiers in order, and implements the BEP 15 UDP
connect/announce binary protocol for IPv4 and IPv6 tracker responses.
Prototype 0.7 adds a 16-request per-peer block pipeline, accepts PIECE messages
out of request order, and persists only SHA-1-verified whole Pieces for restart
and peer failover. Prototype 0.8 adds a shared Piece-job queue for up to four
concurrent peers, availability-aware assignment, five-second no-message
eviction, and failed-job reassignment. Prototype 0.9 adds a per-batch
availability-registration barrier, rarest-first Piece selection, work-conserving
speed adaptation, and per-peer Piece/throughput evidence. Prototype 0.10 adds
Piece-level Endgame duplication with private verification buffers, a
single-winner commit, and standard CANCEL messages.

ShardMeld 1.0 keeps that verified engine and freezes `shardmeld-report` version
1 for BitTorrent transfer reports. The `capabilities` command exposes the
implemented, deferred, and bounded feature sets without requiring README text
to be parsed.

ShardMeld 1.1 accepts standard v1 magnet URIs with hexadecimal or base32
`urn:btih`, deduplicates and validates HTTP(S)/UDP tracker parameters, verifies
that local `.torrent` metadata has the exact same info hash, then uses the
normal Tracker and Peer engine. BEP 9 metadata exchange is not implied.

ShardMeld 1.2 adds a standard v1 upload path for complete files. The source
must match the descriptor SHA-256 and every torrent Piece SHA-1 before the
listener starts. ShardMeld 2.0 adds indexed upload: preflight determines which
Pieces can be reconstructed and verified from authorized CDC locations, the
bitfield advertises only those Pieces, and requests are served by reconstructing
the Piece in memory. The complete target file need not exist in the index.

## Metrics

```text
reuse_ratio = unique matched target bytes / target bytes
missing_payload = target bytes - unique matched target bytes
index_overhead = SQLite database bytes / indexed source bytes
```

`missing_payload` is content payload, not measured BitTorrent traffic. Protocol overhead, retries, peer discovery, and duplicate requests are intentionally excluded until the BT phase.

## Descriptor

`.meld` is JSON and contains:

- format and algorithm version;
- CDC profile;
- target name, length, and SHA-256;
- ordered BLAKE3-256 chunk hashes, offsets, and lengths.

The descriptor is not yet signed and must not be treated as publisher-authenticated metadata.

## Known limitations

- Paths are currently stored as UTF-8 text; non-UTF-8 filesystem paths are not supported.
- The local SQLite index and JSON reports can reveal local file paths and should not be uploaded or shared without sanitization.
- Source freshness is checked using size and modification time, then chunk content is verified again during rebuild.
- The CDC implementation is an experimental deterministic Gear chunker, not a claimed optimal chunking algorithm.
- Existing v1 torrents do not contain this CDC map. Full fine-grained reuse will require a sidecar or compatible extension.
- `bt-plan` supports v1 single-file torrents only; multi-file, v2, and hybrid
  torrents are rejected explicitly.
- `bt-fetch-magnet` requires trusted local `.torrent` metadata. It does not yet
  fetch metadata from peers, use DHT, or accept v2 `btmh` magnets.
- The v2.0 upload server is a correctness-first seed. It handles connections
  serially, has no production choking or tit-for-tat policy, and does not
  announce itself to a tracker automatically.
- Indexed seeding performs a full preflight read of locally covered bytes and
  currently keeps only the most recently reconstructed Piece in memory. It may
  reread overlapping CDC chunks and is not yet I/O optimized.
- A BT piece is advertised as locally complete only after its reconstructed
  bytes pass the torrent's SHA-1. Partially local pieces remain incomplete until
  their missing ranges are acquired.
- If a piece is only partially local, its torrent identity cannot be verified
  until the missing bytes arrive. The unsigned `.meld` sidecar is therefore not
  publisher-authenticated metadata.
- `bt-fetch-peer` sends standard 16 KiB-aligned requests. If only part of such a
  block is missing, the full block is transferred and the report counts the
  redundant bytes explicitly.
- `bt-fetch-tracker` supports HTTP, HTTPS, and base BEP 15 UDP trackers. BEP 41
  UDP path/query extensions, tracker authentication policy, and DHT are not yet
  implemented. UDP announces currently make one 15-second attempt per address;
  the full exponential retransmission schedule is deferred.
- v0.10 starts discovered peers in batches of at most four. Each active peer
  receives one compatible whole-Piece job at a time and may keep 16 block
  requests in flight for that Piece. A failed job is retried by another peer.
  There is no long-lived throughput reputation or retry backoff.
- v0.9 chooses the rarest currently queued Piece within the registered batch.
  Equal-rarity ties use Piece index. Faster peers claim more work by completing
  jobs sooner; there is not yet a long-lived reputation score across sessions.
  The registration barrier can delay the batch until a non-responsive peer's
  five-second handshake/message timeout expires.
- v0.10 Endgame begins only after the ordinary Piece queue is empty. At most
  two workers may claim one active Piece. They write only to private buffers,
  verify SHA-1 independently, and enter a serialized single-winner commit.
  Losers CANCEL remaining requests when they observe completion. Very fast,
  symmetric peers can both finish before either observes the winner; the
  second result is discarded safely but its redundant payload remains counted.
- Resume state is deliberately Piece-granular: partially received Pieces are
  requested again after interruption. A Piece marked complete is rehashed from
  the adjacent partial file before it is trusted, so damaged state cannot skip
  verification.
- The experiment-only local HTTP tracker binds to loopback and exists solely to
  prove discovery with an unchanged external BitTorrent client; it is not a
  production tracker.
- One adjacent-version SQLite benchmark establishes behavior for that specific
  public sample only. It does not establish savings on arbitrary real data.
- Separately compressed versions can have near-zero byte reuse even when their
  extracted content is strongly related; archive-aware reconstruction is not
  implemented.
- The v0.2 peer protocol has integrity checking but no encryption, authentication, discovery, NAT traversal, congestion strategy, or swarm scheduling.
- The chunk server binds to loopback by default. Non-loopback exposure requires an explicit flag and is only suitable for a trusted test network.
