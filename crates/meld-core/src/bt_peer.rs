use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::bittorrent::read_verified_chunk;
use crate::{
    IndexDb, REPORT_FORMAT, REPORT_VERSION, TargetDescriptor, TorrentV1, plan_v1_bridge,
    sha256_file,
};

const PROTOCOL_NAME: &[u8; 19] = b"BitTorrent protocol";
const HANDSHAKE_LENGTH: usize = 68;
const BLOCK_LENGTH: u64 = 16 * 1024;
const MAX_INFLIGHT_REQUESTS: usize = 16;
const MAX_CONCURRENT_PEERS: usize = 4;
const MAX_MESSAGE_LENGTH: u32 = 2 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PEER_STALL_TIMEOUT: Duration = Duration::from_secs(5);
const RESUME_FORMAT: &str = "shardmeld-bt-resume";
const RESUME_VERSION: u32 = 1;

const MESSAGE_CHOKE: u8 = 0;
const MESSAGE_UNCHOKE: u8 = 1;
const MESSAGE_INTERESTED: u8 = 2;
const MESSAGE_NOT_INTERESTED: u8 = 3;
const MESSAGE_HAVE: u8 = 4;
const MESSAGE_BITFIELD: u8 = 5;
const MESSAGE_REQUEST: u8 = 6;
const MESSAGE_PIECE: u8 = 7;
const MESSAGE_CANCEL: u8 = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BtPeerFetchReport {
    pub report_format: String,
    pub report_version: u32,
    pub engine_version: String,
    pub peer: SocketAddr,
    pub remote_peer_id_hex: String,
    pub remote_reserved_hex: String,
    pub info_hash_sha1: String,
    pub output: PathBuf,
    pub target_bytes: u64,
    pub total_pieces: u64,
    pub remote_available_pieces: u64,
    pub fully_local_pieces: u64,
    pub fully_local_piece_bytes: u64,
    pub local_bytes_available: u64,
    pub genuinely_missing_bytes: u64,
    pub remaining_missing_bytes_before_run: u64,
    pub request_window: u64,
    pub concurrent_peer_limit: u64,
    pub piece_selection_strategy: String,
    pub endgame_enabled: bool,
    pub endgame_duplicate_pieces: u64,
    pub endgame_cancelled_jobs: u64,
    pub endgame_cancel_messages: u64,
    pub peer_stall_timeout_seconds: u64,
    pub peers_connected: u64,
    pub contributing_peers: Vec<SocketAddr>,
    pub pieces_reassigned: u64,
    pub resumed_verified_pieces: u64,
    pub resumed_verified_piece_bytes: u64,
    pub network_payload_avoided_by_resume: u64,
    pub newly_verified_pieces: u64,
    pub network_pieces_requested: u64,
    pub network_block_requests: u64,
    pub network_payload_bytes: u64,
    pub network_redundant_bytes: u64,
    pub output_sha256: String,
    pub verified: bool,
}

#[derive(Debug)]
struct LocalRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone)]
struct BlockRequest {
    piece_index: u32,
    begin: u32,
    length: u32,
    absolute_offset: u64,
}

#[derive(Debug, Clone)]
struct PieceJob {
    piece_index: u32,
    requests: Vec<BlockRequest>,
    attempt: u32,
    endgame_duplicate: bool,
}

#[derive(Debug, Clone)]
struct ActivePieceJob {
    job: PieceJob,
    claims: usize,
    completed: bool,
}

#[derive(Debug)]
struct SchedulerState {
    queue: VecDeque<PieceJob>,
    active: HashMap<u32, ActivePieceJob>,
    pieces_reassigned: u64,
    endgame_duplicate_pieces: u64,
    endgame_cancelled_jobs: u64,
    pending_peer_registrations: usize,
    availability_counts: Vec<u16>,
}

#[derive(Debug, Default)]
struct TransferCounters {
    block_requests_sent: u64,
    payload_bytes_received: u64,
    cancel_messages_sent: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct BtPeerWorkerReport {
    pub peer: SocketAddr,
    pub remote_peer_id_hex: Option<String>,
    pub remote_reserved_hex: Option<String>,
    pub available_pieces: Vec<bool>,
    pub block_requests_sent: u64,
    pub payload_bytes_received: u64,
    pub cancel_messages_sent: u64,
    pub endgame_jobs_cancelled: u64,
    pub pieces_verified: u64,
    pub verified_piece_indices: Vec<u32>,
    pub active_transfer_micros: u64,
    pub payload_bytes_per_second: u64,
    pub error: Option<String>,
}

impl BtPeerWorkerReport {
    fn new(peer: SocketAddr, piece_count: usize) -> Self {
        Self {
            peer,
            remote_peer_id_hex: None,
            remote_reserved_hex: None,
            available_pieces: vec![false; piece_count],
            block_requests_sent: 0,
            payload_bytes_received: 0,
            cancel_messages_sent: 0,
            endgame_jobs_cancelled: 0,
            pieces_verified: 0,
            verified_piece_indices: Vec::new(),
            active_transfer_micros: 0,
            payload_bytes_per_second: 0,
            error: None,
        }
    }

    fn failed(peer: SocketAddr, error: &str) -> Self {
        let mut report = Self::new(peer, 0);
        report.error = Some(error.to_owned());
        report
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct BtResumeState {
    format: String,
    version: u32,
    info_hash_sha1: String,
    target_sha256: String,
    target_bytes: u64,
    piece_length: u64,
    completed_pieces: Vec<bool>,
}

struct ResumeFile {
    file: File,
    partial_path: PathBuf,
    state_path: PathBuf,
    state: BtResumeState,
    resumed_verified_pieces: u64,
    resumed_verified_piece_bytes: u64,
}

#[derive(Clone)]
struct PeerWorkerContext {
    partial_path: PathBuf,
    state_path: PathBuf,
    scheduler: Arc<(Mutex<SchedulerState>, Condvar)>,
    resume_state: Arc<Mutex<BtResumeState>>,
    commit_lock: Arc<Mutex<()>>,
}

pub fn fetch_v1_from_peer(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    peer: SocketAddr,
    output: &Path,
) -> Result<BtPeerFetchReport> {
    let peer_id = generate_peer_id()?;
    fetch_v1_from_peer_with_peer_id(torrent, descriptor, index, peer, output, &peer_id)
}

pub(crate) fn fetch_v1_from_peer_with_peer_id(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    peer: SocketAddr,
    output: &Path,
    local_peer_id: &[u8; 20],
) -> Result<BtPeerFetchReport> {
    let mut workers = Vec::new();
    fetch_v1_from_peers_with_peer_id(
        torrent,
        descriptor,
        index,
        &[peer],
        output,
        local_peer_id,
        &mut workers,
    )
}

pub(crate) fn fetch_v1_from_peers_with_peer_id(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    peers: &[SocketAddr],
    output: &Path,
    local_peer_id: &[u8; 20],
    worker_reports: &mut Vec<BtPeerWorkerReport>,
) -> Result<BtPeerFetchReport> {
    if output.exists() {
        bail!("refusing to overwrite existing output {}", output.display());
    }
    let bridge = plan_v1_bridge(torrent, descriptor, index)?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create output directory {}", parent.display()))?;
    let mut resume = open_resume_file(torrent, descriptor, output)?;

    let (local_ranges, local_bytes_available) =
        write_local_chunks(descriptor, index, &mut resume.file)?;
    let all_requests = build_block_requests(torrent, &local_ranges)?;
    let network_payload_avoided_by_resume: u64 = all_requests
        .iter()
        .filter(|request| resume.state.completed_pieces[request.piece_index as usize])
        .map(|request| u64::from(request.length))
        .sum();
    let requests: Vec<BlockRequest> = all_requests
        .into_iter()
        .filter(|request| !resume.state.completed_pieces[request.piece_index as usize])
        .collect();
    let requested_pieces: HashSet<u32> =
        requests.iter().map(|request| request.piece_index).collect();
    let remaining_missing_bytes_before_run: u64 = bridge
        .pieces
        .iter()
        .filter(|piece| !resume.state.completed_pieces[piece.index as usize])
        .map(|piece| piece.missing_bytes)
        .sum();

    if !requests.is_empty() && peers.is_empty() {
        bail!("no BitTorrent peers were supplied for missing pieces");
    }
    let scheduler = Arc::new((
        Mutex::new(SchedulerState {
            queue: build_piece_jobs(&requests),
            active: HashMap::new(),
            pieces_reassigned: 0,
            endgame_duplicate_pieces: 0,
            endgame_cancelled_jobs: 0,
            pending_peer_registrations: 0,
            availability_counts: vec![0; torrent.piece_sha1.len()],
        }),
        Condvar::new(),
    ));
    let shared_resume_state = Arc::new(Mutex::new(resume.state.clone()));
    let commit_lock = Arc::new(Mutex::new(()));

    for peer_batch in peers.chunks(MAX_CONCURRENT_PEERS) {
        if scheduler_is_complete(&scheduler)? {
            break;
        }
        begin_peer_batch(&scheduler, peer_batch.len(), torrent.piece_sha1.len())?;
        let mut handles = Vec::new();
        for peer in peer_batch {
            let torrent = torrent.clone();
            let peer_id = *local_peer_id;
            let context = PeerWorkerContext {
                partial_path: resume.partial_path.clone(),
                state_path: resume.state_path.clone(),
                scheduler: Arc::clone(&scheduler),
                resume_state: Arc::clone(&shared_resume_state),
                commit_lock: Arc::clone(&commit_lock),
            };
            let peer = *peer;
            handles.push((
                peer,
                thread::spawn(move || run_peer_worker(peer, &torrent, &peer_id, context)),
            ));
        }
        for (peer, handle) in handles {
            worker_reports.push(
                handle
                    .join()
                    .unwrap_or_else(|_| BtPeerWorkerReport::failed(peer, "peer worker panicked")),
            );
        }
    }

    let (unfinished_pieces, pieces_reassigned, endgame_duplicate_pieces, endgame_cancelled_jobs) = {
        let state = scheduler
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned"))?;
        let mut unfinished = state
            .queue
            .iter()
            .map(|job| job.piece_index)
            .collect::<Vec<_>>();
        unfinished.extend(
            state
                .active
                .values()
                .filter(|active| !active.completed)
                .map(|active| active.job.piece_index),
        );
        unfinished.sort_unstable();
        unfinished.dedup();
        (
            unfinished,
            state.pieces_reassigned,
            state.endgame_duplicate_pieces,
            state.endgame_cancelled_jobs,
        )
    };
    if !unfinished_pieces.is_empty() {
        let failures = worker_reports
            .iter()
            .filter_map(|report| {
                report
                    .error
                    .as_deref()
                    .map(|error| format!("{}: {error}", report.peer))
            })
            .collect::<Vec<_>>()
            .join("; ");
        bail!(
            "BitTorrent peers exhausted with unfinished pieces {:?}; failures=[{failures}]",
            unfinished_pieces
        );
    }

    resume.file.sync_all()?;
    verify_all_pieces(&mut resume.file, torrent)?;
    let output_sha256 = sha256_file(&resume.partial_path)?;
    if output_sha256 != descriptor.target.sha256 {
        bail!(
            "BT-rebuilt target SHA-256 mismatch: expected {}, got {output_sha256}",
            descriptor.target.sha256
        );
    }
    persist_completed_output(&resume.partial_path, output)?;
    std::fs::remove_file(&resume.state_path).with_context(|| {
        format!(
            "remove completed resume state {}",
            resume.state_path.display()
        )
    })?;

    let mut aggregate_availability = vec![false; torrent.piece_sha1.len()];
    for worker in worker_reports.iter() {
        for (aggregate, available) in aggregate_availability
            .iter_mut()
            .zip(&worker.available_pieces)
        {
            *aggregate |= *available;
        }
    }
    let remote_available_pieces = aggregate_availability
        .iter()
        .filter(|available| **available)
        .count() as u64;
    let peers_connected = worker_reports
        .iter()
        .filter(|worker| worker.remote_peer_id_hex.is_some())
        .count() as u64;
    let contributing_peers = worker_reports
        .iter()
        .filter(|worker| worker.pieces_verified > 0)
        .map(|worker| worker.peer)
        .collect::<Vec<_>>();
    let primary = worker_reports
        .iter()
        .filter(|worker| worker.remote_peer_id_hex.is_some())
        .max_by_key(|worker| worker.pieces_verified);
    let primary_peer = primary
        .map(|worker| worker.peer)
        .or_else(|| peers.first().copied())
        .context("no BitTorrent peer was available for the transfer report")?;
    let network_block_requests = worker_reports
        .iter()
        .map(|worker| worker.block_requests_sent)
        .sum::<u64>();
    let network_payload_bytes = worker_reports
        .iter()
        .map(|worker| worker.payload_bytes_received)
        .sum::<u64>();
    let newly_verified_pieces = worker_reports
        .iter()
        .map(|worker| worker.pieces_verified)
        .sum::<u64>();
    let endgame_cancel_messages = worker_reports
        .iter()
        .map(|worker| worker.cancel_messages_sent)
        .sum::<u64>();
    let network_redundant_bytes = network_payload_bytes
        .checked_sub(remaining_missing_bytes_before_run)
        .context("network payload was smaller than genuinely missing data")?;
    Ok(BtPeerFetchReport {
        report_format: REPORT_FORMAT.to_owned(),
        report_version: REPORT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        peer: primary_peer,
        remote_peer_id_hex: primary
            .and_then(|worker| worker.remote_peer_id_hex.clone())
            .unwrap_or_default(),
        remote_reserved_hex: primary
            .and_then(|worker| worker.remote_reserved_hex.clone())
            .unwrap_or_default(),
        info_hash_sha1: torrent.info_hash_sha1.clone(),
        output: output.to_path_buf(),
        target_bytes: torrent.total_length,
        total_pieces: bridge.total_pieces,
        remote_available_pieces,
        fully_local_pieces: bridge.fully_local_pieces,
        fully_local_piece_bytes: bridge.fully_reconstructable_piece_bytes,
        local_bytes_available,
        genuinely_missing_bytes: bridge.missing_bytes,
        remaining_missing_bytes_before_run,
        request_window: MAX_INFLIGHT_REQUESTS as u64,
        concurrent_peer_limit: MAX_CONCURRENT_PEERS as u64,
        piece_selection_strategy: "rarest-first".to_owned(),
        endgame_enabled: true,
        endgame_duplicate_pieces,
        endgame_cancelled_jobs,
        endgame_cancel_messages,
        peer_stall_timeout_seconds: PEER_STALL_TIMEOUT.as_secs(),
        peers_connected,
        contributing_peers,
        pieces_reassigned,
        resumed_verified_pieces: resume.resumed_verified_pieces,
        resumed_verified_piece_bytes: resume.resumed_verified_piece_bytes,
        network_payload_avoided_by_resume,
        newly_verified_pieces,
        network_pieces_requested: requested_pieces.len() as u64,
        network_block_requests,
        network_payload_bytes,
        network_redundant_bytes,
        output_sha256,
        verified: true,
    })
}

fn resume_paths(output: &Path) -> (PathBuf, PathBuf) {
    fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
        let mut value = OsString::from(path.as_os_str());
        value.push(suffix);
        PathBuf::from(value)
    }
    (
        with_suffix(output, ".shardmeld-partial"),
        with_suffix(output, ".shardmeld-resume.json"),
    )
}

fn open_resume_file(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    output: &Path,
) -> Result<ResumeFile> {
    let (partial_path, state_path) = resume_paths(output);
    let partial_exists = partial_path.exists();
    let state_exists = state_path.exists();
    if partial_exists != state_exists {
        bail!(
            "incomplete BT resume pair: expected both {} and {}; preserve or remove the orphan before retrying",
            partial_path.display(),
            state_path.display()
        );
    }

    if !partial_exists {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&partial_path)
            .with_context(|| format!("create BT partial file {}", partial_path.display()))?;
        file.set_len(torrent.total_length)?;
        let state = BtResumeState {
            format: RESUME_FORMAT.to_string(),
            version: RESUME_VERSION,
            info_hash_sha1: torrent.info_hash_sha1.clone(),
            target_sha256: descriptor.target.sha256.clone(),
            target_bytes: torrent.total_length,
            piece_length: torrent.piece_length,
            completed_pieces: vec![false; torrent.piece_sha1.len()],
        };
        persist_resume_state(&state_path, &state)?;
        return Ok(ResumeFile {
            file,
            partial_path,
            state_path,
            state,
            resumed_verified_pieces: 0,
            resumed_verified_piece_bytes: 0,
        });
    }

    let encoded = std::fs::read(&state_path)
        .with_context(|| format!("read BT resume state {}", state_path.display()))?;
    let mut state: BtResumeState = serde_json::from_slice(&encoded)
        .with_context(|| format!("parse BT resume state {}", state_path.display()))?;
    validate_resume_state(&state, torrent, descriptor)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&partial_path)
        .with_context(|| format!("open BT partial file {}", partial_path.display()))?;
    let actual_length = file.metadata()?.len();
    if actual_length != torrent.total_length {
        bail!(
            "BT partial length mismatch: expected {}, got {actual_length}",
            torrent.total_length
        );
    }

    let mut repaired = false;
    let mut resumed_verified_pieces = 0_u64;
    let mut resumed_verified_piece_bytes = 0_u64;
    for piece_index in 0..state.completed_pieces.len() {
        if !state.completed_pieces[piece_index] {
            continue;
        }
        if piece_matches(&mut file, torrent, piece_index)? {
            resumed_verified_pieces += 1;
            resumed_verified_piece_bytes += piece_length(torrent, piece_index)?;
        } else {
            state.completed_pieces[piece_index] = false;
            repaired = true;
        }
    }
    if repaired {
        persist_resume_state(&state_path, &state)?;
    }
    Ok(ResumeFile {
        file,
        partial_path,
        state_path,
        state,
        resumed_verified_pieces,
        resumed_verified_piece_bytes,
    })
}

fn validate_resume_state(
    state: &BtResumeState,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
) -> Result<()> {
    if state.format != RESUME_FORMAT || state.version != RESUME_VERSION {
        bail!("unsupported BT resume state format or version");
    }
    if state.info_hash_sha1 != torrent.info_hash_sha1
        || state.target_sha256 != descriptor.target.sha256
        || state.target_bytes != torrent.total_length
        || state.piece_length != torrent.piece_length
        || state.completed_pieces.len() != torrent.piece_sha1.len()
    {
        bail!("BT resume state does not belong to this torrent and descriptor");
    }
    Ok(())
}

fn persist_resume_state(path: &Path, state: &BtResumeState) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(".shardmeld-resume-")
        .suffix(".json.tmp")
        .tempfile_in(parent)
        .with_context(|| format!("create temporary resume state in {}", parent.display()))?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), state)?;
    temporary.as_file_mut().write_all(b"\n")?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("persist BT resume state {}", path.display()))?;
    Ok(())
}

fn persist_completed_output(partial: &Path, output: &Path) -> Result<()> {
    std::fs::hard_link(partial, output).with_context(|| {
        format!(
            "publish verified BT output {} without overwriting an existing file",
            output.display()
        )
    })?;
    std::fs::remove_file(partial)
        .with_context(|| format!("remove published BT partial {}", partial.display()))?;
    Ok(())
}

fn write_local_chunks(
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    output: &mut File,
) -> Result<(Vec<LocalRange>, u64)> {
    let mut ranges = Vec::new();
    let mut local_bytes = 0_u64;
    for chunk in &descriptor.chunks {
        let Some(source) = index.lookup_chunk(&chunk.hash, chunk.length)? else {
            continue;
        };
        let bytes = read_verified_chunk(&source.path, source.offset, chunk.length, &chunk.hash)?;
        output.seek(SeekFrom::Start(chunk.offset))?;
        output.write_all(&bytes)?;
        let end = chunk
            .offset
            .checked_add(u64::from(chunk.length))
            .context("local chunk range overflow")?;
        ranges.push(LocalRange {
            start: chunk.offset,
            end,
        });
        local_bytes += u64::from(chunk.length);
    }
    Ok((ranges, local_bytes))
}

fn build_block_requests(
    torrent: &TorrentV1,
    local_ranges: &[LocalRange],
) -> Result<Vec<BlockRequest>> {
    let mut requests = Vec::new();
    for piece_index in 0..torrent.piece_sha1.len() {
        let piece_offset = (piece_index as u64)
            .checked_mul(torrent.piece_length)
            .context("BT piece offset overflow")?;
        let piece_end = piece_offset
            .saturating_add(torrent.piece_length)
            .min(torrent.total_length);
        let piece_length = piece_end - piece_offset;
        let mut begin = 0_u64;
        while begin < piece_length {
            let length = BLOCK_LENGTH.min(piece_length - begin);
            let block_start = piece_offset + begin;
            let block_end = block_start + length;
            let covered: u64 = local_ranges
                .iter()
                .map(|range| overlap_length(block_start, block_end, range.start, range.end))
                .sum();
            if covered < length {
                requests.push(BlockRequest {
                    piece_index: u32::try_from(piece_index).context("too many BT pieces")?,
                    begin: u32::try_from(begin).context("BT block offset is too large")?,
                    length: u32::try_from(length).context("BT block is too large")?,
                    absolute_offset: block_start,
                });
            }
            begin += length;
        }
    }
    Ok(requests)
}

fn build_piece_jobs(requests: &[BlockRequest]) -> VecDeque<PieceJob> {
    let mut jobs = VecDeque::new();
    for request in requests {
        if jobs
            .back()
            .is_none_or(|job: &PieceJob| job.piece_index != request.piece_index)
        {
            jobs.push_back(PieceJob {
                piece_index: request.piece_index,
                requests: Vec::new(),
                attempt: 0,
                endgame_duplicate: false,
            });
        }
        jobs.back_mut()
            .expect("piece job was just inserted")
            .requests
            .push(request.clone());
    }
    jobs
}

fn begin_peer_batch(
    scheduler: &Arc<(Mutex<SchedulerState>, Condvar)>,
    peer_count: usize,
    piece_count: usize,
) -> Result<()> {
    let mut state = scheduler
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned"))?;
    if !state.active.is_empty() || state.pending_peer_registrations != 0 {
        bail!("cannot start a BT peer batch while scheduler work is active");
    }
    state.pending_peer_registrations = peer_count;
    state.availability_counts.clear();
    state.availability_counts.resize(piece_count, 0);
    Ok(())
}

fn register_peer_availability(
    scheduler: &Arc<(Mutex<SchedulerState>, Condvar)>,
    availability: Option<&[bool]>,
) -> Result<()> {
    let (lock, condition) = &**scheduler;
    let mut state = lock
        .lock()
        .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned"))?;
    if state.pending_peer_registrations == 0 {
        bail!("BT peer registered outside an active scheduler batch");
    }
    if let Some(availability) = availability {
        if availability.len() != state.availability_counts.len() {
            bail!("BT peer availability length does not match the torrent");
        }
        for (count, available) in state.availability_counts.iter_mut().zip(availability) {
            if *available {
                *count = count.saturating_add(1);
            }
        }
    }
    state.pending_peer_registrations -= 1;
    condition.notify_all();
    while state.pending_peer_registrations > 0 {
        state = condition
            .wait(state)
            .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned while registering"))?;
    }
    Ok(())
}

fn scheduler_is_complete(scheduler: &Arc<(Mutex<SchedulerState>, Condvar)>) -> Result<bool> {
    let state = scheduler
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned"))?;
    Ok(state.queue.is_empty() && state.active.is_empty())
}

fn piece_job_completed(
    scheduler: &Arc<(Mutex<SchedulerState>, Condvar)>,
    piece_index: u32,
) -> Result<bool> {
    let state = scheduler
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned"))?;
    Ok(state
        .active
        .get(&piece_index)
        .is_some_and(|active| active.completed))
}

fn claim_piece_job(
    scheduler: &Arc<(Mutex<SchedulerState>, Condvar)>,
    availability: &[bool],
) -> Result<Option<PieceJob>> {
    let (lock, condition) = &**scheduler;
    let mut state = lock
        .lock()
        .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned"))?;
    loop {
        if let Some(position) = state
            .queue
            .iter()
            .enumerate()
            .filter(|(_, job)| {
                availability
                    .get(job.piece_index as usize)
                    .copied()
                    .unwrap_or(false)
            })
            .min_by_key(|(_, job)| {
                (
                    state
                        .availability_counts
                        .get(job.piece_index as usize)
                        .copied()
                        .unwrap_or(u16::MAX),
                    std::cmp::Reverse(job.attempt),
                    job.piece_index,
                )
            })
            .map(|(position, _)| position)
        {
            let mut job = state
                .queue
                .remove(position)
                .context("BT scheduler queue changed unexpectedly")?;
            job.endgame_duplicate = false;
            state.active.insert(
                job.piece_index,
                ActivePieceJob {
                    job: job.clone(),
                    claims: 1,
                    completed: false,
                },
            );
            return Ok(Some(job));
        }
        let duplicate_piece = state
            .active
            .iter()
            .filter(|(piece_index, active)| {
                !active.completed
                    && active.claims == 1
                    && availability
                        .get(**piece_index as usize)
                        .copied()
                        .unwrap_or(false)
            })
            .min_by_key(|(piece_index, _)| **piece_index)
            .map(|(piece_index, _)| *piece_index);
        if let Some(piece_index) = duplicate_piece {
            let active = state
                .active
                .get_mut(&piece_index)
                .context("BT Endgame Piece disappeared")?;
            active.claims += 1;
            let mut duplicate = active.job.clone();
            duplicate.endgame_duplicate = true;
            state.endgame_duplicate_pieces += 1;
            return Ok(Some(duplicate));
        }
        if state.active.is_empty() {
            return Ok(None);
        }
        state = condition
            .wait(state)
            .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned while waiting"))?;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PieceJobOutcome {
    Success,
    Failed,
    Cancelled,
}

fn finish_piece_job(
    scheduler: &Arc<(Mutex<SchedulerState>, Condvar)>,
    mut job: PieceJob,
    outcome: PieceJobOutcome,
) -> Result<bool> {
    let (lock, condition) = &**scheduler;
    let mut state = lock
        .lock()
        .map_err(|_| anyhow::anyhow!("BT scheduler lock was poisoned"))?;
    let (first_success, claims, completed) = {
        let active = state
            .active
            .get_mut(&job.piece_index)
            .context("BT scheduler finished an unknown Piece job")?;
        active.claims = active
            .claims
            .checked_sub(1)
            .context("BT scheduler Piece claim accounting underflow")?;
        let first_success = outcome == PieceJobOutcome::Success && !active.completed;
        if first_success {
            active.completed = true;
        }
        (first_success, active.claims, active.completed)
    };
    if outcome == PieceJobOutcome::Cancelled {
        state.endgame_cancelled_jobs = state.endgame_cancelled_jobs.saturating_add(1);
    }
    if claims == 0 {
        state.active.remove(&job.piece_index);
        if !completed {
            job.attempt = job.attempt.saturating_add(1);
            job.endgame_duplicate = false;
            state.pieces_reassigned += 1;
            state.queue.push_back(job);
        }
    }
    condition.notify_all();
    Ok(first_success)
}

fn overlap_length(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> u64 {
    left_end
        .min(right_end)
        .saturating_sub(left_start.max(right_start))
}

struct RemoteHandshake {
    reserved: [u8; 8],
    peer_id: [u8; 20],
}

fn send_handshake(stream: &mut TcpStream, info_hash_hex: &str, peer_id: &[u8; 20]) -> Result<()> {
    let info_hash = hex::decode(info_hash_hex).context("decode torrent info-hash")?;
    if info_hash.len() != 20 {
        bail!("torrent info-hash must be 20 bytes");
    }
    let mut handshake = Vec::with_capacity(HANDSHAKE_LENGTH);
    handshake.push(PROTOCOL_NAME.len() as u8);
    handshake.extend_from_slice(PROTOCOL_NAME);
    handshake.extend_from_slice(&[0_u8; 8]);
    handshake.extend_from_slice(&info_hash);
    handshake.extend_from_slice(peer_id);
    stream.write_all(&handshake)?;
    stream.flush()?;
    Ok(())
}

fn read_handshake(stream: &mut TcpStream, expected_info_hash_hex: &str) -> Result<RemoteHandshake> {
    let mut handshake = [0_u8; HANDSHAKE_LENGTH];
    stream
        .read_exact(&mut handshake)
        .context("read BitTorrent handshake")?;
    if handshake[0] != PROTOCOL_NAME.len() as u8 || &handshake[1..20] != PROTOCOL_NAME {
        bail!("peer returned an invalid BitTorrent protocol header");
    }
    let expected_info_hash = hex::decode(expected_info_hash_hex)?;
    if handshake[28..48] != expected_info_hash {
        bail!("peer returned the wrong torrent info-hash");
    }
    let mut reserved = [0_u8; 8];
    reserved.copy_from_slice(&handshake[20..28]);
    let mut peer_id = [0_u8; 20];
    peer_id.copy_from_slice(&handshake[48..68]);
    Ok(RemoteHandshake { reserved, peer_id })
}

pub(crate) fn generate_peer_id() -> Result<[u8; 20]> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    let seed = format!("{}:{now}", std::process::id());
    let suffix = hex::encode(Sha1::digest(seed.as_bytes()));
    let text = format!("-SM2000-{}", &suffix[..12]);
    let mut peer_id = [0_u8; 20];
    peer_id.copy_from_slice(text.as_bytes());
    Ok(peer_id)
}

fn send_message(stream: &mut TcpStream, message_id: u8, payload: &[u8]) -> Result<()> {
    let message_length = u32::try_from(payload.len() + 1).context("BT message is too large")?;
    stream.write_all(&message_length.to_be_bytes())?;
    stream.write_all(&[message_id])?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

fn read_message(stream: &mut TcpStream) -> Result<Option<(u8, Vec<u8>)>> {
    let mut length_bytes = [0_u8; 4];
    stream.read_exact(&mut length_bytes)?;
    let message_length = u32::from_be_bytes(length_bytes);
    if message_length == 0 {
        return Ok(None);
    }
    if message_length > MAX_MESSAGE_LENGTH {
        bail!("peer sent oversized BT message of {message_length} bytes");
    }
    let mut message = vec![0_u8; message_length as usize];
    stream.read_exact(&mut message)?;
    let message_id = message[0];
    Ok(Some((message_id, message[1..].to_vec())))
}

fn process_control_message(
    message_id: u8,
    payload: &[u8],
    availability: &mut [bool],
    availability_received: &mut bool,
    choked: &mut bool,
) -> Result<()> {
    match message_id {
        MESSAGE_CHOKE => {
            expect_empty_payload(payload, "choke")?;
            *choked = true;
        }
        MESSAGE_UNCHOKE => {
            expect_empty_payload(payload, "unchoke")?;
            *choked = false;
        }
        MESSAGE_HAVE => {
            if payload.len() != 4 {
                bail!("peer sent malformed HAVE message");
            }
            let index = u32::from_be_bytes(payload.try_into().unwrap()) as usize;
            let slot = availability
                .get_mut(index)
                .context("peer HAVE index is outside torrent")?;
            *slot = true;
            *availability_received = true;
        }
        MESSAGE_BITFIELD => {
            apply_bitfield(payload, availability)?;
            *availability_received = true;
        }
        _ => {}
    }
    Ok(())
}

fn apply_bitfield(payload: &[u8], availability: &mut [bool]) -> Result<()> {
    let expected_length = availability.len().div_ceil(8);
    if payload.len() != expected_length {
        bail!(
            "peer bitfield length mismatch: expected {expected_length}, got {}",
            payload.len()
        );
    }
    for (index, available) in availability.iter_mut().enumerate() {
        *available = payload[index / 8] & (0x80 >> (index % 8)) != 0;
    }
    let spare_bits = payload.len() * 8 - availability.len();
    if spare_bits > 0 {
        let spare_mask = (1_u8 << spare_bits) - 1;
        if payload.last().is_some_and(|last| last & spare_mask != 0) {
            bail!("peer bitfield has non-zero spare bits");
        }
    }
    Ok(())
}

fn expect_empty_payload(payload: &[u8], message: &str) -> Result<()> {
    if !payload.is_empty() {
        bail!("peer sent malformed {message} message");
    }
    Ok(())
}

fn wait_for_unchoke(
    stream: &mut TcpStream,
    availability: &mut [bool],
    choked: &mut bool,
) -> Result<()> {
    let mut availability_received = true;
    while *choked {
        if let Some((message_id, payload)) = read_message(stream)? {
            process_control_message(
                message_id,
                &payload,
                availability,
                &mut availability_received,
                choked,
            )?;
        }
    }
    Ok(())
}

fn run_peer_worker(
    peer: SocketAddr,
    torrent: &TorrentV1,
    local_peer_id: &[u8; 20],
    context: PeerWorkerContext,
) -> BtPeerWorkerReport {
    let mut report = BtPeerWorkerReport::new(peer, torrent.piece_sha1.len());
    let mut availability_registered = false;
    let result = (|| -> Result<()> {
        let mut output = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&context.partial_path)
            .with_context(|| {
                format!(
                    "open shared BT partial file {}",
                    context.partial_path.display()
                )
            })?;
        let mut stream = TcpStream::connect_timeout(&peer, CONNECT_TIMEOUT)
            .with_context(|| format!("connect to BitTorrent peer {peer}"))?;
        stream.set_read_timeout(Some(PEER_STALL_TIMEOUT))?;
        stream.set_write_timeout(Some(PEER_STALL_TIMEOUT))?;
        send_handshake(&mut stream, &torrent.info_hash_sha1, local_peer_id)?;
        let remote_handshake = read_handshake(&mut stream, &torrent.info_hash_sha1)?;
        report.remote_peer_id_hex = Some(hex::encode(remote_handshake.peer_id));
        report.remote_reserved_hex = Some(hex::encode(remote_handshake.reserved));
        send_message(&mut stream, MESSAGE_INTERESTED, &[])?;

        let mut availability_received = false;
        let mut choked = true;
        while choked || !availability_received {
            if let Some((message_id, payload)) = read_message(&mut stream)? {
                process_control_message(
                    message_id,
                    &payload,
                    &mut report.available_pieces,
                    &mut availability_received,
                    &mut choked,
                )?;
            }
        }
        register_peer_availability(&context.scheduler, Some(&report.available_pieces))?;
        availability_registered = true;

        while let Some(job) = claim_piece_job(&context.scheduler, &report.available_pieces)? {
            let transfer_started = Instant::now();
            let mut counters = TransferCounters::default();
            let transfer = receive_piece_job(
                &mut PeerSession {
                    stream: &mut stream,
                    output: &mut output,
                    availability: &mut report.available_pieces,
                    choked: &mut choked,
                    scheduler: &context.scheduler,
                },
                torrent,
                &job,
                &mut counters,
            );
            report.block_requests_sent += counters.block_requests_sent;
            report.payload_bytes_received += counters.payload_bytes_received;
            report.cancel_messages_sent += counters.cancel_messages_sent;
            report.active_transfer_micros = report.active_transfer_micros.saturating_add(
                u64::try_from(transfer_started.elapsed().as_micros()).unwrap_or(u64::MAX),
            );
            match transfer {
                Ok(PieceReceiveOutcome::Verified(piece_bytes)) => {
                    let commit = (|| -> Result<bool> {
                        let _commit_guard = context
                            .commit_lock
                            .lock()
                            .map_err(|_| anyhow::anyhow!("BT Piece commit lock was poisoned"))?;
                        if piece_job_completed(&context.scheduler, job.piece_index)? {
                            return Ok(false);
                        }
                        commit_verified_piece(&mut output, torrent, job.piece_index, &piece_bytes)?;
                        output.sync_data()?;
                        let mut state = context
                            .resume_state
                            .lock()
                            .map_err(|_| anyhow::anyhow!("BT resume lock was poisoned"))?;
                        state.completed_pieces[job.piece_index as usize] = true;
                        if let Err(error) = persist_resume_state(&context.state_path, &state) {
                            state.completed_pieces[job.piece_index as usize] = false;
                            return Err(error);
                        }
                        Ok(true)
                    })();
                    let should_commit = match commit {
                        Ok(should_commit) => should_commit,
                        Err(error) => {
                            finish_piece_job(&context.scheduler, job, PieceJobOutcome::Failed)?;
                            return Err(error);
                        }
                    };
                    if !should_commit {
                        finish_piece_job(&context.scheduler, job, PieceJobOutcome::Cancelled)?;
                        report.endgame_jobs_cancelled += 1;
                        continue;
                    }
                    let first = finish_piece_job(
                        &context.scheduler,
                        job.clone(),
                        PieceJobOutcome::Success,
                    )?;
                    if first {
                        report.pieces_verified += 1;
                        report.verified_piece_indices.push(job.piece_index);
                    } else {
                        report.endgame_jobs_cancelled += 1;
                    }
                }
                Ok(PieceReceiveOutcome::Cancelled) => {
                    finish_piece_job(&context.scheduler, job, PieceJobOutcome::Cancelled)?;
                    report.endgame_jobs_cancelled += 1;
                }
                Err(error) => {
                    if piece_job_completed(&context.scheduler, job.piece_index)? {
                        finish_piece_job(&context.scheduler, job, PieceJobOutcome::Cancelled)?;
                        report.endgame_jobs_cancelled += 1;
                        continue;
                    }
                    finish_piece_job(&context.scheduler, job, PieceJobOutcome::Failed)?;
                    return Err(error);
                }
            }
        }
        send_message(&mut stream, MESSAGE_NOT_INTERESTED, &[])?;
        let _ = stream.shutdown(Shutdown::Both);
        Ok(())
    })();
    if !availability_registered
        && let Err(error) = register_peer_availability(&context.scheduler, None)
    {
        report.error = Some(format!("register failed peer: {error:#}"));
        return report;
    }
    if let Err(error) = result {
        report.error = Some(format!("{error:#}"));
    }
    report.payload_bytes_per_second = report
        .payload_bytes_received
        .saturating_mul(1_000_000)
        .checked_div(report.active_transfer_micros)
        .unwrap_or(0);
    report
}

enum PieceReceiveOutcome {
    Verified(Vec<u8>),
    Cancelled,
}

struct PeerSession<'a> {
    stream: &'a mut TcpStream,
    output: &'a mut File,
    availability: &'a mut [bool],
    choked: &'a mut bool,
    scheduler: &'a Arc<(Mutex<SchedulerState>, Condvar)>,
}

fn receive_piece_job(
    session: &mut PeerSession<'_>,
    torrent: &TorrentV1,
    job: &PieceJob,
    counters: &mut TransferCounters,
) -> Result<PieceReceiveOutcome> {
    if !session
        .availability
        .get(job.piece_index as usize)
        .copied()
        .unwrap_or(false)
    {
        bail!("peer does not advertise assigned piece {}", job.piece_index);
    }
    let mut pending = VecDeque::from(job.requests.clone());
    let mut inflight: HashMap<(u32, u32), BlockRequest> = HashMap::new();
    let mut availability_received = true;
    let piece_offset = u64::from(job.piece_index)
        .checked_mul(torrent.piece_length)
        .context("BT Piece offset overflow")?;
    let length = piece_length(torrent, job.piece_index as usize)?;
    session.output.seek(SeekFrom::Start(piece_offset))?;
    let mut piece_bytes = vec![0_u8; usize::try_from(length)?];
    session.output.read_exact(&mut piece_bytes)?;

    while !pending.is_empty() || !inflight.is_empty() {
        if piece_job_completed(session.scheduler, job.piece_index)? {
            cancel_inflight_requests(session.stream, &inflight, counters)?;
            return Ok(PieceReceiveOutcome::Cancelled);
        }
        if *session.choked {
            if !inflight.is_empty() {
                bail!("peer choked while pipelined block requests were outstanding");
            }
            wait_for_unchoke(session.stream, session.availability, session.choked)?;
        }
        while !*session.choked && inflight.len() < MAX_INFLIGHT_REQUESTS {
            let Some(request) = pending.pop_front() else {
                break;
            };
            let mut request_payload = Vec::with_capacity(12);
            request_payload.extend_from_slice(&request.piece_index.to_be_bytes());
            request_payload.extend_from_slice(&request.begin.to_be_bytes());
            request_payload.extend_from_slice(&request.length.to_be_bytes());
            send_message(session.stream, MESSAGE_REQUEST, &request_payload)?;
            counters.block_requests_sent += 1;
            if inflight
                .insert((request.piece_index, request.begin), request)
                .is_some()
            {
                bail!("duplicate pipelined BT block request");
            }
        }

        if inflight.is_empty() {
            continue;
        }
        let Some((message_id, payload)) = read_message(session.stream)? else {
            continue;
        };
        if message_id == MESSAGE_PIECE {
            if payload.len() < 8 {
                bail!("peer sent malformed PIECE message");
            }
            let piece_index = u32::from_be_bytes(payload[0..4].try_into().unwrap());
            let begin = u32::from_be_bytes(payload[4..8].try_into().unwrap());
            let block = &payload[8..];
            let request = inflight
                .remove(&(piece_index, begin))
                .context("peer returned a block that was not requested or was already received")?;
            if block.len() != request.length as usize {
                bail!("peer returned a block with the wrong length");
            }
            let relative = request
                .absolute_offset
                .checked_sub(piece_offset)
                .context("BT block precedes its Piece")?;
            let end = relative
                .checked_add(u64::from(request.length))
                .context("BT block range overflow")?;
            if end > length {
                bail!("peer returned a block outside its Piece");
            }
            piece_bytes[relative as usize..end as usize].copy_from_slice(block);
            counters.payload_bytes_received += block.len() as u64;
            continue;
        }
        process_control_message(
            message_id,
            &payload,
            session.availability,
            &mut availability_received,
            session.choked,
        )?;
        if *session.choked {
            bail!("peer choked while pipelined block requests were outstanding");
        }
    }
    let actual_sha1 = hex::encode(Sha1::digest(&piece_bytes));
    let expected_sha1 = &torrent.piece_sha1[job.piece_index as usize];
    if &actual_sha1 != expected_sha1 {
        bail!(
            "BT piece {} SHA-1 mismatch after transfer: expected {expected_sha1}, got {actual_sha1}",
            job.piece_index
        );
    }
    Ok(PieceReceiveOutcome::Verified(piece_bytes))
}

fn cancel_inflight_requests(
    stream: &mut TcpStream,
    inflight: &HashMap<(u32, u32), BlockRequest>,
    counters: &mut TransferCounters,
) -> Result<()> {
    for request in inflight.values() {
        let mut payload = Vec::with_capacity(12);
        payload.extend_from_slice(&request.piece_index.to_be_bytes());
        payload.extend_from_slice(&request.begin.to_be_bytes());
        payload.extend_from_slice(&request.length.to_be_bytes());
        send_message(stream, MESSAGE_CANCEL, &payload)?;
        counters.cancel_messages_sent += 1;
    }
    Ok(())
}

fn commit_verified_piece(
    output: &mut File,
    torrent: &TorrentV1,
    piece_index: u32,
    bytes: &[u8],
) -> Result<()> {
    let expected_length = piece_length(torrent, piece_index as usize)?;
    if bytes.len() as u64 != expected_length {
        bail!("verified BT Piece buffer has the wrong length");
    }
    let offset = u64::from(piece_index)
        .checked_mul(torrent.piece_length)
        .context("BT Piece offset overflow")?;
    output.seek(SeekFrom::Start(offset))?;
    output.write_all(bytes)?;
    Ok(())
}

fn verify_all_pieces(file: &mut File, torrent: &TorrentV1) -> Result<()> {
    for index in 0..torrent.piece_sha1.len() {
        verify_piece(file, torrent, index)?;
    }
    Ok(())
}

fn piece_length(torrent: &TorrentV1, index: usize) -> Result<u64> {
    let offset = (index as u64)
        .checked_mul(torrent.piece_length)
        .context("BT piece offset overflow")?;
    Ok(torrent
        .piece_length
        .min(torrent.total_length.saturating_sub(offset)))
}

fn piece_matches(file: &mut File, torrent: &TorrentV1, index: usize) -> Result<bool> {
    let offset = (index as u64)
        .checked_mul(torrent.piece_length)
        .context("BT piece offset overflow")?;
    let length = piece_length(torrent, index)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0_u8; usize::try_from(length)?];
    file.read_exact(&mut bytes)?;
    Ok(hex::encode(Sha1::digest(&bytes)) == torrent.piece_sha1[index])
}

fn verify_piece(file: &mut File, torrent: &TorrentV1, index: usize) -> Result<()> {
    if !piece_matches(file, torrent, index)? {
        let offset = (index as u64)
            .checked_mul(torrent.piece_length)
            .context("BT piece offset overflow")?;
        let length = piece_length(torrent, index)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0_u8; usize::try_from(length)?];
        file.read_exact(&mut bytes)?;
        let actual_sha1 = hex::encode(Sha1::digest(&bytes));
        bail!(
            "BT piece {index} SHA-1 mismatch after transfer: expected {}, got {actual_sha1}",
            torrent.piece_sha1[index]
        );
    }
    Ok(())
}
