# ShardMeld 1.2 file-seed to qBittorrent verification

Date: 2026-08-31  
ShardMeld: 1.2.0  
External receiver: qBittorrent 5.0.5

## Result

ShardMeld verified the complete source against the `.meld` SHA-256 and every
v1 torrent Piece SHA-1 before listening. An unchanged qBittorrent process then
discovered it through the loopback experiment tracker and downloaded the full
file through standard BitTorrent peer-wire requests.

| Measurement | Result |
|---|---:|
| Advertised Pieces | 37 |
| Successful handshakes | 1 |
| Standard block requests | 581 |
| Payload uploaded | 9,515,341 bytes |
| Protocol errors | 0 |
| Download SHA-256 | `b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| Byte-identical | Yes |

This proves verified full-file upload interoperability with qBittorrent. It
does not prove DHT, public-swarm behavior, production choking, NAT traversal,
or indexed reconstruction seeding.

## Evidence

- `seed-report.json`: ShardMeld upload counters and verified identity.
- `downloaded-by-qbittorrent.bin`: retained byte-identical receiver output.
