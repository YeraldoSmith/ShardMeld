use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::bittorrent::{plan_v1_bridge, read_verified_chunk};
use crate::bt_peer::generate_peer_id;
use crate::bt_tracker::{BtTrackerLifecycleAttempt, start_seed_trackers, stop_seed_trackers};
use crate::{
    BtBridgeReport, IndexDb, REPORT_FORMAT, REPORT_VERSION, TargetDescriptor, TorrentV1,
    sha256_file,
};

const PROTOCOL_NAME: &[u8; 19] = b"BitTorrent protocol";
const HANDSHAKE_LENGTH: usize = 68;
const BLOCK_LENGTH: u32 = 16 * 1024;
const MAX_MESSAGE_LENGTH: u32 = 2 * 1024 * 1024;
const PEER_TIMEOUT: Duration = Duration::from_secs(5);

const MESSAGE_CHOKE: u8 = 0;
const MESSAGE_UNCHOKE: u8 = 1;
const MESSAGE_INTERESTED: u8 = 2;
const MESSAGE_NOT_INTERESTED: u8 = 3;
const MESSAGE_BITFIELD: u8 = 5;
const MESSAGE_REQUEST: u8 = 6;
const MESSAGE_PIECE: u8 = 7;
const MESSAGE_CANCEL: u8 = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtSeedReport {
    pub report_format: String,
    pub report_version: u32,
    pub engine_version: String,
    pub bind: SocketAddr,
    pub source: PathBuf,
    pub info_hash_sha1: String,
    pub source_sha256: String,
    pub advertised_pieces: u64,
    pub connections: u64,
    pub successful_handshakes: u64,
    pub block_requests: u64,
    pub payload_bytes_sent: u64,
    pub cancel_messages_received: u64,
    pub protocol_errors: u64,
    pub source_verified: bool,
    #[serde(default)]
    pub tracker_announces: Vec<BtTrackerLifecycleAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtIndexSeedReport {
    pub report_format: String,
    pub report_version: u32,
    pub engine_version: String,
    pub bind: SocketAddr,
    pub index_db: PathBuf,
    pub info_hash_sha1: String,
    pub target_sha256: String,
    pub total_pieces: u64,
    pub advertised_pieces: u64,
    pub advertised_piece_bytes: u64,
    pub preflight_locally_covered_bytes: u64,
    pub connections: u64,
    pub successful_handshakes: u64,
    pub block_requests: u64,
    pub payload_bytes_sent: u64,
    pub on_demand_local_chunks_read: u64,
    pub on_demand_local_bytes_read: u64,
    pub cancel_messages_received: u64,
    pub protocol_errors: u64,
    #[serde(default)]
    pub tracker_announces: Vec<BtTrackerLifecycleAttempt>,
}

struct BtIndexSeedStart {
    plan: BtBridgeReport,
    local_peer_id: [u8; 20],
    max_connections: Option<u64>,
}

pub fn serve_v1_file(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    bind: SocketAddr,
    allow_non_loopback: bool,
    max_connections: Option<u64>,
) -> Result<BtSeedReport> {
    if !bind.ip().is_loopback() && !allow_non_loopback {
        bail!("refusing non-loopback BT seed bind without --allow-non-loopback");
    }
    let source_sha256 = validate_seed_source(torrent, descriptor, source)?;
    let listener = TcpListener::bind(bind).with_context(|| format!("bind BT seed {bind}"))?;
    let bind = listener.local_addr()?;
    let local_peer_id = generate_peer_id()?;
    let (active_trackers, mut tracker_announces) =
        start_seed_trackers(torrent, &local_peer_id, bind.port());
    let result = serve_v1_file_listener_verified(
        listener,
        torrent,
        source,
        &source_sha256,
        max_connections,
        &local_peer_id,
    );
    let uploaded = result
        .as_ref()
        .map(|report| report.payload_bytes_sent)
        .unwrap_or(0);
    tracker_announces.extend(stop_seed_trackers(
        &active_trackers,
        torrent,
        &local_peer_id,
        bind.port(),
        uploaded,
    ));
    result.map(|mut report| {
        report.tracker_announces = tracker_announces;
        report
    })
}

pub fn serve_v1_file_listener(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    max_connections: Option<u64>,
) -> Result<BtSeedReport> {
    let source_sha256 = validate_seed_source(torrent, descriptor, source)?;
    let local_peer_id = generate_peer_id()?;
    serve_v1_file_listener_verified(
        listener,
        torrent,
        source,
        &source_sha256,
        max_connections,
        &local_peer_id,
    )
}

fn serve_v1_file_listener_verified(
    listener: TcpListener,
    torrent: &TorrentV1,
    source: &Path,
    source_sha256: &str,
    max_connections: Option<u64>,
    local_peer_id: &[u8; 20],
) -> Result<BtSeedReport> {
    let bind = listener.local_addr()?;
    let mut report = BtSeedReport {
        report_format: REPORT_FORMAT.to_owned(),
        report_version: REPORT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        bind,
        source: source.to_path_buf(),
        info_hash_sha1: torrent.info_hash_sha1.clone(),
        source_sha256: source_sha256.to_owned(),
        advertised_pieces: torrent.piece_sha1.len() as u64,
        connections: 0,
        successful_handshakes: 0,
        block_requests: 0,
        payload_bytes_sent: 0,
        cancel_messages_received: 0,
        protocol_errors: 0,
        source_verified: true,
        tracker_announces: Vec::new(),
    };

    while max_connections.is_none_or(|limit| report.connections < limit) {
        let (mut stream, _) = listener.accept().context("accept BT seed peer")?;
        report.connections += 1;
        stream.set_read_timeout(Some(PEER_TIMEOUT))?;
        stream.set_write_timeout(Some(PEER_TIMEOUT))?;
        if let Err(error) = serve_peer(&mut stream, torrent, source, local_peer_id, &mut report) {
            report.protocol_errors += 1;
            if max_connections == Some(1) {
                return Err(error);
            }
        }
    }
    Ok(report)
}

pub fn serve_v1_index(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    index_db: &Path,
    bind: SocketAddr,
    allow_non_loopback: bool,
    max_connections: Option<u64>,
) -> Result<BtIndexSeedReport> {
    if !bind.ip().is_loopback() && !allow_non_loopback {
        bail!("refusing non-loopback BT index seed bind without --allow-non-loopback");
    }
    let plan = plan_v1_bridge(torrent, descriptor, index)?;
    if !plan.pieces.iter().any(|piece| piece.fully_local) {
        bail!("authorized index cannot reconstruct any verified torrent Piece");
    }
    let listener = TcpListener::bind(bind).with_context(|| format!("bind BT index seed {bind}"))?;
    let bind = listener.local_addr()?;
    let local_peer_id = generate_peer_id()?;
    let (active_trackers, mut tracker_announces) =
        start_seed_trackers(torrent, &local_peer_id, bind.port());
    let result = serve_v1_index_listener_preflighted(
        listener,
        torrent,
        descriptor,
        index,
        index_db,
        BtIndexSeedStart {
            plan,
            local_peer_id,
            max_connections,
        },
    );
    let uploaded = result
        .as_ref()
        .map(|report| report.payload_bytes_sent)
        .unwrap_or(0);
    tracker_announces.extend(stop_seed_trackers(
        &active_trackers,
        torrent,
        &local_peer_id,
        bind.port(),
        uploaded,
    ));
    result.map(|mut report| {
        report.tracker_announces = tracker_announces;
        report
    })
}

pub fn serve_v1_index_listener(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    index_db: &Path,
    max_connections: Option<u64>,
) -> Result<BtIndexSeedReport> {
    let plan = plan_v1_bridge(torrent, descriptor, index)?;
    if !plan.pieces.iter().any(|piece| piece.fully_local) {
        bail!("authorized index cannot reconstruct any verified torrent Piece");
    }
    let local_peer_id = generate_peer_id()?;
    serve_v1_index_listener_preflighted(
        listener,
        torrent,
        descriptor,
        index,
        index_db,
        BtIndexSeedStart {
            plan,
            local_peer_id,
            max_connections,
        },
    )
}

fn serve_v1_index_listener_preflighted(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    index_db: &Path,
    start: BtIndexSeedStart,
) -> Result<BtIndexSeedReport> {
    let available: Vec<bool> = start
        .plan
        .pieces
        .iter()
        .map(|piece| piece.fully_local)
        .collect();
    let bind = listener.local_addr()?;
    let mut report = BtIndexSeedReport {
        report_format: REPORT_FORMAT.to_owned(),
        report_version: REPORT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        bind,
        index_db: index_db.to_path_buf(),
        info_hash_sha1: torrent.info_hash_sha1.clone(),
        target_sha256: descriptor.target.sha256.clone(),
        total_pieces: start.plan.total_pieces,
        advertised_pieces: start.plan.fully_local_pieces,
        advertised_piece_bytes: start.plan.fully_reconstructable_piece_bytes,
        preflight_locally_covered_bytes: start.plan.locally_covered_bytes,
        connections: 0,
        successful_handshakes: 0,
        block_requests: 0,
        payload_bytes_sent: 0,
        on_demand_local_chunks_read: 0,
        on_demand_local_bytes_read: 0,
        cancel_messages_received: 0,
        protocol_errors: 0,
        tracker_announces: Vec::new(),
    };

    while start
        .max_connections
        .is_none_or(|limit| report.connections < limit)
    {
        let (mut stream, _) = listener.accept().context("accept BT index seed peer")?;
        report.connections += 1;
        stream.set_read_timeout(Some(PEER_TIMEOUT))?;
        stream.set_write_timeout(Some(PEER_TIMEOUT))?;
        if let Err(error) = serve_index_peer(
            &mut stream,
            torrent,
            descriptor,
            index,
            &available,
            &start.local_peer_id,
            &mut report,
        ) {
            report.protocol_errors += 1;
            if start.max_connections == Some(1) {
                return Err(error);
            }
        }
    }
    Ok(report)
}

fn validate_seed_source(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
) -> Result<String> {
    descriptor.validate()?;
    if torrent.total_length != descriptor.target.size {
        bail!("torrent/descriptor size mismatch for BT seed");
    }
    let metadata = source
        .metadata()
        .with_context(|| format!("read BT seed source metadata {}", source.display()))?;
    if metadata.len() != torrent.total_length {
        bail!("BT seed source length does not match torrent");
    }
    let source_sha256 = sha256_file(source)?;
    if source_sha256 != descriptor.target.sha256 {
        bail!("BT seed source SHA-256 does not match descriptor");
    }
    let mut file = File::open(source)?;
    for (index, expected) in torrent.piece_sha1.iter().enumerate() {
        let length = piece_length(torrent, index)?;
        let mut bytes = vec![0_u8; usize::try_from(length)?];
        file.read_exact(&mut bytes)?;
        let actual = hex::encode(Sha1::digest(&bytes));
        if &actual != expected {
            bail!("BT seed source Piece {index} SHA-1 mismatch");
        }
    }
    Ok(source_sha256)
}

fn serve_peer(
    stream: &mut TcpStream,
    torrent: &TorrentV1,
    source: &Path,
    local_peer_id: &[u8; 20],
    report: &mut BtSeedReport,
) -> Result<()> {
    read_and_reply_handshake(stream, &torrent.info_hash_sha1, local_peer_id)?;
    report.successful_handshakes += 1;
    send_message(
        stream,
        MESSAGE_BITFIELD,
        &complete_bitfield(torrent.piece_sha1.len()),
    )?;
    let mut file = File::open(source)?;
    let mut unchoked = false;
    loop {
        let message = match read_message(stream) {
            Ok(message) => message,
            Err(error) if is_connection_end(&error) => break,
            Err(error) => return Err(error),
        };
        let Some((message_id, payload)) = message else {
            continue;
        };
        match message_id {
            MESSAGE_INTERESTED => {
                expect_empty(&payload, "interested")?;
                if !unchoked {
                    send_message(stream, MESSAGE_UNCHOKE, &[])?;
                    unchoked = true;
                }
            }
            MESSAGE_NOT_INTERESTED => {
                expect_empty(&payload, "not interested")?;
                break;
            }
            MESSAGE_REQUEST => {
                if !unchoked {
                    bail!("BT peer requested data before unchoke");
                }
                let (piece, begin, length) = parse_block_message(&payload, torrent)?;
                let absolute = u64::from(piece)
                    .checked_mul(torrent.piece_length)
                    .and_then(|offset| offset.checked_add(u64::from(begin)))
                    .context("BT seed request offset overflow")?;
                file.seek(SeekFrom::Start(absolute))?;
                let mut block = vec![0_u8; length as usize];
                file.read_exact(&mut block)?;
                let mut response = Vec::with_capacity(8 + block.len());
                response.extend_from_slice(&piece.to_be_bytes());
                response.extend_from_slice(&begin.to_be_bytes());
                response.extend_from_slice(&block);
                send_message(stream, MESSAGE_PIECE, &response)?;
                report.block_requests += 1;
                report.payload_bytes_sent += u64::from(length);
            }
            MESSAGE_CANCEL => {
                parse_block_message(&payload, torrent)?;
                report.cancel_messages_received += 1;
            }
            MESSAGE_CHOKE => expect_empty(&payload, "choke")?,
            _ => {}
        }
    }
    Ok(())
}

fn serve_index_peer(
    stream: &mut TcpStream,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    available: &[bool],
    local_peer_id: &[u8; 20],
    report: &mut BtIndexSeedReport,
) -> Result<()> {
    read_and_reply_handshake(stream, &torrent.info_hash_sha1, local_peer_id)?;
    report.successful_handshakes += 1;
    send_message(stream, MESSAGE_BITFIELD, &availability_bitfield(available))?;
    let mut unchoked = false;
    let mut cached_piece: Option<(u32, Vec<u8>)> = None;
    loop {
        let message = match read_message(stream) {
            Ok(message) => message,
            Err(error) if is_connection_end(&error) => break,
            Err(error) => return Err(error),
        };
        let Some((message_id, payload)) = message else {
            continue;
        };
        match message_id {
            MESSAGE_INTERESTED => {
                expect_empty(&payload, "interested")?;
                if !unchoked {
                    send_message(stream, MESSAGE_UNCHOKE, &[])?;
                    unchoked = true;
                }
            }
            MESSAGE_NOT_INTERESTED => {
                expect_empty(&payload, "not interested")?;
                break;
            }
            MESSAGE_REQUEST => {
                if !unchoked {
                    bail!("BT peer requested index data before unchoke");
                }
                let (piece, begin, length) = parse_block_message(&payload, torrent)?;
                if !available[piece as usize] {
                    bail!("BT peer requested a Piece not advertised by the local index");
                }
                if cached_piece.as_ref().map(|cached| cached.0) != Some(piece) {
                    let (bytes, chunks_read, bytes_read) =
                        reconstruct_index_piece(torrent, descriptor, index, piece as usize)?;
                    report.on_demand_local_chunks_read += chunks_read;
                    report.on_demand_local_bytes_read += bytes_read;
                    cached_piece = Some((piece, bytes));
                }
                let piece_bytes = &cached_piece.as_ref().context("missing cached Piece")?.1;
                let block = &piece_bytes[begin as usize..(begin + length) as usize];
                let mut response = Vec::with_capacity(8 + block.len());
                response.extend_from_slice(&piece.to_be_bytes());
                response.extend_from_slice(&begin.to_be_bytes());
                response.extend_from_slice(block);
                send_message(stream, MESSAGE_PIECE, &response)?;
                report.block_requests += 1;
                report.payload_bytes_sent += u64::from(length);
            }
            MESSAGE_CANCEL => {
                parse_block_message(&payload, torrent)?;
                report.cancel_messages_received += 1;
            }
            MESSAGE_CHOKE => expect_empty(&payload, "choke")?,
            _ => {}
        }
    }
    Ok(())
}

fn reconstruct_index_piece(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    piece_index: usize,
) -> Result<(Vec<u8>, u64, u64)> {
    let piece_offset = (piece_index as u64)
        .checked_mul(torrent.piece_length)
        .context("BT index seed Piece offset overflow")?;
    let length = piece_length(torrent, piece_index)?;
    let piece_end = piece_offset + length;
    let mut piece_bytes = vec![0_u8; usize::try_from(length)?];
    let mut chunks_read = 0_u64;
    let mut bytes_read = 0_u64;
    for chunk in &descriptor.chunks {
        let chunk_end = chunk.offset + u64::from(chunk.length);
        let overlap_start = chunk.offset.max(piece_offset);
        let overlap_end = chunk_end.min(piece_end);
        if overlap_start >= overlap_end {
            continue;
        }
        let source = index
            .lookup_chunk(&chunk.hash, chunk.length)?
            .context("advertised index Piece lost a required local chunk")?;
        let bytes = read_verified_chunk(&source.path, source.offset, chunk.length, &chunk.hash)?;
        let source_start = usize::try_from(overlap_start - chunk.offset)?;
        let destination_start = usize::try_from(overlap_start - piece_offset)?;
        let overlap_length = usize::try_from(overlap_end - overlap_start)?;
        piece_bytes[destination_start..destination_start + overlap_length]
            .copy_from_slice(&bytes[source_start..source_start + overlap_length]);
        chunks_read += 1;
        bytes_read += u64::from(chunk.length);
    }
    let actual = hex::encode(Sha1::digest(&piece_bytes));
    if actual != torrent.piece_sha1[piece_index] {
        bail!("on-demand reconstructed BT Piece {piece_index} failed SHA-1");
    }
    Ok((piece_bytes, chunks_read, bytes_read))
}

fn read_and_reply_handshake(
    stream: &mut TcpStream,
    expected_info_hash_hex: &str,
    peer_id: &[u8; 20],
) -> Result<()> {
    let mut handshake = [0_u8; HANDSHAKE_LENGTH];
    stream.read_exact(&mut handshake)?;
    if handshake[0] != 19 || &handshake[1..20] != PROTOCOL_NAME {
        bail!("BT seed peer sent an invalid handshake header");
    }
    let expected = hex::decode(expected_info_hash_hex)?;
    if handshake[28..48] != expected {
        bail!("BT seed peer requested the wrong info hash");
    }
    let mut response = Vec::with_capacity(HANDSHAKE_LENGTH);
    response.push(19);
    response.extend_from_slice(PROTOCOL_NAME);
    response.extend_from_slice(&[0_u8; 8]);
    response.extend_from_slice(&expected);
    response.extend_from_slice(peer_id);
    stream.write_all(&response)?;
    stream.flush()?;
    Ok(())
}

fn complete_bitfield(piece_count: usize) -> Vec<u8> {
    let mut bitfield = vec![0xff; piece_count.div_ceil(8)];
    let spare = bitfield.len() * 8 - piece_count;
    if spare > 0
        && let Some(last) = bitfield.last_mut()
    {
        *last &= 0xff << spare;
    }
    bitfield
}

fn availability_bitfield(available: &[bool]) -> Vec<u8> {
    let mut bitfield = vec![0_u8; available.len().div_ceil(8)];
    for (index, available) in available.iter().enumerate() {
        if *available {
            bitfield[index / 8] |= 1 << (7 - index % 8);
        }
    }
    bitfield
}

fn parse_block_message(payload: &[u8], torrent: &TorrentV1) -> Result<(u32, u32, u32)> {
    if payload.len() != 12 {
        bail!("BT peer sent malformed request or cancel");
    }
    let piece = u32::from_be_bytes(payload[0..4].try_into().unwrap());
    let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
    let length = u32::from_be_bytes(payload[8..12].try_into().unwrap());
    if length == 0 || length > BLOCK_LENGTH {
        bail!("BT peer requested an invalid block length");
    }
    let piece_index = piece as usize;
    if piece_index >= torrent.piece_sha1.len() {
        bail!("BT peer requested a Piece outside the torrent");
    }
    let piece_length = piece_length(torrent, piece_index)?;
    if u64::from(begin) + u64::from(length) > piece_length {
        bail!("BT peer requested a block outside its Piece");
    }
    Ok((piece, begin, length))
}

fn piece_length(torrent: &TorrentV1, index: usize) -> Result<u64> {
    let offset = (index as u64)
        .checked_mul(torrent.piece_length)
        .context("BT seed Piece offset overflow")?;
    Ok(torrent
        .piece_length
        .min(torrent.total_length.saturating_sub(offset)))
}

fn send_message(stream: &mut TcpStream, message_id: u8, payload: &[u8]) -> Result<()> {
    let length = u32::try_from(payload.len() + 1)?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&[message_id])?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

fn read_message(stream: &mut TcpStream) -> Result<Option<(u8, Vec<u8>)>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length);
    if length == 0 {
        return Ok(None);
    }
    if length > MAX_MESSAGE_LENGTH {
        bail!("BT seed peer sent an oversized message");
    }
    let mut message = vec![0_u8; length as usize];
    stream.read_exact(&mut message)?;
    Ok(Some((message[0], message[1..].to_vec())))
}

fn expect_empty(payload: &[u8], name: &str) -> Result<()> {
    if !payload.is_empty() {
        bail!("BT peer sent malformed {name} message");
    }
    Ok(())
}

fn is_connection_end(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            ErrorKind::UnexpectedEof
                | ErrorKind::ConnectionReset
                | ErrorKind::BrokenPipe
                | ErrorKind::TimedOut
                | ErrorKind::WouldBlock
        )
    })
}
