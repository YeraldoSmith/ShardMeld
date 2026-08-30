use std::path::PathBuf;

use anyhow::{Context, Result, bail};

fn deterministic_bytes(length: usize) -> Vec<u8> {
    let mut value = 0x1234_5678_9abc_def0_u64;
    let mut bytes = Vec::with_capacity(length);
    for _ in 0..length {
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        bytes.push((value >> 24) as u8);
    }
    bytes
}

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: generate_fixture <new-output-directory>")?;
    if output.exists() {
        bail!("fixture output already exists: {}", output.display());
    }
    let sources = output.join("sources");
    std::fs::create_dir_all(&sources)?;

    let base = deterministic_bytes(16 * 1024 * 1024);
    std::fs::write(sources.join("base-v1.bin"), &base)?;
    let mut target = base;
    target.splice(2_000_000..2_000_000, deterministic_bytes(65_537));
    for byte in &mut target[9_000_000..9_262_144] {
        *byte ^= 0xa5;
    }
    std::fs::write(output.join("target-v2.bin"), target)?;
    Ok(())
}
