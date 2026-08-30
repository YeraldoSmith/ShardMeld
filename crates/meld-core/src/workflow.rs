use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{IndexDb, IndexStats, SourceLocation, TargetDescriptor, sha256_file};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanChunk {
    pub offset: u64,
    pub length: u32,
    pub hash: String,
    pub local_source: Option<SourceLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompareReport {
    pub target_name: String,
    pub target_bytes: u64,
    pub target_sha256: String,
    pub target_chunks: u64,
    pub matched_chunks: u64,
    pub missing_chunks: u64,
    pub local_reusable_bytes: u64,
    pub missing_payload_bytes: u64,
    pub reuse_ratio: f64,
    pub index: IndexStats,
    pub plan: Vec<PlanChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageMissingReport {
    pub output_directory: PathBuf,
    pub missing_chunk_occurrences: u64,
    pub missing_payload_bytes: u64,
    pub unique_chunk_files_written: u64,
    pub unique_chunk_file_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RebuildReport {
    pub output: PathBuf,
    pub target_bytes: u64,
    pub local_bytes_read: u64,
    pub missing_source_bytes_read: u64,
    pub local_chunk_occurrences: u64,
    pub missing_chunk_occurrences: u64,
    pub output_sha256: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyReport {
    pub file: PathBuf,
    pub expected_sha256: String,
    pub actual_sha256: String,
    pub expected_bytes: u64,
    pub actual_bytes: u64,
    pub verified: bool,
}

pub fn compare_descriptor(descriptor: &TargetDescriptor, index: &IndexDb) -> Result<CompareReport> {
    descriptor.validate()?;
    index.ensure_profile(descriptor.profile)?;
    let mut plan = Vec::with_capacity(descriptor.chunks.len());
    let mut matched_chunks = 0_u64;
    let mut local_reusable_bytes = 0_u64;
    for chunk in &descriptor.chunks {
        let local_source = index.lookup_chunk(&chunk.hash, chunk.length)?;
        if local_source.is_some() {
            matched_chunks += 1;
            local_reusable_bytes += chunk.length as u64;
        }
        plan.push(PlanChunk {
            offset: chunk.offset,
            length: chunk.length,
            hash: chunk.hash.clone(),
            local_source,
        });
    }
    let missing_payload_bytes = descriptor.target.size - local_reusable_bytes;
    let target_chunks = descriptor.chunks.len() as u64;
    Ok(CompareReport {
        target_name: descriptor.target.name.clone(),
        target_bytes: descriptor.target.size,
        target_sha256: descriptor.target.sha256.clone(),
        target_chunks,
        matched_chunks,
        missing_chunks: target_chunks - matched_chunks,
        local_reusable_bytes,
        missing_payload_bytes,
        reuse_ratio: if descriptor.target.size == 0 {
            1.0
        } else {
            local_reusable_bytes as f64 / descriptor.target.size as f64
        },
        index: index.stats()?,
        plan,
    })
}

pub fn stage_missing_chunks(
    descriptor: &TargetDescriptor,
    target: &Path,
    index: &IndexDb,
    output_directory: &Path,
) -> Result<StageMissingReport> {
    descriptor.validate()?;
    index.ensure_profile(descriptor.profile)?;
    verify_target_identity(descriptor, target)?;
    std::fs::create_dir_all(output_directory).with_context(|| {
        format!(
            "create missing chunk directory {}",
            output_directory.display()
        )
    })?;

    let mut target_file = File::open(target)
        .with_context(|| format!("open target {} for missing chunk staging", target.display()))?;
    let mut missing_chunk_occurrences = 0_u64;
    let mut missing_payload_bytes = 0_u64;
    let mut unique_chunk_files_written = 0_u64;
    let mut unique_chunk_file_bytes = 0_u64;

    for chunk in &descriptor.chunks {
        if index.lookup_chunk(&chunk.hash, chunk.length)?.is_some() {
            continue;
        }
        missing_chunk_occurrences += 1;
        missing_payload_bytes += chunk.length as u64;
        let destination = output_directory.join(format!("{}.chunk", chunk.hash));
        let bytes = read_range(&mut target_file, chunk.offset, chunk.length)?;
        verify_chunk(&bytes, &chunk.hash)?;
        if destination.exists() {
            let existing = std::fs::read(&destination)
                .with_context(|| format!("read existing chunk {}", destination.display()))?;
            verify_chunk(&existing, &chunk.hash)?;
            continue;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .with_context(|| format!("create missing chunk {}", destination.display()))?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        unique_chunk_files_written += 1;
        unique_chunk_file_bytes += bytes.len() as u64;
    }

    Ok(StageMissingReport {
        output_directory: output_directory.to_path_buf(),
        missing_chunk_occurrences,
        missing_payload_bytes,
        unique_chunk_files_written,
        unique_chunk_file_bytes,
    })
}

pub fn rebuild_target(
    descriptor: &TargetDescriptor,
    index: &IndexDb,
    missing_source: &Path,
    output: &Path,
) -> Result<RebuildReport> {
    descriptor.validate()?;
    index.ensure_profile(descriptor.profile)?;
    if output.exists() {
        bail!("refusing to overwrite existing output {}", output.display());
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create output directory {}", parent.display()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".shardmeld-")
        .suffix(".partial")
        .tempfile_in(parent)
        .with_context(|| format!("create partial output in {}", parent.display()))?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, temporary.as_file());
    let mut sha256 = Sha256::new();
    let mut local_bytes_read = 0_u64;
    let mut missing_source_bytes_read = 0_u64;
    let mut local_chunk_occurrences = 0_u64;
    let mut missing_chunk_occurrences = 0_u64;

    for chunk in &descriptor.chunks {
        let mut local_bytes = None;
        if let Some(source) = index.lookup_chunk(&chunk.hash, chunk.length)?
            && let Ok(bytes) = read_verified_source(&source, &chunk.hash)
        {
            local_bytes = Some(bytes);
        }
        let bytes = if let Some(bytes) = local_bytes {
            local_bytes_read += bytes.len() as u64;
            local_chunk_occurrences += 1;
            bytes
        } else {
            let path = missing_source.join(format!("{}.chunk", chunk.hash));
            let bytes = std::fs::read(&path).with_context(|| {
                format!(
                    "missing chunk source not found or unreadable: {}",
                    path.display()
                )
            })?;
            if bytes.len() != chunk.length as usize {
                bail!(
                    "missing chunk {} length mismatch: expected {}, got {}",
                    path.display(),
                    chunk.length,
                    bytes.len()
                );
            }
            verify_chunk(&bytes, &chunk.hash)?;
            missing_source_bytes_read += bytes.len() as u64;
            missing_chunk_occurrences += 1;
            bytes
        };
        writer.write_all(&bytes)?;
        sha256.update(&bytes);
    }
    writer.flush()?;
    drop(writer);
    temporary.as_file().sync_all()?;
    let output_sha256 = hex::encode(sha256.finalize());
    if output_sha256 != descriptor.target.sha256 {
        bail!(
            "rebuilt file SHA-256 mismatch: expected {}, got {output_sha256}",
            descriptor.target.sha256
        );
    }
    temporary
        .persist_noclobber(output)
        .map_err(|error| error.error)
        .with_context(|| format!("persist rebuilt output {}", output.display()))?;

    Ok(RebuildReport {
        output: output.to_path_buf(),
        target_bytes: descriptor.target.size,
        local_bytes_read,
        missing_source_bytes_read,
        local_chunk_occurrences,
        missing_chunk_occurrences,
        output_sha256,
        verified: true,
    })
}

pub fn verify_target(descriptor: &TargetDescriptor, file: &Path) -> Result<VerifyReport> {
    descriptor.validate()?;
    let actual_bytes = std::fs::metadata(file)
        .with_context(|| format!("read verification metadata {}", file.display()))?
        .len();
    let actual_sha256 = sha256_file(file)?;
    Ok(VerifyReport {
        file: file.to_path_buf(),
        expected_sha256: descriptor.target.sha256.clone(),
        actual_sha256: actual_sha256.clone(),
        expected_bytes: descriptor.target.size,
        actual_bytes,
        verified: actual_bytes == descriptor.target.size
            && actual_sha256 == descriptor.target.sha256,
    })
}

fn verify_target_identity(descriptor: &TargetDescriptor, target: &Path) -> Result<()> {
    let report = verify_target(descriptor, target)?;
    if !report.verified {
        bail!(
            "target does not match descriptor: expected {} bytes / {}, got {} bytes / {}",
            report.expected_bytes,
            report.expected_sha256,
            report.actual_bytes,
            report.actual_sha256
        );
    }
    Ok(())
}

fn read_verified_source(source: &SourceLocation, expected_hash: &str) -> Result<Vec<u8>> {
    let mut file = File::open(&source.path)
        .with_context(|| format!("open indexed source {}", source.path.display()))?;
    let bytes = read_range(&mut file, source.offset, source.length)?;
    verify_chunk(&bytes, expected_hash)?;
    Ok(bytes)
}

fn read_range(file: &mut File, offset: u64, length: u32) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0_u8; length as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn verify_chunk(bytes: &[u8], expected_hash: &str) -> Result<()> {
    let actual = blake3::hash(bytes).to_hex().to_string();
    if actual != expected_hash {
        bail!("chunk BLAKE3 mismatch: expected {expected_hash}, got {actual}");
    }
    Ok(())
}
