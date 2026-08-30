use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::chunker::visit_file_chunks;
use crate::{ChunkProfile, ChunkRecord, sha256_file};

pub const DESCRIPTOR_FORMAT: &str = "shardmeld-descriptor";
pub const DESCRIPTOR_VERSION: u32 = 1;
pub const CHUNK_ALGORITHM: &str = "gear-cdc-v1+blake3-256";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetInfo {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetDescriptor {
    pub format: String,
    pub version: u32,
    pub chunk_algorithm: String,
    pub profile: ChunkProfile,
    pub target: TargetInfo,
    pub chunks: Vec<ChunkRecord>,
}

impl TargetDescriptor {
    pub fn validate(&self) -> Result<()> {
        if self.format != DESCRIPTOR_FORMAT || self.version != DESCRIPTOR_VERSION {
            bail!(
                "unsupported descriptor {} version {}",
                self.format,
                self.version
            );
        }
        if self.chunk_algorithm != CHUNK_ALGORITHM {
            bail!(
                "unsupported chunk algorithm '{}'; expected {CHUNK_ALGORITHM}",
                self.chunk_algorithm
            );
        }
        self.profile.validate()?;
        if self.target.sha256.len() != 64 || hex::decode(&self.target.sha256).is_err() {
            bail!("target SHA-256 must be 32-byte hexadecimal");
        }

        let mut expected_offset = 0_u64;
        for (index, chunk) in self.chunks.iter().enumerate() {
            if chunk.offset != expected_offset {
                bail!(
                    "descriptor chunk {index} offset mismatch: expected {expected_offset}, got {}",
                    chunk.offset
                );
            }
            if chunk.length == 0 {
                bail!("descriptor chunk {index} has zero length");
            }
            if chunk.hash.len() != 64 || hex::decode(&chunk.hash).is_err() {
                bail!("descriptor chunk {index} has invalid BLAKE3 hash");
            }
            expected_offset = expected_offset
                .checked_add(chunk.length as u64)
                .context("descriptor size overflow")?;
        }
        if expected_offset != self.target.size {
            bail!(
                "descriptor target size mismatch: chunks total {expected_offset}, target says {}",
                self.target.size
            );
        }
        if self.target.size > 0 && self.chunks.is_empty() {
            bail!("non-empty target has no chunks");
        }
        Ok(())
    }
}

pub fn create_descriptor(target: &Path, profile: ChunkProfile) -> Result<TargetDescriptor> {
    profile.validate()?;
    let metadata = std::fs::metadata(target)
        .with_context(|| format!("read target metadata {}", target.display()))?;
    if !metadata.is_file() {
        bail!("target is not a regular file: {}", target.display());
    }
    let mut chunks = Vec::new();
    visit_file_chunks(target, profile, |chunk| {
        chunks.push(chunk);
        Ok(())
    })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("target")
        .to_owned();
    let descriptor = TargetDescriptor {
        format: DESCRIPTOR_FORMAT.to_owned(),
        version: DESCRIPTOR_VERSION,
        chunk_algorithm: CHUNK_ALGORITHM.to_owned(),
        profile,
        target: TargetInfo {
            name,
            size: metadata.len(),
            sha256: sha256_file(target)?,
        },
        chunks,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

pub fn save_descriptor(descriptor: &TargetDescriptor, output: &Path) -> Result<()> {
    descriptor.validate()?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .with_context(|| format!("create descriptor {}", output.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), descriptor)
        .with_context(|| format!("write descriptor {}", output.display()))?;
    Ok(())
}

pub fn load_descriptor(path: &Path) -> Result<TargetDescriptor> {
    let file =
        std::fs::File::open(path).with_context(|| format!("open descriptor {}", path.display()))?;
    let descriptor: TargetDescriptor = serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("parse descriptor {}", path.display()))?;
    descriptor.validate()?;
    Ok(descriptor)
}
