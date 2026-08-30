use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ChunkProfile;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkRecord {
    pub offset: u64,
    pub length: u32,
    pub hash: String,
}

static GEAR_TABLE: LazyLock<[u64; 256]> = LazyLock::new(|| {
    let mut table = [0_u64; 256];
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    for value in &mut table {
        state = splitmix64(state);
        *value = state;
    }
    table
});

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut mixed = value;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^ (mixed >> 31)
}

pub fn visit_file_chunks<F>(path: &Path, profile: ChunkProfile, mut visitor: F) -> Result<u64>
where
    F: FnMut(ChunkRecord) -> Result<()>,
{
    profile.validate()?;
    let file = File::open(path).with_context(|| format!("open source file {}", path.display()))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut read_buffer = [0_u8; 1024 * 1024];
    let mut chunk_buffer = Vec::with_capacity(profile.max_size as usize);
    let mut gear_hash = 0_u64;
    let mut chunk_start = 0_u64;
    let mut total_bytes = 0_u64;
    let mut chunk_count = 0_u64;

    let average_bits = profile.avg_size.trailing_zeros();
    let early_mask = (1_u64 << (average_bits + 1)) - 1;
    let late_mask = (1_u64 << average_bits.saturating_sub(1)) - 1;

    loop {
        let bytes_read = reader
            .read(&mut read_buffer)
            .with_context(|| format!("read source file {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }

        for &byte in &read_buffer[..bytes_read] {
            chunk_buffer.push(byte);
            // Left shift intentionally discards old high bits. A rotate would keep
            // the entire byte history alive and prevent CDC from resynchronizing
            // after insertions or deletions.
            gear_hash = (gear_hash << 1).wrapping_add(GEAR_TABLE[byte as usize]);

            let length = chunk_buffer.len() as u32;
            let mask = if length < profile.avg_size {
                early_mask
            } else {
                late_mask
            };
            let boundary = length >= profile.min_size && (gear_hash & mask == 0);
            if boundary || length >= profile.max_size {
                emit_chunk(&mut visitor, chunk_start, &chunk_buffer)?;
                chunk_count += 1;
                chunk_start += length as u64;
                total_bytes += length as u64;
                chunk_buffer.clear();
                gear_hash = 0;
            }
        }
    }

    if !chunk_buffer.is_empty() {
        let length = chunk_buffer.len() as u64;
        emit_chunk(&mut visitor, chunk_start, &chunk_buffer)?;
        chunk_count += 1;
        total_bytes += length;
    }

    let expected = std::fs::metadata(path)?.len();
    if total_bytes != expected {
        anyhow::bail!(
            "chunked byte count mismatch for {}: expected {expected}, got {total_bytes}",
            path.display()
        );
    }

    Ok(chunk_count)
}

fn emit_chunk<F>(visitor: &mut F, offset: u64, bytes: &[u8]) -> Result<()>
where
    F: FnMut(ChunkRecord) -> Result<()>,
{
    visitor(ChunkRecord {
        offset,
        length: bytes.len() as u32,
        hash: blake3::hash(bytes).to_hex().to_string(),
    })
}
