use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{IndexDb, TargetDescriptor};

const PROTOCOL_MAGIC: &[u8; 5] = b"SMLD2";
const STATUS_OK: u8 = 0;
const STATUS_UNAVAILABLE: u8 = 1;
const MAX_CHUNK_BYTES: u32 = 16 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServeReport {
    pub bound_address: SocketAddr,
    pub source_directory: PathBuf,
    pub connections: u64,
    pub requests_received: u64,
    pub chunks_sent: u64,
    pub bytes_sent: u64,
    pub unavailable_requests: u64,
    pub connection_errors: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FetchReport {
    pub peer: SocketAddr,
    pub output_directory: PathBuf,
    pub missing_chunk_occurrences: u64,
    pub unique_chunks_needed: u64,
    pub chunks_fetched: u64,
    pub bytes_fetched: u64,
    pub existing_verified_chunks: u64,
}

#[derive(Debug, Default)]
struct ConnectionReport {
    requests: u64,
    sent: u64,
    bytes: u64,
    unavailable: u64,
}

pub fn serve_chunk_directory(
    bind: SocketAddr,
    source_directory: &Path,
    allow_non_loopback: bool,
    max_requests: Option<u64>,
) -> Result<ServeReport> {
    if !bind.ip().is_loopback() && !allow_non_loopback {
        bail!(
            "refusing non-loopback bind {bind}; pass --allow-non-loopback only on a trusted network"
        );
    }
    if !source_directory.is_dir() {
        bail!(
            "chunk source is not a directory: {}",
            source_directory.display()
        );
    }
    let listener = TcpListener::bind(bind).with_context(|| format!("bind chunk server {bind}"))?;
    serve_chunk_listener(listener, source_directory, max_requests)
}

pub fn serve_chunk_listener(
    listener: TcpListener,
    source_directory: &Path,
    max_requests: Option<u64>,
) -> Result<ServeReport> {
    if matches!(max_requests, Some(0)) {
        bail!("max_requests must be greater than zero when specified");
    }
    let source_directory = source_directory
        .canonicalize()
        .with_context(|| format!("canonicalize chunk source {}", source_directory.display()))?;
    let bound_address = listener.local_addr()?;
    let mut report = ServeReport {
        bound_address,
        source_directory: source_directory.clone(),
        connections: 0,
        requests_received: 0,
        chunks_sent: 0,
        bytes_sent: 0,
        unavailable_requests: 0,
        connection_errors: 0,
    };

    loop {
        if max_requests.is_some_and(|maximum| report.requests_received >= maximum) {
            break;
        }
        let (mut stream, _) = listener.accept().context("accept chunk client")?;
        report.connections += 1;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let remaining = max_requests.map(|maximum| maximum - report.requests_received);
        match serve_connection(&mut stream, &source_directory, remaining) {
            Ok(connection) => {
                report.requests_received += connection.requests;
                report.chunks_sent += connection.sent;
                report.bytes_sent += connection.bytes;
                report.unavailable_requests += connection.unavailable;
            }
            Err(_) => report.connection_errors += 1,
        }
    }
    Ok(report)
}

pub fn fetch_missing_chunks(
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    peer: SocketAddr,
    output_directory: &Path,
) -> Result<FetchReport> {
    descriptor.validate()?;
    index.ensure_profile(descriptor.profile)?;
    std::fs::create_dir_all(output_directory).with_context(|| {
        format!(
            "create fetched chunk directory {}",
            output_directory.display()
        )
    })?;

    let mut unique = HashSet::new();
    let mut needed = Vec::new();
    let mut missing_chunk_occurrences = 0_u64;
    for chunk in &descriptor.chunks {
        if index.lookup_chunk(&chunk.hash, chunk.length)?.is_some() {
            continue;
        }
        missing_chunk_occurrences += 1;
        if unique.insert((chunk.hash.clone(), chunk.length)) {
            needed.push((chunk.hash.clone(), chunk.length));
        }
    }

    let mut pending = Vec::new();
    let mut existing_verified_chunks = 0_u64;
    for (hash, length) in needed {
        let destination = chunk_path(output_directory, &hash);
        if destination.exists() {
            let existing = std::fs::read(&destination)
                .with_context(|| format!("read existing chunk {}", destination.display()))?;
            verify_chunk_bytes(&existing, &hash, length)?;
            existing_verified_chunks += 1;
        } else {
            pending.push((hash, length));
        }
    }

    let unique_chunks_needed = pending.len() as u64 + existing_verified_chunks;
    let mut chunks_fetched = 0_u64;
    let mut bytes_fetched = 0_u64;
    if !pending.is_empty() {
        let mut stream = TcpStream::connect_timeout(&peer, IO_TIMEOUT)
            .with_context(|| format!("connect to chunk peer {peer}"))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.write_all(PROTOCOL_MAGIC)?;

        for (hash, length) in pending {
            let hash_bytes = hex::decode(&hash).context("decode requested chunk hash")?;
            if hash_bytes.len() != 32 {
                bail!("chunk hash is not 32 bytes: {hash}");
            }
            stream.write_all(&hash_bytes)?;
            stream.write_all(&length.to_be_bytes())?;
            stream.flush()?;

            let mut status = [0_u8; 1];
            stream.read_exact(&mut status)?;
            let mut response_length = [0_u8; 4];
            stream.read_exact(&mut response_length)?;
            let response_length = u32::from_be_bytes(response_length);
            if status[0] != STATUS_OK {
                bail!("peer {peer} could not provide chunk {hash}");
            }
            if response_length != length || response_length > MAX_CHUNK_BYTES {
                bail!(
                    "peer {peer} returned invalid length {response_length} for chunk {hash}; expected {length}"
                );
            }
            let mut bytes = vec![0_u8; response_length as usize];
            stream.read_exact(&mut bytes)?;
            verify_chunk_bytes(&bytes, &hash, length)?;
            persist_chunk(output_directory, &hash, &bytes)?;
            chunks_fetched += 1;
            bytes_fetched += bytes.len() as u64;
        }
        let _ = stream.shutdown(Shutdown::Both);
    }

    Ok(FetchReport {
        peer,
        output_directory: output_directory.to_path_buf(),
        missing_chunk_occurrences,
        unique_chunks_needed,
        chunks_fetched,
        bytes_fetched,
        existing_verified_chunks,
    })
}

fn serve_connection(
    stream: &mut TcpStream,
    source_directory: &Path,
    request_limit: Option<u64>,
) -> Result<ConnectionReport> {
    let mut magic = [0_u8; PROTOCOL_MAGIC.len()];
    if !read_exact_or_eof(stream, &mut magic)? {
        return Ok(ConnectionReport::default());
    }
    if &magic != PROTOCOL_MAGIC {
        bail!("invalid ShardMeld protocol magic");
    }

    let mut report = ConnectionReport::default();
    loop {
        if request_limit.is_some_and(|limit| report.requests >= limit) {
            break;
        }
        let mut hash_bytes = [0_u8; 32];
        if !read_exact_or_eof(stream, &mut hash_bytes)? {
            break;
        }
        let mut length_bytes = [0_u8; 4];
        stream.read_exact(&mut length_bytes)?;
        let requested_length = u32::from_be_bytes(length_bytes);
        let hash = hex::encode(hash_bytes);
        report.requests += 1;

        let response = load_verified_chunk(source_directory, &hash, requested_length);
        match response {
            Ok(bytes) => {
                stream.write_all(&[STATUS_OK])?;
                stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
                stream.write_all(&bytes)?;
                stream.flush()?;
                report.sent += 1;
                report.bytes += bytes.len() as u64;
            }
            Err(_) => {
                stream.write_all(&[STATUS_UNAVAILABLE])?;
                stream.write_all(&0_u32.to_be_bytes())?;
                stream.flush()?;
                report.unavailable += 1;
            }
        }
    }
    Ok(report)
}

fn load_verified_chunk(source_directory: &Path, hash: &str, length: u32) -> Result<Vec<u8>> {
    if length == 0 || length > MAX_CHUNK_BYTES {
        bail!("invalid requested chunk length {length}");
    }
    let path = chunk_path(source_directory, hash);
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("chunk unavailable: {}", path.display()))?;
    if metadata.len() != length as u64 {
        bail!("chunk length mismatch");
    }
    let bytes = std::fs::read(&path)?;
    verify_chunk_bytes(&bytes, hash, length)?;
    Ok(bytes)
}

fn verify_chunk_bytes(bytes: &[u8], expected_hash: &str, expected_length: u32) -> Result<()> {
    if bytes.len() != expected_length as usize {
        bail!(
            "chunk length mismatch: expected {expected_length}, got {}",
            bytes.len()
        );
    }
    let actual = blake3::hash(bytes).to_hex().to_string();
    if actual != expected_hash {
        bail!("chunk BLAKE3 mismatch: expected {expected_hash}, got {actual}");
    }
    Ok(())
}

fn persist_chunk(output_directory: &Path, hash: &str, bytes: &[u8]) -> Result<()> {
    let destination = chunk_path(output_directory, hash);
    let temporary = tempfile::Builder::new()
        .prefix(".shardmeld-chunk-")
        .suffix(".partial")
        .tempfile_in(output_directory)?;
    temporary.as_file().write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(&destination)
        .map_err(|error| error.error)
        .with_context(|| format!("persist fetched chunk {}", destination.display()))?;
    Ok(())
}

fn chunk_path(directory: &Path, hash: &str) -> PathBuf {
    directory.join(format!("{hash}.chunk"))
}

fn read_exact_or_eof(reader: &mut impl Read, buffer: &mut [u8]) -> Result<bool> {
    if buffer.is_empty() {
        return Ok(true);
    }
    match reader.read(&mut buffer[..1]) {
        Ok(0) => Ok(false),
        Ok(1) => {
            reader.read_exact(&mut buffer[1..])?;
            Ok(true)
        }
        Ok(_) => unreachable!("one-byte read returned more than one byte"),
        Err(error) if error.kind() == ErrorKind::Interrupted => read_exact_or_eof(reader, buffer),
        Err(error) => Err(error.into()),
    }
}
