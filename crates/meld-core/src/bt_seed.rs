use std::collections::BTreeSet;
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::bittorrent::{plan_v1_bridge, read_verified_chunk};
use crate::bt_peer::generate_peer_id;
use crate::bt_tracker::{
    BtTrackerLifecycleAttempt, SeedTrackerHeartbeat, start_seed_trackers, stop_seed_trackers,
};
use crate::{
    BtBridgeReport, IndexDb, REPORT_FORMAT, REPORT_VERSION, TargetDescriptor, TorrentV1,
    sha256_file,
};

const PROTOCOL_NAME: &[u8; 19] = b"BitTorrent protocol";
const HANDSHAKE_LENGTH: usize = 68;
const BLOCK_LENGTH: u32 = 16 * 1024;
const MAX_MESSAGE_LENGTH: u32 = 2 * 1024 * 1024;
const PEER_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_ACTIVE_UPLOAD_CONNECTIONS: usize = 4;

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
    #[serde(default)]
    pub concurrent_connection_limit: u64,
    #[serde(default)]
    pub peak_concurrent_connections: u64,
    #[serde(default)]
    pub upload_rate_limit_bytes_per_second: Option<u64>,
    #[serde(default)]
    pub upload_scheduler: String,
    #[serde(default)]
    pub upload_throttle_wait_micros: u64,
    pub source_verified: bool,
    #[serde(default)]
    pub shutdown_requested: bool,
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
    pub concurrent_connection_limit: u64,
    #[serde(default)]
    pub peak_concurrent_connections: u64,
    #[serde(default)]
    pub upload_rate_limit_bytes_per_second: Option<u64>,
    #[serde(default)]
    pub upload_scheduler: String,
    #[serde(default)]
    pub upload_throttle_wait_micros: u64,
    #[serde(default)]
    pub shutdown_requested: bool,
    #[serde(default)]
    pub tracker_announces: Vec<BtTrackerLifecycleAttempt>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BtSeedOptions {
    pub max_connections: Option<u64>,
    pub max_upload_bytes_per_second: Option<u64>,
}

struct BtFileSeedStart {
    source_sha256: String,
    local_peer_id: [u8; 20],
    options: BtSeedOptions,
}

struct BtFilePeerContext<'a> {
    torrent: &'a TorrentV1,
    source: &'a Path,
    local_peer_id: &'a [u8; 20],
    shutdown: Option<&'a AtomicBool>,
    uploaded_counter: Option<&'a AtomicU64>,
    upload_limiter: &'a UploadLimiter,
}

struct BtIndexSeedStart {
    plan: BtBridgeReport,
    local_peer_id: [u8; 20],
    options: BtSeedOptions,
}

struct BtIndexPeerContext<'a> {
    torrent: &'a TorrentV1,
    descriptor: &'a TargetDescriptor,
    available: &'a [bool],
    local_peer_id: &'a [u8; 20],
    shutdown: Option<&'a AtomicBool>,
    uploaded_counter: Option<&'a AtomicU64>,
    upload_limiter: &'a UploadLimiter,
}

#[derive(Debug, Default)]
struct BtSeedConnectionStats {
    successful_handshakes: u64,
    block_requests: u64,
    payload_bytes_sent: u64,
    cancel_messages_received: u64,
    upload_throttle_wait_micros: u64,
}

#[derive(Debug, Default)]
struct BtIndexSeedConnectionStats {
    successful_handshakes: u64,
    block_requests: u64,
    payload_bytes_sent: u64,
    on_demand_local_chunks_read: u64,
    on_demand_local_bytes_read: u64,
    cancel_messages_received: u64,
    upload_throttle_wait_micros: u64,
}

#[derive(Debug)]
struct UploadLimiter {
    rate_bytes_per_second: Option<u64>,
    state: Mutex<UploadLimiterState>,
    ready: Condvar,
}

#[derive(Debug)]
struct UploadLimiterState {
    next_ticket: u64,
    serving_ticket: u64,
    cancelled_tickets: BTreeSet<u64>,
    next_send: Instant,
}

impl UploadLimiter {
    fn new(rate_bytes_per_second: Option<u64>) -> Result<Self> {
        validate_upload_rate(rate_bytes_per_second)?;
        Ok(Self {
            rate_bytes_per_second,
            state: Mutex::new(UploadLimiterState {
                next_ticket: 0,
                serving_ticket: 0,
                cancelled_tickets: BTreeSet::new(),
                next_send: Instant::now(),
            }),
            ready: Condvar::new(),
        })
    }

    fn reserve(&self, bytes: u64, shutdown: Option<&AtomicBool>) -> Result<Option<u64>> {
        let Some(rate) = self.rate_bytes_per_second else {
            return Ok(Some(0));
        };
        let started = Instant::now();
        let mut state = self.state.lock().expect("upload limiter lock poisoned");
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        loop {
            if is_shutdown(shutdown) {
                cancel_upload_ticket(&mut state, ticket);
                self.ready.notify_all();
                return Ok(None);
            }
            if ticket != state.serving_ticket {
                state = self
                    .ready
                    .wait_timeout(state, SHUTDOWN_POLL_INTERVAL)
                    .expect("upload limiter lock poisoned")
                    .0;
                continue;
            }
            let now = Instant::now();
            if state.next_send > now {
                let wait = (state.next_send - now).min(SHUTDOWN_POLL_INTERVAL);
                state = self
                    .ready
                    .wait_timeout(state, wait)
                    .expect("upload limiter lock poisoned")
                    .0;
                continue;
            }
            let nanos = u128::from(bytes)
                .saturating_mul(1_000_000_000)
                .div_ceil(u128::from(rate))
                .max(1)
                .min(u128::from(u64::MAX));
            state.next_send = now + Duration::from_nanos(nanos as u64);
            advance_upload_ticket(&mut state);
            self.ready.notify_all();
            return Ok(Some(
                started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
            ));
        }
    }
}

fn validate_upload_rate(rate_bytes_per_second: Option<u64>) -> Result<()> {
    if rate_bytes_per_second == Some(0) {
        bail!("BT seed upload rate limit must be greater than zero");
    }
    Ok(())
}

fn upload_scheduler_name(rate_bytes_per_second: Option<u64>) -> &'static str {
    if rate_bytes_per_second.is_some() {
        "fifo-block-fair"
    } else {
        "unlimited"
    }
}

fn cancel_upload_ticket(state: &mut UploadLimiterState, ticket: u64) {
    if ticket == state.serving_ticket {
        advance_upload_ticket(state);
    } else if ticket > state.serving_ticket {
        state.cancelled_tickets.insert(ticket);
    }
}

fn advance_upload_ticket(state: &mut UploadLimiterState) {
    state.serving_ticket += 1;
    while state.cancelled_tickets.remove(&state.serving_ticket) {
        state.serving_ticket += 1;
    }
}

pub fn serve_v1_file(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    bind: SocketAddr,
    allow_non_loopback: bool,
    max_connections: Option<u64>,
) -> Result<BtSeedReport> {
    serve_v1_file_with_options(
        torrent,
        descriptor,
        source,
        bind,
        allow_non_loopback,
        BtSeedOptions {
            max_connections,
            max_upload_bytes_per_second: None,
        },
    )
}

pub fn serve_v1_file_with_options(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    bind: SocketAddr,
    allow_non_loopback: bool,
    options: BtSeedOptions,
) -> Result<BtSeedReport> {
    validate_upload_rate(options.max_upload_bytes_per_second)?;
    serve_v1_file_controlled(
        torrent,
        descriptor,
        source,
        bind,
        allow_non_loopback,
        options,
        None,
    )
}

pub fn serve_v1_file_until_shutdown(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    bind: SocketAddr,
    allow_non_loopback: bool,
    max_connections: Option<u64>,
    shutdown: &AtomicBool,
) -> Result<BtSeedReport> {
    serve_v1_file_until_shutdown_with_options(
        torrent,
        descriptor,
        source,
        bind,
        allow_non_loopback,
        BtSeedOptions {
            max_connections,
            max_upload_bytes_per_second: None,
        },
        shutdown,
    )
}

pub fn serve_v1_file_until_shutdown_with_options(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    bind: SocketAddr,
    allow_non_loopback: bool,
    options: BtSeedOptions,
    shutdown: &AtomicBool,
) -> Result<BtSeedReport> {
    serve_v1_file_controlled(
        torrent,
        descriptor,
        source,
        bind,
        allow_non_loopback,
        options,
        Some(shutdown),
    )
}

fn serve_v1_file_controlled(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    bind: SocketAddr,
    allow_non_loopback: bool,
    options: BtSeedOptions,
    shutdown: Option<&AtomicBool>,
) -> Result<BtSeedReport> {
    validate_upload_rate(options.max_upload_bytes_per_second)?;
    if !bind.ip().is_loopback() && !allow_non_loopback {
        bail!("refusing non-loopback BT seed bind without --allow-non-loopback");
    }
    let source_sha256 = validate_seed_source(torrent, descriptor, source)?;
    let listener = TcpListener::bind(bind).with_context(|| format!("bind BT seed {bind}"))?;
    let bind = listener.local_addr()?;
    let local_peer_id = generate_peer_id()?;
    let (active_trackers, mut tracker_announces) =
        start_seed_trackers(torrent, &local_peer_id, bind.port());
    let uploaded = Arc::new(AtomicU64::new(0));
    let tracker_heartbeat = SeedTrackerHeartbeat::start(
        &active_trackers,
        torrent,
        &local_peer_id,
        bind.port(),
        Arc::clone(&uploaded),
    );
    let result = serve_v1_file_listener_verified(
        listener,
        torrent,
        source,
        BtFileSeedStart {
            source_sha256,
            local_peer_id,
            options,
        },
        shutdown,
        Some(uploaded.as_ref()),
    );
    if let Some(tracker_heartbeat) = tracker_heartbeat {
        tracker_announces.extend(tracker_heartbeat.finish());
    }
    tracker_announces.extend(stop_seed_trackers(
        &active_trackers,
        torrent,
        &local_peer_id,
        bind.port(),
        uploaded.load(Ordering::Relaxed),
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
    serve_v1_file_listener_with_options(
        listener,
        torrent,
        descriptor,
        source,
        BtSeedOptions {
            max_connections,
            max_upload_bytes_per_second: None,
        },
    )
}

pub fn serve_v1_file_listener_with_options(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    options: BtSeedOptions,
) -> Result<BtSeedReport> {
    validate_upload_rate(options.max_upload_bytes_per_second)?;
    let source_sha256 = validate_seed_source(torrent, descriptor, source)?;
    let local_peer_id = generate_peer_id()?;
    serve_v1_file_listener_verified(
        listener,
        torrent,
        source,
        BtFileSeedStart {
            source_sha256,
            local_peer_id,
            options,
        },
        None,
        None,
    )
}

pub fn serve_v1_file_listener_until_shutdown(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    max_connections: Option<u64>,
    shutdown: &AtomicBool,
) -> Result<BtSeedReport> {
    serve_v1_file_listener_until_shutdown_with_options(
        listener,
        torrent,
        descriptor,
        source,
        BtSeedOptions {
            max_connections,
            max_upload_bytes_per_second: None,
        },
        shutdown,
    )
}

pub fn serve_v1_file_listener_until_shutdown_with_options(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    source: &Path,
    options: BtSeedOptions,
    shutdown: &AtomicBool,
) -> Result<BtSeedReport> {
    validate_upload_rate(options.max_upload_bytes_per_second)?;
    let source_sha256 = validate_seed_source(torrent, descriptor, source)?;
    let local_peer_id = generate_peer_id()?;
    serve_v1_file_listener_verified(
        listener,
        torrent,
        source,
        BtFileSeedStart {
            source_sha256,
            local_peer_id,
            options,
        },
        Some(shutdown),
        None,
    )
}

fn serve_v1_file_listener_verified(
    listener: TcpListener,
    torrent: &TorrentV1,
    source: &Path,
    start: BtFileSeedStart,
    shutdown: Option<&AtomicBool>,
    uploaded_counter: Option<&AtomicU64>,
) -> Result<BtSeedReport> {
    let upload_limiter = UploadLimiter::new(start.options.max_upload_bytes_per_second)?;
    let bind = listener.local_addr()?;
    if shutdown.is_some() {
        listener.set_nonblocking(true)?;
    }
    let mut report = BtSeedReport {
        report_format: REPORT_FORMAT.to_owned(),
        report_version: REPORT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        bind,
        source: source.to_path_buf(),
        info_hash_sha1: torrent.info_hash_sha1.clone(),
        source_sha256: start.source_sha256,
        advertised_pieces: torrent.piece_sha1.len() as u64,
        connections: 0,
        successful_handshakes: 0,
        block_requests: 0,
        payload_bytes_sent: 0,
        cancel_messages_received: 0,
        protocol_errors: 0,
        concurrent_connection_limit: MAX_ACTIVE_UPLOAD_CONNECTIONS as u64,
        peak_concurrent_connections: 0,
        upload_rate_limit_bytes_per_second: start.options.max_upload_bytes_per_second,
        upload_scheduler: upload_scheduler_name(start.options.max_upload_bytes_per_second)
            .to_owned(),
        upload_throttle_wait_micros: 0,
        source_verified: true,
        shutdown_requested: false,
        tracker_announces: Vec::new(),
    };

    let mut fatal_error = None;
    thread::scope(|scope| -> Result<()> {
        let (completed_tx, completed_rx) = mpsc::channel();
        let mut active = 0_usize;
        loop {
            while let Ok(result) = completed_rx.try_recv() {
                active -= 1;
                merge_file_connection(
                    &mut report,
                    result,
                    start.options.max_connections,
                    &mut fatal_error,
                );
            }
            if fatal_error.is_some()
                || is_shutdown(shutdown)
                || start
                    .options
                    .max_connections
                    .is_some_and(|limit| report.connections >= limit)
            {
                break;
            }
            if active >= MAX_ACTIVE_UPLOAD_CONNECTIONS {
                let result = completed_rx.recv().context("wait for BT seed worker")?;
                active -= 1;
                merge_file_connection(
                    &mut report,
                    result,
                    start.options.max_connections,
                    &mut fatal_error,
                );
                continue;
            }
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(ACCEPT_POLL_INTERVAL);
                    continue;
                }
                Err(error) => return Err(error).context("accept BT seed peer"),
            };
            report.connections += 1;
            active += 1;
            report.peak_concurrent_connections =
                report.peak_concurrent_connections.max(active as u64);
            stream.set_read_timeout(Some(if shutdown.is_some() {
                SHUTDOWN_POLL_INTERVAL
            } else {
                PEER_TIMEOUT
            }))?;
            stream.set_write_timeout(Some(PEER_TIMEOUT))?;
            let completed_tx = completed_tx.clone();
            let upload_limiter = &upload_limiter;
            let local_peer_id = &start.local_peer_id;
            scope.spawn(move || {
                let mut stats = BtSeedConnectionStats::default();
                let error = serve_peer(
                    &mut stream,
                    &mut stats,
                    BtFilePeerContext {
                        torrent,
                        source,
                        local_peer_id,
                        shutdown,
                        uploaded_counter,
                        upload_limiter,
                    },
                )
                .err();
                let _ = completed_tx.send((stats, error));
            });
        }
        drop(completed_tx);
        while active > 0 {
            let result = completed_rx
                .recv()
                .context("join remaining BT seed worker")?;
            active -= 1;
            merge_file_connection(
                &mut report,
                result,
                start.options.max_connections,
                &mut fatal_error,
            );
        }
        Ok(())
    })?;
    if let Some(error) = fatal_error {
        return Err(error);
    }
    report.shutdown_requested = is_shutdown(shutdown);
    Ok(report)
}

pub fn serve_v1_index(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    bind: SocketAddr,
    allow_non_loopback: bool,
    max_connections: Option<u64>,
) -> Result<BtIndexSeedReport> {
    serve_v1_index_with_options(
        torrent,
        descriptor,
        index,
        bind,
        allow_non_loopback,
        BtSeedOptions {
            max_connections,
            max_upload_bytes_per_second: None,
        },
    )
}

pub fn serve_v1_index_with_options(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    bind: SocketAddr,
    allow_non_loopback: bool,
    options: BtSeedOptions,
) -> Result<BtIndexSeedReport> {
    validate_upload_rate(options.max_upload_bytes_per_second)?;
    serve_v1_index_controlled(
        torrent,
        descriptor,
        index,
        bind,
        allow_non_loopback,
        options,
        None,
    )
}

pub fn serve_v1_index_until_shutdown(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    bind: SocketAddr,
    allow_non_loopback: bool,
    max_connections: Option<u64>,
    shutdown: &AtomicBool,
) -> Result<BtIndexSeedReport> {
    serve_v1_index_until_shutdown_with_options(
        torrent,
        descriptor,
        index,
        bind,
        allow_non_loopback,
        BtSeedOptions {
            max_connections,
            max_upload_bytes_per_second: None,
        },
        shutdown,
    )
}

pub fn serve_v1_index_until_shutdown_with_options(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    bind: SocketAddr,
    allow_non_loopback: bool,
    options: BtSeedOptions,
    shutdown: &AtomicBool,
) -> Result<BtIndexSeedReport> {
    serve_v1_index_controlled(
        torrent,
        descriptor,
        index,
        bind,
        allow_non_loopback,
        options,
        Some(shutdown),
    )
}

fn serve_v1_index_controlled(
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    bind: SocketAddr,
    allow_non_loopback: bool,
    options: BtSeedOptions,
    shutdown: Option<&AtomicBool>,
) -> Result<BtIndexSeedReport> {
    validate_upload_rate(options.max_upload_bytes_per_second)?;
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
    let uploaded = Arc::new(AtomicU64::new(0));
    let tracker_heartbeat = SeedTrackerHeartbeat::start(
        &active_trackers,
        torrent,
        &local_peer_id,
        bind.port(),
        Arc::clone(&uploaded),
    );
    let result = serve_v1_index_listener_preflighted(
        listener,
        torrent,
        descriptor,
        index.path(),
        BtIndexSeedStart {
            plan,
            local_peer_id,
            options,
        },
        shutdown,
        Some(uploaded.as_ref()),
    );
    if let Some(tracker_heartbeat) = tracker_heartbeat {
        tracker_announces.extend(tracker_heartbeat.finish());
    }
    tracker_announces.extend(stop_seed_trackers(
        &active_trackers,
        torrent,
        &local_peer_id,
        bind.port(),
        uploaded.load(Ordering::Relaxed),
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
    max_connections: Option<u64>,
) -> Result<BtIndexSeedReport> {
    serve_v1_index_listener_with_options(
        listener,
        torrent,
        descriptor,
        index,
        BtSeedOptions {
            max_connections,
            max_upload_bytes_per_second: None,
        },
    )
}

pub fn serve_v1_index_listener_with_options(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    options: BtSeedOptions,
) -> Result<BtIndexSeedReport> {
    validate_upload_rate(options.max_upload_bytes_per_second)?;
    let plan = plan_v1_bridge(torrent, descriptor, index)?;
    if !plan.pieces.iter().any(|piece| piece.fully_local) {
        bail!("authorized index cannot reconstruct any verified torrent Piece");
    }
    let local_peer_id = generate_peer_id()?;
    serve_v1_index_listener_preflighted(
        listener,
        torrent,
        descriptor,
        index.path(),
        BtIndexSeedStart {
            plan,
            local_peer_id,
            options,
        },
        None,
        None,
    )
}

fn serve_v1_index_listener_preflighted(
    listener: TcpListener,
    torrent: &TorrentV1,
    descriptor: &TargetDescriptor,
    index_db: &Path,
    start: BtIndexSeedStart,
    shutdown: Option<&AtomicBool>,
    uploaded_counter: Option<&AtomicU64>,
) -> Result<BtIndexSeedReport> {
    let upload_limiter = UploadLimiter::new(start.options.max_upload_bytes_per_second)?;
    let available: Vec<bool> = start
        .plan
        .pieces
        .iter()
        .map(|piece| piece.fully_local)
        .collect();
    let bind = listener.local_addr()?;
    if shutdown.is_some() {
        listener.set_nonblocking(true)?;
    }
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
        concurrent_connection_limit: MAX_ACTIVE_UPLOAD_CONNECTIONS as u64,
        peak_concurrent_connections: 0,
        upload_rate_limit_bytes_per_second: start.options.max_upload_bytes_per_second,
        upload_scheduler: upload_scheduler_name(start.options.max_upload_bytes_per_second)
            .to_owned(),
        upload_throttle_wait_micros: 0,
        shutdown_requested: false,
        tracker_announces: Vec::new(),
    };

    let mut fatal_error = None;
    thread::scope(|scope| -> Result<()> {
        let (completed_tx, completed_rx) = mpsc::channel();
        let mut active = 0_usize;
        loop {
            while let Ok(result) = completed_rx.try_recv() {
                active -= 1;
                merge_index_connection(
                    &mut report,
                    result,
                    start.options.max_connections,
                    &mut fatal_error,
                );
            }
            if fatal_error.is_some()
                || is_shutdown(shutdown)
                || start
                    .options
                    .max_connections
                    .is_some_and(|limit| report.connections >= limit)
            {
                break;
            }
            if active >= MAX_ACTIVE_UPLOAD_CONNECTIONS {
                let result = completed_rx
                    .recv()
                    .context("wait for BT index seed worker")?;
                active -= 1;
                merge_index_connection(
                    &mut report,
                    result,
                    start.options.max_connections,
                    &mut fatal_error,
                );
                continue;
            }
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(ACCEPT_POLL_INTERVAL);
                    continue;
                }
                Err(error) => return Err(error).context("accept BT index seed peer"),
            };
            report.connections += 1;
            active += 1;
            report.peak_concurrent_connections =
                report.peak_concurrent_connections.max(active as u64);
            stream.set_read_timeout(Some(if shutdown.is_some() {
                SHUTDOWN_POLL_INTERVAL
            } else {
                PEER_TIMEOUT
            }))?;
            stream.set_write_timeout(Some(PEER_TIMEOUT))?;
            let completed_tx = completed_tx.clone();
            let local_peer_id = &start.local_peer_id;
            let available = &available;
            let upload_limiter = &upload_limiter;
            scope.spawn(move || {
                let mut stats = BtIndexSeedConnectionStats::default();
                let error = IndexDb::open(index_db)
                    .and_then(|index| {
                        serve_index_peer(
                            &mut stream,
                            &index,
                            &mut stats,
                            BtIndexPeerContext {
                                torrent,
                                descriptor,
                                available,
                                local_peer_id,
                                shutdown,
                                uploaded_counter,
                                upload_limiter,
                            },
                        )
                    })
                    .err();
                let _ = completed_tx.send((stats, error));
            });
        }
        drop(completed_tx);
        while active > 0 {
            let result = completed_rx
                .recv()
                .context("join remaining BT index seed worker")?;
            active -= 1;
            merge_index_connection(
                &mut report,
                result,
                start.options.max_connections,
                &mut fatal_error,
            );
        }
        Ok(())
    })?;
    if let Some(error) = fatal_error {
        return Err(error);
    }
    report.shutdown_requested = is_shutdown(shutdown);
    Ok(report)
}

fn merge_file_connection(
    report: &mut BtSeedReport,
    (stats, error): (BtSeedConnectionStats, Option<anyhow::Error>),
    max_connections: Option<u64>,
    fatal_error: &mut Option<anyhow::Error>,
) {
    report.successful_handshakes += stats.successful_handshakes;
    report.block_requests += stats.block_requests;
    report.payload_bytes_sent += stats.payload_bytes_sent;
    report.cancel_messages_received += stats.cancel_messages_received;
    report.upload_throttle_wait_micros += stats.upload_throttle_wait_micros;
    if let Some(error) = error {
        report.protocol_errors += 1;
        if max_connections == Some(1) && fatal_error.is_none() {
            *fatal_error = Some(error);
        }
    }
}

fn merge_index_connection(
    report: &mut BtIndexSeedReport,
    (stats, error): (BtIndexSeedConnectionStats, Option<anyhow::Error>),
    max_connections: Option<u64>,
    fatal_error: &mut Option<anyhow::Error>,
) {
    report.successful_handshakes += stats.successful_handshakes;
    report.block_requests += stats.block_requests;
    report.payload_bytes_sent += stats.payload_bytes_sent;
    report.on_demand_local_chunks_read += stats.on_demand_local_chunks_read;
    report.on_demand_local_bytes_read += stats.on_demand_local_bytes_read;
    report.cancel_messages_received += stats.cancel_messages_received;
    report.upload_throttle_wait_micros += stats.upload_throttle_wait_micros;
    if let Some(error) = error {
        report.protocol_errors += 1;
        if max_connections == Some(1) && fatal_error.is_none() {
            *fatal_error = Some(error);
        }
    }
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
    stats: &mut BtSeedConnectionStats,
    context: BtFilePeerContext<'_>,
) -> Result<()> {
    read_and_reply_handshake(
        stream,
        &context.torrent.info_hash_sha1,
        context.local_peer_id,
        context.shutdown,
    )?;
    stats.successful_handshakes += 1;
    send_message(
        stream,
        MESSAGE_BITFIELD,
        &complete_bitfield(context.torrent.piece_sha1.len()),
    )?;
    let mut file = File::open(context.source)?;
    let mut unchoked = false;
    loop {
        if is_shutdown(context.shutdown) {
            break;
        }
        let message = match read_message(stream, context.shutdown) {
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
                let (piece, begin, length) = parse_block_message(&payload, context.torrent)?;
                let absolute = u64::from(piece)
                    .checked_mul(context.torrent.piece_length)
                    .and_then(|offset| offset.checked_add(u64::from(begin)))
                    .context("BT seed request offset overflow")?;
                file.seek(SeekFrom::Start(absolute))?;
                let mut block = vec![0_u8; length as usize];
                file.read_exact(&mut block)?;
                let mut response = Vec::with_capacity(8 + block.len());
                response.extend_from_slice(&piece.to_be_bytes());
                response.extend_from_slice(&begin.to_be_bytes());
                response.extend_from_slice(&block);
                let Some(wait_micros) = context
                    .upload_limiter
                    .reserve(u64::from(length), context.shutdown)?
                else {
                    break;
                };
                send_message(stream, MESSAGE_PIECE, &response)?;
                stats.block_requests += 1;
                stats.payload_bytes_sent += u64::from(length);
                stats.upload_throttle_wait_micros += wait_micros;
                if let Some(uploaded_counter) = context.uploaded_counter {
                    uploaded_counter.fetch_add(u64::from(length), Ordering::Relaxed);
                }
            }
            MESSAGE_CANCEL => {
                parse_block_message(&payload, context.torrent)?;
                stats.cancel_messages_received += 1;
            }
            MESSAGE_CHOKE => expect_empty(&payload, "choke")?,
            _ => {}
        }
    }
    Ok(())
}

fn serve_index_peer(
    stream: &mut TcpStream,
    index: &IndexDb,
    stats: &mut BtIndexSeedConnectionStats,
    context: BtIndexPeerContext<'_>,
) -> Result<()> {
    read_and_reply_handshake(
        stream,
        &context.torrent.info_hash_sha1,
        context.local_peer_id,
        context.shutdown,
    )?;
    stats.successful_handshakes += 1;
    send_message(
        stream,
        MESSAGE_BITFIELD,
        &availability_bitfield(context.available),
    )?;
    let mut unchoked = false;
    let mut cached_piece: Option<(u32, Vec<u8>)> = None;
    loop {
        if is_shutdown(context.shutdown) {
            break;
        }
        let message = match read_message(stream, context.shutdown) {
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
                let (piece, begin, length) = parse_block_message(&payload, context.torrent)?;
                if !context.available[piece as usize] {
                    bail!("BT peer requested a Piece not advertised by the local index");
                }
                if cached_piece.as_ref().map(|cached| cached.0) != Some(piece) {
                    let (bytes, chunks_read, bytes_read) = reconstruct_index_piece(
                        context.torrent,
                        context.descriptor,
                        index,
                        piece as usize,
                    )?;
                    stats.on_demand_local_chunks_read += chunks_read;
                    stats.on_demand_local_bytes_read += bytes_read;
                    cached_piece = Some((piece, bytes));
                }
                let piece_bytes = &cached_piece.as_ref().context("missing cached Piece")?.1;
                let block = &piece_bytes[begin as usize..(begin + length) as usize];
                let mut response = Vec::with_capacity(8 + block.len());
                response.extend_from_slice(&piece.to_be_bytes());
                response.extend_from_slice(&begin.to_be_bytes());
                response.extend_from_slice(block);
                let Some(wait_micros) = context
                    .upload_limiter
                    .reserve(u64::from(length), context.shutdown)?
                else {
                    break;
                };
                send_message(stream, MESSAGE_PIECE, &response)?;
                stats.block_requests += 1;
                stats.payload_bytes_sent += u64::from(length);
                stats.upload_throttle_wait_micros += wait_micros;
                if let Some(uploaded_counter) = context.uploaded_counter {
                    uploaded_counter.fetch_add(u64::from(length), Ordering::Relaxed);
                }
            }
            MESSAGE_CANCEL => {
                parse_block_message(&payload, context.torrent)?;
                stats.cancel_messages_received += 1;
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
    shutdown: Option<&AtomicBool>,
) -> Result<()> {
    let mut handshake = [0_u8; HANDSHAKE_LENGTH];
    read_exact_interruptible(stream, &mut handshake, shutdown)?;
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

fn read_message(
    stream: &mut TcpStream,
    shutdown: Option<&AtomicBool>,
) -> Result<Option<(u8, Vec<u8>)>> {
    let mut length = [0_u8; 4];
    read_exact_interruptible(stream, &mut length, shutdown)?;
    let length = u32::from_be_bytes(length);
    if length == 0 {
        return Ok(None);
    }
    if length > MAX_MESSAGE_LENGTH {
        bail!("BT seed peer sent an oversized message");
    }
    let mut message = vec![0_u8; length as usize];
    read_exact_interruptible(stream, &mut message, shutdown)?;
    Ok(Some((message[0], message[1..].to_vec())))
}

fn read_exact_interruptible(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    shutdown: Option<&AtomicBool>,
) -> Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        match stream.read(&mut buffer[filled..]) {
            Ok(0) => {
                return Err(std::io::Error::from(ErrorKind::UnexpectedEof).into());
            }
            Ok(length) => filled += length,
            Err(error)
                if shutdown.is_some()
                    && matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
                    && !is_shutdown(shutdown) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
                | ErrorKind::Interrupted
                | ErrorKind::TimedOut
                | ErrorKind::WouldBlock
        )
    })
}

fn is_shutdown(shutdown: Option<&AtomicBool>) -> bool {
    shutdown.is_some_and(|shutdown| shutdown.load(Ordering::Relaxed))
}
