use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChunkProfile {
    pub min_size: u32,
    pub avg_size: u32,
    pub max_size: u32,
}

impl ChunkProfile {
    pub fn named(name: &str) -> Result<Self> {
        let profile = match name.to_ascii_lowercase().as_str() {
            "s" | "small" => Self {
                min_size: 8 * 1024,
                avg_size: 32 * 1024,
                max_size: 128 * 1024,
            },
            "m" | "medium" => Self {
                min_size: 16 * 1024,
                avg_size: 64 * 1024,
                max_size: 256 * 1024,
            },
            "l" | "large" => Self {
                min_size: 64 * 1024,
                avg_size: 256 * 1024,
                max_size: 1024 * 1024,
            },
            _ => bail!("unknown chunk profile '{name}'; expected s, m, or l"),
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<()> {
        if self.min_size == 0 || self.min_size >= self.avg_size || self.avg_size >= self.max_size {
            bail!("chunk profile must satisfy 0 < min < avg < max");
        }
        if !self.avg_size.is_power_of_two() {
            bail!("average chunk size must be a power of two");
        }
        Ok(())
    }
}
