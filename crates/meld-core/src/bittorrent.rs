use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::{IndexDb, TargetDescriptor};

const MAX_BENCODE_DEPTH: usize = 128;
const MAX_BT_PIECE_LENGTH: u64 = 16 * 1024 * 1024;
const MAX_TRACKER_TIERS: usize = 32;
const MAX_TRACKERS_PER_TIER: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TorrentV1 {
    pub name: String,
    pub total_length: u64,
    pub piece_length: u64,
    pub piece_sha1: Vec<String>,
    pub info_hash_sha1: String,
    pub announce: Option<String>,
    pub announce_list: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackerResponse {
    pub interval: u64,
    pub peers: Vec<SocketAddr>,
    pub warning_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtPiecePlan {
    pub index: u64,
    pub offset: u64,
    pub length: u64,
    pub expected_sha1: String,
    pub locally_covered_bytes: u64,
    pub missing_bytes: u64,
    pub fully_local: bool,
    pub local_sha1_verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BtBridgeReport {
    pub torrent_name: String,
    pub descriptor_name: String,
    pub names_match: bool,
    pub info_hash_sha1: String,
    pub target_bytes: u64,
    pub piece_length: u64,
    pub total_pieces: u64,
    pub fully_local_pieces: u64,
    pub partially_local_pieces: u64,
    pub missing_pieces: u64,
    pub locally_covered_bytes: u64,
    pub missing_bytes: u64,
    pub fully_reconstructable_piece_bytes: u64,
    pub local_coverage_ratio: f64,
    pub fully_reconstructable_piece_ratio: f64,
    pub pieces: Vec<BtPiecePlan>,
}

pub fn load_v1_torrent(path: &Path) -> Result<TorrentV1> {
    let bytes = std::fs::read(path).with_context(|| format!("read torrent {}", path.display()))?;
    parse_v1_torrent(&bytes).with_context(|| format!("parse torrent {}", path.display()))
}

pub fn plan_v1_bridge(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
) -> Result<BtBridgeReport> {
    descriptor.validate()?;
    index.ensure_profile(descriptor.profile)?;
    if torrent.total_length != descriptor.target.size {
        bail!(
            "torrent/descriptor size mismatch: torrent has {} bytes, descriptor has {}",
            torrent.total_length,
            descriptor.target.size
        );
    }

    let mut pieces = Vec::with_capacity(torrent.piece_sha1.len());
    let mut locally_covered_bytes = 0_u64;
    let mut fully_reconstructable_piece_bytes = 0_u64;
    let mut fully_local_pieces = 0_u64;
    let mut partially_local_pieces = 0_u64;

    for (piece_index, expected_sha1) in torrent.piece_sha1.iter().enumerate() {
        let piece_offset = (piece_index as u64)
            .checked_mul(torrent.piece_length)
            .context("BT piece offset overflow")?;
        let piece_end = piece_offset
            .saturating_add(torrent.piece_length)
            .min(torrent.total_length);
        let piece_length = piece_end - piece_offset;
        let piece_buffer_length = usize::try_from(piece_length).context("BT piece is too large")?;
        let mut piece_bytes = vec![0_u8; piece_buffer_length];
        let mut covered = 0_u64;

        for chunk in &descriptor.chunks {
            let chunk_start = chunk.offset;
            let chunk_end = chunk.offset + u64::from(chunk.length);
            let overlap_start = chunk_start.max(piece_offset);
            let overlap_end = chunk_end.min(piece_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let overlap_length = overlap_end - overlap_start;
            if let Some(source) = index.lookup_chunk(&chunk.hash, chunk.length)? {
                let bytes =
                    read_verified_chunk(&source.path, source.offset, chunk.length, &chunk.hash)?;
                let source_start = (overlap_start - chunk_start) as usize;
                let destination_start = (overlap_start - piece_offset) as usize;
                let overlap_length = overlap_length as usize;
                piece_bytes[destination_start..destination_start + overlap_length]
                    .copy_from_slice(&bytes[source_start..source_start + overlap_length]);
                covered += overlap_length as u64;
            }
        }

        let missing = piece_length - covered;
        let fully_local = missing == 0;
        let local_sha1_verified = if fully_local {
            let actual_sha1 = hex::encode(Sha1::digest(&piece_bytes));
            if actual_sha1 != *expected_sha1 {
                bail!(
                    "locally reconstructed BT piece {piece_index} failed SHA-1: expected {expected_sha1}, got {actual_sha1}"
                );
            }
            fully_local_pieces += 1;
            fully_reconstructable_piece_bytes += piece_length;
            true
        } else {
            if covered > 0 {
                partially_local_pieces += 1;
            }
            false
        };
        locally_covered_bytes += covered;
        pieces.push(BtPiecePlan {
            index: piece_index as u64,
            offset: piece_offset,
            length: piece_length,
            expected_sha1: expected_sha1.clone(),
            locally_covered_bytes: covered,
            missing_bytes: missing,
            fully_local,
            local_sha1_verified,
        });
    }

    let total_pieces = pieces.len() as u64;
    let missing_pieces = total_pieces - fully_local_pieces;
    let missing_bytes = torrent.total_length - locally_covered_bytes;
    Ok(BtBridgeReport {
        torrent_name: torrent.name.clone(),
        descriptor_name: descriptor.target.name.clone(),
        names_match: torrent.name == descriptor.target.name,
        info_hash_sha1: torrent.info_hash_sha1.clone(),
        target_bytes: torrent.total_length,
        piece_length: torrent.piece_length,
        total_pieces,
        fully_local_pieces,
        partially_local_pieces,
        missing_pieces,
        locally_covered_bytes,
        missing_bytes,
        fully_reconstructable_piece_bytes,
        local_coverage_ratio: ratio(locally_covered_bytes, torrent.total_length),
        fully_reconstructable_piece_ratio: ratio(
            fully_reconstructable_piece_bytes,
            torrent.total_length,
        ),
        pieces,
    })
}

pub(crate) fn read_verified_chunk(
    path: &Path,
    offset: u64,
    length: u32,
    expected_hash: &str,
) -> Result<Vec<u8>> {
    let mut file =
        File::open(path).with_context(|| format!("open indexed source {}", path.display()))?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0_u8; length as usize];
    file.read_exact(&mut bytes)?;
    let actual_hash = blake3::hash(&bytes).to_hex().to_string();
    if actual_hash != expected_hash {
        bail!(
            "indexed chunk BLAKE3 mismatch in {}: expected {expected_hash}, got {actual_hash}",
            path.display()
        );
    }
    Ok(bytes)
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        1.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn parse_v1_torrent(bytes: &[u8]) -> Result<TorrentV1> {
    let mut parser = Parser::new(bytes);
    let root = parser.parse_node(0)?;
    if parser.position != bytes.len() {
        bail!("trailing bytes after bencoded torrent metadata");
    }
    let root_dictionary = root.as_dictionary("torrent root")?;
    let info =
        dictionary_value(root_dictionary, b"info").context("torrent has no info dictionary")?;
    let info_dictionary = info.as_dictionary("info")?;
    if dictionary_value(info_dictionary, b"files").is_some() {
        bail!("multi-file v1 torrents are not supported by the v0.3 bridge");
    }
    if dictionary_value(info_dictionary, b"meta version").is_some() {
        bail!("BitTorrent v2/hybrid metadata is not supported by the v0.3 bridge");
    }

    let name = node_utf8(
        dictionary_value(info_dictionary, b"name").context("info.name is missing")?,
        "info.name",
    )?;
    let total_length = node_u64(
        dictionary_value(info_dictionary, b"length").context("info.length is missing")?,
        "info.length",
    )?;
    let piece_length = node_u64(
        dictionary_value(info_dictionary, b"piece length")
            .context("info.piece length is missing")?,
        "info.piece length",
    )?;
    if piece_length == 0 {
        bail!("info.piece length must be greater than zero");
    }
    if piece_length > MAX_BT_PIECE_LENGTH {
        bail!(
            "info.piece length {piece_length} exceeds the v0.3 safety limit of {MAX_BT_PIECE_LENGTH} bytes"
        );
    }
    let pieces = dictionary_value(info_dictionary, b"pieces")
        .context("info.pieces is missing")?
        .as_bytes("info.pieces")?;
    if pieces.len() % 20 != 0 {
        bail!("info.pieces length must be a multiple of 20 bytes");
    }
    let expected_piece_count = if total_length == 0 {
        0
    } else {
        total_length.div_ceil(piece_length)
    };
    let actual_piece_count = u64::try_from(pieces.len() / 20).context("too many BT pieces")?;
    if actual_piece_count != expected_piece_count {
        bail!(
            "info.pieces count mismatch: expected {expected_piece_count}, got {actual_piece_count}",
        );
    }
    let piece_sha1 = pieces.chunks_exact(20).map(hex::encode).collect();
    let announce = dictionary_value(root_dictionary, b"announce")
        .map(|node| node_utf8(node, "announce"))
        .transpose()?;
    let announce_list = dictionary_value(root_dictionary, b"announce-list")
        .map(parse_announce_list)
        .transpose()?;

    Ok(TorrentV1 {
        name,
        total_length,
        piece_length,
        piece_sha1,
        info_hash_sha1: hex::encode(Sha1::digest(&bytes[info.start..info.end])),
        announce,
        announce_list,
    })
}

fn parse_announce_list(node: &Node<'_>) -> Result<Vec<Vec<String>>> {
    let Value::List(tiers) = &node.value else {
        bail!("announce-list must be a list of tiers");
    };
    if tiers.len() > MAX_TRACKER_TIERS {
        bail!("announce-list exceeds the safety limit of {MAX_TRACKER_TIERS} tiers");
    }
    let mut parsed = Vec::with_capacity(tiers.len());
    for (tier_index, tier) in tiers.iter().enumerate() {
        let Value::List(trackers) = &tier.value else {
            bail!("announce-list tier {tier_index} must be a list");
        };
        if trackers.len() > MAX_TRACKERS_PER_TIER {
            bail!(
                "announce-list tier {tier_index} exceeds the safety limit of {MAX_TRACKERS_PER_TIER} trackers"
            );
        }
        let mut urls = Vec::with_capacity(trackers.len());
        let mut seen = HashSet::new();
        for (tracker_index, tracker) in trackers.iter().enumerate() {
            let url = node_utf8(
                tracker,
                &format!("announce-list[{tier_index}][{tracker_index}]"),
            )?;
            if url.is_empty() {
                bail!("announce-list contains an empty tracker URL");
            }
            if seen.insert(url.clone()) {
                urls.push(url);
            }
        }
        if !urls.is_empty() {
            parsed.push(urls);
        }
    }
    Ok(parsed)
}

pub fn parse_tracker_response(bytes: &[u8]) -> Result<TrackerResponse> {
    let mut parser = Parser::new(bytes);
    let root = parser.parse_node(0)?;
    if parser.position != bytes.len() {
        bail!("trailing bytes after bencoded tracker response");
    }
    let dictionary = root.as_dictionary("tracker response")?;
    if let Some(failure) = dictionary_value(dictionary, b"failure reason") {
        bail!("tracker failure: {}", node_utf8(failure, "failure reason")?);
    }
    let interval = dictionary_value(dictionary, b"interval")
        .map(|node| node_u64(node, "interval"))
        .transpose()?
        .unwrap_or(0);
    let warning_message = dictionary_value(dictionary, b"warning message")
        .map(|node| node_utf8(node, "warning message"))
        .transpose()?;

    let mut peers = Vec::new();
    if let Some(node) = dictionary_value(dictionary, b"peers") {
        match &node.value {
            Value::Bytes(compact) => parse_compact_ipv4_peers(compact, &mut peers)?,
            Value::List(entries) => parse_dictionary_peers(entries, &mut peers)?,
            _ => bail!("tracker peers must be a compact byte string or a list"),
        }
    }
    if let Some(node) = dictionary_value(dictionary, b"peers6") {
        parse_compact_ipv6_peers(node.as_bytes("peers6")?, &mut peers)?;
    }
    let mut seen = HashSet::new();
    peers.retain(|peer| seen.insert(*peer));
    Ok(TrackerResponse {
        interval,
        peers,
        warning_message,
    })
}

fn parse_compact_ipv4_peers(bytes: &[u8], peers: &mut Vec<SocketAddr>) -> Result<()> {
    if !bytes.len().is_multiple_of(6) {
        bail!("compact IPv4 peer list length must be a multiple of 6");
    }
    for peer in bytes.chunks_exact(6) {
        let port = u16::from_be_bytes([peer[4], peer[5]]);
        if port != 0 {
            peers.push(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3])),
                port,
            ));
        }
    }
    Ok(())
}

fn parse_compact_ipv6_peers(bytes: &[u8], peers: &mut Vec<SocketAddr>) -> Result<()> {
    if !bytes.len().is_multiple_of(18) {
        bail!("compact IPv6 peer list length must be a multiple of 18");
    }
    for peer in bytes.chunks_exact(18) {
        let address = <[u8; 16]>::try_from(&peer[..16]).expect("exact 16-byte IPv6 chunk");
        let port = u16::from_be_bytes([peer[16], peer[17]]);
        if port != 0 {
            peers.push(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(address)), port));
        }
    }
    Ok(())
}

fn parse_dictionary_peers(entries: &[Node<'_>], peers: &mut Vec<SocketAddr>) -> Result<()> {
    for (index, node) in entries.iter().enumerate() {
        let dictionary = node.as_dictionary("tracker peer")?;
        let ip = node_utf8(
            dictionary_value(dictionary, b"ip")
                .with_context(|| format!("tracker peer {index} has no ip"))?,
            "peer.ip",
        )?;
        let ip: IpAddr = ip
            .parse()
            .with_context(|| format!("tracker peer {index} has invalid IP address {ip}"))?;
        let port = node_u64(
            dictionary_value(dictionary, b"port")
                .with_context(|| format!("tracker peer {index} has no port"))?,
            "peer.port",
        )?;
        let port = u16::try_from(port)
            .with_context(|| format!("tracker peer {index} port is out of range"))?;
        if port != 0 {
            peers.push(SocketAddr::new(ip, port));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Node<'a> {
    value: Value<'a>,
    start: usize,
    end: usize,
}

#[derive(Debug)]
enum Value<'a> {
    Integer(i64),
    Bytes(&'a [u8]),
    List(Vec<Node<'a>>),
    Dictionary(Vec<(&'a [u8], Node<'a>)>),
}

impl<'a> Node<'a> {
    fn as_dictionary(&'a self, label: &str) -> Result<&'a [(&'a [u8], Node<'a>)]> {
        match &self.value {
            Value::Dictionary(entries) => Ok(entries),
            _ => bail!("{label} must be a dictionary"),
        }
    }

    fn as_bytes(&'a self, label: &str) -> Result<&'a [u8]> {
        match &self.value {
            Value::Bytes(bytes) => Ok(bytes),
            _ => bail!("{label} must be a byte string"),
        }
    }
}

fn dictionary_value<'a>(
    dictionary: &'a [(&'a [u8], Node<'a>)],
    key: &[u8],
) -> Option<&'a Node<'a>> {
    dictionary
        .iter()
        .find_map(|(candidate, value)| (*candidate == key).then_some(value))
}

fn node_utf8(node: &Node<'_>, label: &str) -> Result<String> {
    let bytes = node.as_bytes(label)?;
    Ok(std::str::from_utf8(bytes)
        .with_context(|| format!("{label} is not UTF-8"))?
        .to_owned())
}

fn node_u64(node: &Node<'_>, label: &str) -> Result<u64> {
    match node.value {
        Value::Integer(value) => {
            u64::try_from(value).with_context(|| format!("{label} is negative"))
        }
        _ => bail!("{label} must be an integer"),
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn parse_node(&mut self, depth: usize) -> Result<Node<'a>> {
        if depth > MAX_BENCODE_DEPTH {
            bail!("bencode nesting exceeds {MAX_BENCODE_DEPTH}");
        }
        let start = self.position;
        let marker = *self
            .bytes
            .get(self.position)
            .context("unexpected end of bencode")?;
        let value = match marker {
            b'i' => self.parse_integer()?,
            b'l' => self.parse_list(depth)?,
            b'd' => self.parse_dictionary(depth)?,
            b'0'..=b'9' => self.parse_bytes()?,
            _ => bail!("invalid bencode marker 0x{marker:02x} at byte {start}"),
        };
        Ok(Node {
            value,
            start,
            end: self.position,
        })
    }

    fn parse_integer(&mut self) -> Result<Value<'a>> {
        self.position += 1;
        let start = self.position;
        let end = self.find_byte(b'e')?;
        let raw = &self.bytes[start..end];
        validate_integer(raw)?;
        let text = std::str::from_utf8(raw)?;
        let value = text.parse::<i64>().context("bencode integer overflow")?;
        self.position = end + 1;
        Ok(Value::Integer(value))
    }

    fn parse_bytes(&mut self) -> Result<Value<'a>> {
        let length_start = self.position;
        let colon = self.find_byte(b':')?;
        let raw_length = &self.bytes[length_start..colon];
        if raw_length.len() > 1 && raw_length[0] == b'0' {
            bail!("bencode byte-string length has a leading zero");
        }
        let length = std::str::from_utf8(raw_length)?
            .parse::<usize>()
            .context("bencode byte-string length overflow")?;
        let data_start = colon + 1;
        let data_end = data_start
            .checked_add(length)
            .context("bencode byte-string length overflow")?;
        if data_end > self.bytes.len() {
            bail!("bencode byte string extends past end of input");
        }
        self.position = data_end;
        Ok(Value::Bytes(&self.bytes[data_start..data_end]))
    }

    fn parse_list(&mut self, depth: usize) -> Result<Value<'a>> {
        self.position += 1;
        let mut entries = Vec::new();
        while self.bytes.get(self.position) != Some(&b'e') {
            entries.push(self.parse_node(depth + 1)?);
        }
        self.position += 1;
        Ok(Value::List(entries))
    }

    fn parse_dictionary(&mut self, depth: usize) -> Result<Value<'a>> {
        self.position += 1;
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        while self.bytes.get(self.position) != Some(&b'e') {
            let key = match self.parse_node(depth + 1)?.value {
                Value::Bytes(key) => key,
                _ => bail!("bencode dictionary key must be a byte string"),
            };
            if !seen.insert(key) {
                bail!("duplicate bencode dictionary key");
            }
            let value = self.parse_node(depth + 1)?;
            entries.push((key, value));
        }
        self.position += 1;
        Ok(Value::Dictionary(entries))
    }

    fn find_byte(&self, needle: u8) -> Result<usize> {
        self.bytes[self.position..]
            .iter()
            .position(|byte| *byte == needle)
            .map(|relative| self.position + relative)
            .with_context(|| format!("unterminated bencode value, missing byte 0x{needle:02x}"))
    }
}

fn validate_integer(raw: &[u8]) -> Result<()> {
    if raw.is_empty() {
        bail!("empty bencode integer");
    }
    if raw == b"-0" || (raw.len() > 1 && raw[0] == b'0') {
        bail!("non-canonical bencode integer");
    }
    let digits = if raw[0] == b'-' {
        if raw.len() == 1 || raw[1] == b'0' {
            bail!("non-canonical bencode integer");
        }
        &raw[1..]
    } else {
        raw
    };
    if !digits.iter().all(u8::is_ascii_digit) {
        bail!("invalid bencode integer");
    }
    Ok(())
}
