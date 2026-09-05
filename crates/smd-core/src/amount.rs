use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const ATOMIC_UNITS_PER_SMD: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Amount(u64);

impl Amount {
    pub const ZERO: Self = Self(0);

    pub const fn from_atomic(atomic: u64) -> Self {
        Self(atomic)
    }

    pub const fn atomic(self) -> u64 {
        self.0
    }

    pub fn checked_add(self, other: Self) -> Result<Self> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .context("SMD amount addition overflow")
    }

    pub fn checked_sub(self, other: Self) -> Result<Self> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .context("insufficient SMD balance")
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / ATOMIC_UNITS_PER_SMD;
        let fractional = self.0 % ATOMIC_UNITS_PER_SMD;
        write!(formatter, "{whole}.{fractional:08}")
    }
}

impl FromStr for Amount {
    type Err = anyhow::Error;

    fn from_str(input: &str) -> Result<Self> {
        if input.is_empty() || input.starts_with('-') || input.starts_with('+') {
            bail!("invalid SMD amount");
        }
        let mut parts = input.split('.');
        let whole = parts.next().unwrap_or_default();
        let fractional = parts.next();
        if parts.next().is_some() || whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit())
        {
            bail!("invalid SMD amount");
        }
        let whole = whole.parse::<u64>().context("invalid whole SMD amount")?;
        let whole_atomic = whole
            .checked_mul(ATOMIC_UNITS_PER_SMD)
            .context("SMD amount overflow")?;
        let fractional_atomic = match fractional {
            None => 0,
            Some(value) => {
                if value.is_empty() || value.len() > 8 || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    bail!("SMD amount supports at most 8 decimal places");
                }
                let parsed = value
                    .parse::<u64>()
                    .context("invalid fractional SMD amount")?;
                parsed
                    .checked_mul(10_u64.pow((8 - value.len()) as u32))
                    .context("SMD amount overflow")?
            }
        };
        Ok(Self(
            whole_atomic
                .checked_add(fractional_atomic)
                .context("SMD amount overflow")?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::Amount;

    #[test]
    fn parses_and_formats_exactly_without_floats() {
        let amount: Amount = "12.84000000".parse().unwrap();
        assert_eq!(amount.atomic(), 1_284_000_000);
        assert_eq!(amount.to_string(), "12.84000000");
        assert!("0.000000001".parse::<Amount>().is_err());
        assert!("184467440737.09551616".parse::<Amount>().is_err());
    }
}
