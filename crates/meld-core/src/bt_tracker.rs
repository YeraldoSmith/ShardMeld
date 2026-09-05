use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use url::Url;

use crate::bt_peer::{fetch_v1_from_peers_with_peer_id, generate_peer_id};
use crate::{
    BtPeerFetchReport, IndexDb, REPORT_FORMAT, REPORT_VERSION, TargetDescriptor, TorrentV1,
    TrackerResponse, parse_tracker_response, plan_v1_bridge,
};

const MAX_TRACKER_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TRACKER_PEERS: usize = 100;
const UDP_PROTOCOL_ID: u64 = 0x0417_2710_1980;
const UDP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_UDP_RESPONSE_BYTES: usize = 65_507;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtTrackerAttempt {
    pub peer: SocketAddr,
    pub connected: bool,
    pub remote_peer_id_hex: Option<String>,
    pub pieces_verified: u64,
    pub verified_piece_indices: Vec<u32>,
    pub block_requests_sent: u64,
    pub payload_bytes_received: u64,
    pub cancel_messages_sent: u64,
    pub endgame_jobs_cancelled: u64,
    pub active_transfer_micros: u64,
    pub payload_bytes_per_second: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtDiscoveryAttempt {
    pub tier: u64,
    pub tracker: String,
    pub peers_returned: u64,
    pub interval_seconds: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtTrackerLifecycleAttempt {
    pub event: String,
    pub tracker: String,
    pub success: bool,
    pub interval_seconds: Option<u64>,
    pub warning_message: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtTrackerFetchReport {
    pub report_format: String,
    pub report_version: u32,
    pub engine_version: String,
    pub tracker: String,
    pub tracker_interval_seconds: u64,
    pub tracker_warning: Option<String>,
    pub tracker_attempts: Vec<BtDiscoveryAttempt>,
    pub peers_discovered: u64,
    pub peers_attempted: Vec<BtTrackerAttempt>,
    pub selected_peer: SocketAddr,
    pub transfer: BtPeerFetchReport,
    pub output: PathBuf,
    pub verified: bool,
}

#[derive(Debug)]
struct PeerCandidate {
    peer: SocketAddr,
    tracker: String,
    interval: u64,
    warning: Option<String>,
}

pub fn fetch_v1_via_tracker(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    tracker_override: Option<&str>,
    announce_port: u16,
    output: &Path,
) -> Result<BtTrackerFetchReport> {
    let tiers = tracker_tiers(torrent, tracker_override)?;
    let bridge = plan_v1_bridge(torrent, descriptor, index)?;
    let peer_id = generate_peer_id()?;
    let key = tracker_key(&peer_id);
    let mut tracker_attempts = Vec::new();
    let mut peer_attempts = Vec::new();
    let mut discovered_peers = HashSet::new();
    let mut tried_peers = HashSet::new();

    for (tier_index, tier) in tiers.into_iter().enumerate() {
        let mut candidates = Vec::new();
        let mut active_trackers = Vec::new();
        for tracker in shuffled_tier(tier, &peer_id, tier_index) {
            let display = redact_tracker_url(&tracker);
            match announce(
                &tracker,
                torrent,
                &peer_id,
                key,
                announce_port,
                TrackerCounters::new(0, bridge.missing_bytes),
                TrackerEvent::Started,
            ) {
                Ok(response) if response.peers.len() <= MAX_TRACKER_PEERS => {
                    tracker_attempts.push(BtDiscoveryAttempt {
                        tier: tier_index as u64,
                        tracker: display.clone(),
                        peers_returned: response.peers.len() as u64,
                        interval_seconds: Some(response.interval),
                        error: None,
                    });
                    active_trackers.push(tracker.clone());
                    for peer in response.peers {
                        discovered_peers.insert(peer);
                        candidates.push(PeerCandidate {
                            peer,
                            tracker: display.clone(),
                            interval: response.interval,
                            warning: response.warning_message.clone(),
                        });
                    }
                }
                Ok(response) => {
                    active_trackers.push(tracker.clone());
                    tracker_attempts.push(BtDiscoveryAttempt {
                        tier: tier_index as u64,
                        tracker: display,
                        peers_returned: response.peers.len() as u64,
                        interval_seconds: Some(response.interval),
                        error: Some(format!(
                            "tracker peer count exceeds the safety limit of {MAX_TRACKER_PEERS}"
                        )),
                    });
                }
                Err(error) => tracker_attempts.push(BtDiscoveryAttempt {
                    tier: tier_index as u64,
                    tracker: display,
                    peers_returned: 0,
                    interval_seconds: None,
                    error: Some(format!("{error:#}")),
                }),
            }
        }

        let candidates = candidates
            .into_iter()
            .filter(|candidate| tried_peers.insert(candidate.peer))
            .collect::<Vec<_>>();
        let candidate_peers = candidates
            .iter()
            .map(|candidate| candidate.peer)
            .collect::<Vec<_>>();
        let mut workers = Vec::new();
        match fetch_v1_from_peers_with_peer_id(
            torrent,
            descriptor,
            index,
            &candidate_peers,
            output,
            &peer_id,
            &mut workers,
        ) {
            Ok(transfer) => {
                peer_attempts.extend(workers.into_iter().map(|worker| {
                    let connected = worker.remote_peer_id_hex.is_some();
                    BtTrackerAttempt {
                        peer: worker.peer,
                        connected,
                        remote_peer_id_hex: worker.remote_peer_id_hex,
                        pieces_verified: worker.pieces_verified,
                        verified_piece_indices: worker.verified_piece_indices,
                        block_requests_sent: worker.block_requests_sent,
                        payload_bytes_received: worker.payload_bytes_received,
                        cancel_messages_sent: worker.cancel_messages_sent,
                        endgame_jobs_cancelled: worker.endgame_jobs_cancelled,
                        active_transfer_micros: worker.active_transfer_micros,
                        payload_bytes_per_second: worker.payload_bytes_per_second,
                        error: worker.error,
                    }
                }));
                let selected = candidates
                    .iter()
                    .find(|candidate| candidate.peer == transfer.peer)
                    .or_else(|| candidates.first())
                    .context("successful BT transfer had no tracker candidate")?;
                stop_trackers(&active_trackers, torrent, &peer_id, key, announce_port);
                return Ok(BtTrackerFetchReport {
                    report_format: REPORT_FORMAT.to_owned(),
                    report_version: REPORT_VERSION,
                    engine_version: env!("CARGO_PKG_VERSION").to_owned(),
                    tracker: selected.tracker.clone(),
                    tracker_interval_seconds: selected.interval,
                    tracker_warning: selected.warning.clone(),
                    tracker_attempts,
                    peers_discovered: discovered_peers.len() as u64,
                    peers_attempted: peer_attempts,
                    selected_peer: transfer.peer,
                    output: output.to_path_buf(),
                    verified: transfer.verified,
                    transfer,
                });
            }
            Err(error) => {
                peer_attempts.extend(workers.into_iter().map(|worker| {
                    let connected = worker.remote_peer_id_hex.is_some();
                    BtTrackerAttempt {
                        peer: worker.peer,
                        connected,
                        remote_peer_id_hex: worker.remote_peer_id_hex,
                        pieces_verified: worker.pieces_verified,
                        verified_piece_indices: worker.verified_piece_indices,
                        block_requests_sent: worker.block_requests_sent,
                        payload_bytes_received: worker.payload_bytes_received,
                        cancel_messages_sent: worker.cancel_messages_sent,
                        endgame_jobs_cancelled: worker.endgame_jobs_cancelled,
                        active_transfer_micros: worker.active_transfer_micros,
                        payload_bytes_per_second: worker.payload_bytes_per_second,
                        error: worker
                            .error
                            .or_else(|| Some(format!("transfer incomplete: {error:#}"))),
                    }
                }));
            }
        }
        stop_trackers(&active_trackers, torrent, &peer_id, key, announce_port);
    }

    let tracker_failures = tracker_attempts
        .iter()
        .filter_map(|attempt| {
            attempt
                .error
                .as_deref()
                .map(|error| format!("{}: {error}", attempt.tracker))
        })
        .collect::<Vec<_>>()
        .join("; ");
    let peer_failures = peer_attempts
        .iter()
        .filter_map(|attempt| {
            attempt
                .error
                .as_deref()
                .map(|error| format!("{}: {error}", attempt.peer))
        })
        .collect::<Vec<_>>()
        .join("; ");
    bail!(
        "tracker discovery exhausted without a working peer; tracker_failures=[{tracker_failures}] peer_failures=[{peer_failures}]"
    )
}

fn tracker_tiers(torrent: &TorrentV1, tracker_override: Option<&str>) -> Result<Vec<Vec<String>>> {
    let tiers = if let Some(tracker) = tracker_override {
        vec![vec![tracker.to_owned()]]
    } else if let Some(announce_list) = &torrent.announce_list {
        announce_list.clone()
    } else if let Some(announce) = &torrent.announce {
        vec![vec![announce.clone()]]
    } else {
        Vec::new()
    };
    if tiers.is_empty() || tiers.iter().all(Vec::is_empty) {
        bail!("torrent has no usable tracker URL; provide --tracker");
    }
    Ok(tiers)
}

pub(crate) fn start_seed_trackers(
    torrent: &TorrentV1,
    peer_id: &[u8; 20],
    port: u16,
) -> (Vec<String>, Vec<BtTrackerLifecycleAttempt>) {
    let tiers = if let Some(announce_list) = &torrent.announce_list {
        announce_list.clone()
    } else if let Some(announce) = &torrent.announce {
        vec![vec![announce.clone()]]
    } else {
        Vec::new()
    };
    let key = tracker_key(peer_id);
    let mut active = Vec::new();
    let mut attempts = Vec::new();
    let mut seen = HashSet::new();
    for (tier_index, tier) in tiers.into_iter().enumerate() {
        for tracker in shuffled_tier(tier, peer_id, tier_index) {
            if !seen.insert(tracker.clone()) {
                continue;
            }
            let display = redact_tracker_url(&tracker);
            match announce(
                &tracker,
                torrent,
                peer_id,
                key,
                port,
                TrackerCounters::new(0, 0),
                TrackerEvent::Started,
            ) {
                Ok(response) => {
                    attempts.push(BtTrackerLifecycleAttempt {
                        event: TrackerEvent::Started.http_name().to_owned(),
                        tracker: display,
                        success: true,
                        interval_seconds: Some(response.interval),
                        warning_message: response.warning_message,
                        error: None,
                    });
                    active.push(tracker);
                    break;
                }
                Err(error) => attempts.push(BtTrackerLifecycleAttempt {
                    event: TrackerEvent::Started.http_name().to_owned(),
                    tracker: display,
                    success: false,
                    interval_seconds: None,
                    warning_message: None,
                    error: Some(format!("{error:#}")),
                }),
            }
        }
    }
    (active, attempts)
}

pub(crate) fn stop_seed_trackers(
    trackers: &[String],
    torrent: &TorrentV1,
    peer_id: &[u8; 20],
    port: u16,
    uploaded: u64,
) -> Vec<BtTrackerLifecycleAttempt> {
    let key = tracker_key(peer_id);
    trackers
        .iter()
        .map(|tracker| {
            let display = redact_tracker_url(tracker);
            match announce(
                tracker,
                torrent,
                peer_id,
                key,
                port,
                TrackerCounters::new(uploaded, 0),
                TrackerEvent::Stopped,
            ) {
                Ok(response) => BtTrackerLifecycleAttempt {
                    event: TrackerEvent::Stopped.http_name().to_owned(),
                    tracker: display,
                    success: true,
                    interval_seconds: Some(response.interval),
                    warning_message: response.warning_message,
                    error: None,
                },
                Err(error) => BtTrackerLifecycleAttempt {
                    event: TrackerEvent::Stopped.http_name().to_owned(),
                    tracker: display,
                    success: false,
                    interval_seconds: None,
                    warning_message: None,
                    error: Some(format!("{error:#}")),
                },
            }
        })
        .collect()
}

fn shuffled_tier(mut tier: Vec<String>, peer_id: &[u8; 20], tier_index: usize) -> Vec<String> {
    let mut digest = Sha1::new();
    digest.update(peer_id);
    digest.update(tier_index.to_be_bytes());
    let hash = digest.finalize();
    let mut state = u64::from_be_bytes(hash[..8].try_into().expect("SHA-1 has eight bytes"));
    for index in (1..tier.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        tier.swap(index, state as usize % (index + 1));
    }
    tier
}

fn stop_trackers(
    trackers: &[String],
    torrent: &TorrentV1,
    peer_id: &[u8; 20],
    key: u32,
    port: u16,
) {
    for tracker in trackers {
        let _ = announce(
            tracker,
            torrent,
            peer_id,
            key,
            port,
            TrackerCounters::new(0, 0),
            TrackerEvent::Stopped,
        );
    }
}

#[derive(Debug, Clone, Copy)]
enum TrackerEvent {
    Started,
    Stopped,
}

#[derive(Debug, Clone, Copy)]
struct TrackerCounters {
    uploaded: u64,
    left: u64,
}

impl TrackerCounters {
    const fn new(uploaded: u64, left: u64) -> Self {
        Self { uploaded, left }
    }
}

impl TrackerEvent {
    fn http_name(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Stopped => "stopped",
        }
    }

    fn udp_value(self) -> u32 {
        match self {
            Self::Started => 2,
            Self::Stopped => 3,
        }
    }
}

fn announce(
    tracker: &str,
    torrent: &TorrentV1,
    peer_id: &[u8; 20],
    key: u32,
    port: u16,
    counters: TrackerCounters,
    event: TrackerEvent,
) -> Result<TrackerResponse> {
    let scheme = tracker
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .context("tracker URL has no scheme")?;
    match scheme.as_str() {
        "http" | "https" => announce_http(tracker, torrent, peer_id, key, port, counters, event),
        "udp" => announce_udp(tracker, torrent, peer_id, key, port, counters, event),
        _ => bail!(
            "unsupported tracker scheme {scheme}; ShardMeld 1.1 supports HTTP, HTTPS, and UDP trackers"
        ),
    }
}

fn announce_http(
    tracker: &str,
    torrent: &TorrentV1,
    peer_id: &[u8; 20],
    key: u32,
    port: u16,
    counters: TrackerCounters,
    event: TrackerEvent,
) -> Result<TrackerResponse> {
    if tracker.contains('#') {
        bail!("tracker URL fragments are not supported");
    }
    let info_hash = decode_info_hash(torrent)?;
    let separator = if tracker.contains('?') { '&' } else { '?' };
    let url = format!(
        "{tracker}{separator}info_hash={}&peer_id={}&port={port}&uploaded={}&downloaded=0&left={}&compact=1&numwant=50&key={key}&event={}",
        percent_encode_bytes(&info_hash),
        percent_encode_bytes(peer_id),
        counters.uploaded,
        counters.left,
        event.http_name(),
    );
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .new_agent();
    let mut response = agent
        .get(&url)
        .header("User-Agent", "ShardMeld/2.0.0")
        .call()
        .with_context(|| format!("HTTP tracker request {}", redact_tracker_url(tracker)))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_TRACKER_RESPONSE_BYTES)
        .read_to_vec()
        .context("read tracker response body")?;
    parse_tracker_response(&body)
}

fn announce_udp(
    tracker: &str,
    torrent: &TorrentV1,
    peer_id: &[u8; 20],
    key: u32,
    port: u16,
    counters: TrackerCounters,
    event: TrackerEvent,
) -> Result<TrackerResponse> {
    let parsed = Url::parse(tracker).context("parse UDP tracker URL")?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("UDP tracker URL credentials require an unsupported extension");
    }
    if parsed.query().is_some() {
        bail!("UDP tracker query extensions are not supported by ShardMeld 1.1");
    }
    if parsed.fragment().is_some() {
        bail!("UDP tracker URL fragments are not supported");
    }
    if !matches!(parsed.path(), "" | "/" | "/announce") {
        bail!("UDP tracker paths other than /announce require BEP 41 extensions");
    }
    let host = parsed.host_str().context("UDP tracker URL has no host")?;
    let tracker_port = parsed.port().context("UDP tracker URL has no port")?;
    let addresses = (host, tracker_port)
        .to_socket_addrs()
        .with_context(|| format!("resolve UDP tracker host {host}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        bail!("UDP tracker host {host} resolved to no addresses");
    }
    let mut failures = Vec::new();
    for address in addresses {
        match announce_udp_address(address, torrent, peer_id, key, port, counters, event) {
            Ok(response) => return Ok(response),
            Err(error) => failures.push(format!("{address}: {error:#}")),
        }
    }
    bail!("all UDP tracker addresses failed: {}", failures.join("; "))
}

fn announce_udp_address(
    tracker: SocketAddr,
    torrent: &TorrentV1,
    peer_id: &[u8; 20],
    key: u32,
    port: u16,
    counters: TrackerCounters,
    event: TrackerEvent,
) -> Result<TrackerResponse> {
    let bind = if tracker.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind).context("bind UDP tracker socket")?;
    socket.connect(tracker)?;
    socket.set_read_timeout(Some(UDP_TIMEOUT))?;
    socket.set_write_timeout(Some(UDP_TIMEOUT))?;

    let connect_transaction = transaction_id(peer_id, b"connect")?;
    let mut connect_request = Vec::with_capacity(16);
    connect_request.extend_from_slice(&UDP_PROTOCOL_ID.to_be_bytes());
    connect_request.extend_from_slice(&0_u32.to_be_bytes());
    connect_request.extend_from_slice(&connect_transaction.to_be_bytes());
    let connect_response = udp_round_trip(&socket, &connect_request)?;
    validate_udp_header(&connect_response, connect_transaction, 0)?;
    if connect_response.len() < 16 {
        bail!("UDP tracker connect response is shorter than 16 bytes");
    }
    let connection_id = u64::from_be_bytes(connect_response[8..16].try_into()?);

    let announce_transaction = transaction_id(peer_id, b"announce")?;
    let info_hash = decode_info_hash(torrent)?;
    let mut request = Vec::with_capacity(98);
    request.extend_from_slice(&connection_id.to_be_bytes());
    request.extend_from_slice(&1_u32.to_be_bytes());
    request.extend_from_slice(&announce_transaction.to_be_bytes());
    request.extend_from_slice(&info_hash);
    request.extend_from_slice(peer_id);
    request.extend_from_slice(&counters.uploaded.to_be_bytes());
    request.extend_from_slice(&counters.left.to_be_bytes());
    request.extend_from_slice(&0_u64.to_be_bytes());
    request.extend_from_slice(&event.udp_value().to_be_bytes());
    request.extend_from_slice(&0_u32.to_be_bytes());
    request.extend_from_slice(&key.to_be_bytes());
    request.extend_from_slice(&50_i32.to_be_bytes());
    request.extend_from_slice(&port.to_be_bytes());

    let response = udp_round_trip(&socket, &request)?;
    validate_udp_header(&response, announce_transaction, 1)?;
    if response.len() < 20 {
        bail!("UDP tracker announce response is shorter than 20 bytes");
    }
    let interval = u32::from_be_bytes(response[8..12].try_into()?) as u64;
    let compact = &response[20..];
    let stride = if tracker.is_ipv4() { 6 } else { 18 };
    if !compact.len().is_multiple_of(stride) {
        bail!("UDP tracker peer payload has an invalid length");
    }
    if compact.len() / stride > MAX_TRACKER_PEERS {
        bail!("UDP tracker response exceeds the peer safety limit");
    }
    let mut peers = Vec::with_capacity(compact.len() / stride);
    if tracker.is_ipv4() {
        for peer in compact.chunks_exact(6) {
            let port = u16::from_be_bytes([peer[4], peer[5]]);
            if port != 0 {
                peers.push(SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::new(peer[0], peer[1], peer[2], peer[3])),
                    port,
                ));
            }
        }
    } else {
        for peer in compact.chunks_exact(18) {
            let address = <[u8; 16]>::try_from(&peer[..16])?;
            let port = u16::from_be_bytes([peer[16], peer[17]]);
            if port != 0 {
                peers.push(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(address)), port));
            }
        }
    }
    Ok(TrackerResponse {
        interval,
        peers,
        warning_message: None,
    })
}

fn udp_round_trip(socket: &UdpSocket, request: &[u8]) -> Result<Vec<u8>> {
    socket.send(request)?;
    let mut response = vec![0_u8; MAX_UDP_RESPONSE_BYTES];
    let length = socket.recv(&mut response)?;
    response.truncate(length);
    Ok(response)
}

fn validate_udp_header(
    response: &[u8],
    expected_transaction: u32,
    expected_action: u32,
) -> Result<()> {
    if response.len() < 8 {
        bail!("UDP tracker response is shorter than 8 bytes");
    }
    let action = u32::from_be_bytes(response[..4].try_into()?);
    let transaction = u32::from_be_bytes(response[4..8].try_into()?);
    if transaction != expected_transaction {
        bail!("UDP tracker returned the wrong transaction ID");
    }
    if action == 3 {
        let message = String::from_utf8_lossy(&response[8..]);
        bail!("UDP tracker failure: {message}");
    }
    if action != expected_action {
        bail!("UDP tracker returned action {action}, expected {expected_action}");
    }
    Ok(())
}

fn decode_info_hash(torrent: &TorrentV1) -> Result<[u8; 20]> {
    let bytes = hex::decode(&torrent.info_hash_sha1).context("decode torrent info-hash")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("torrent info-hash must be 20 bytes"))
}

fn tracker_key(peer_id: &[u8; 20]) -> u32 {
    let digest = Sha1::digest(peer_id);
    u32::from_be_bytes(digest[..4].try_into().expect("SHA-1 has four bytes"))
}

fn transaction_id(peer_id: &[u8; 20], label: &[u8]) -> Result<u32> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    let mut digest = Sha1::new();
    digest.update(peer_id);
    digest.update(label);
    digest.update(now.to_be_bytes());
    digest.update(std::process::id().to_be_bytes());
    let hash = digest.finalize();
    Ok(u32::from_be_bytes(
        hash[..4].try_into().expect("SHA-1 has four bytes"),
    ))
}

fn percent_encode_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(bytes.len() * 3);
    for byte in bytes {
        encoded.push('%');
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn redact_tracker_url(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?<redacted>"),
        None => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{percent_encode_bytes, redact_tracker_url, shuffled_tier};

    #[test]
    fn percent_encodes_every_raw_byte() {
        assert_eq!(percent_encode_bytes(&[0, b'A', 0xff]), "%00%41%FF");
    }

    #[test]
    fn redacts_private_tracker_query_from_reports() {
        assert_eq!(
            redact_tracker_url("https://tracker.example/announce?passkey=secret"),
            "https://tracker.example/announce?<redacted>"
        );
    }

    #[test]
    fn tier_shuffle_is_deterministic_for_one_session() {
        let tier = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let peer_id = *b"-SM2000-123456789012";
        assert_eq!(
            shuffled_tier(tier.clone(), &peer_id, 0),
            shuffled_tier(tier, &peer_id, 0)
        );
    }
}
