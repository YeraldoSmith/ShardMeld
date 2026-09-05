use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use bech32::{FromBase32, ToBase32, Variant};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PERMANENT_RESERVE_ADDRESS: &str = "SMD_PERMANENT_RESERVE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkId {
    Mainnet,
    Devnet,
}

impl NetworkId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Devnet => "devnet",
        }
    }

    fn hrp(self) -> &'static str {
        match self {
            Self::Mainnet => "smd",
            Self::Devnet => "smddev",
        }
    }
}

impl fmt::Display for NetworkId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for NetworkId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "mainnet" => Ok(Self::Mainnet),
            "devnet" => Ok(Self::Devnet),
            _ => bail!("unsupported SMD network: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Address(String);

impl Address {
    pub fn from_public_key(network: NetworkId, public_key: &[u8; 32]) -> Result<Self> {
        let digest = Sha256::digest(public_key);
        let encoded = bech32::encode(
            network.hrp(),
            digest[..20].to_vec().to_base32(),
            Variant::Bech32,
        )?;
        Ok(Self(encoded))
    }

    pub fn parse_for_network(value: &str, network: NetworkId) -> Result<Self> {
        if value == PERMANENT_RESERVE_ADDRESS {
            return Ok(Self(value.to_owned()));
        }
        if value.len() > 128 {
            bail!("SMD address exceeds size limit");
        }
        let (hrp, data, variant) = bech32::decode(value)?;
        if hrp != network.hrp() || variant != Variant::Bech32 {
            bail!("address does not belong to {network}");
        }
        let payload = Vec::<u8>::from_base32(&data)?;
        if payload.len() != 20 {
            bail!("invalid SMD address payload length");
        }
        Ok(Self(value.to_owned()))
    }

    pub fn reserve() -> Self {
        Self(PERMANENT_RESERVE_ADDRESS.to_owned())
    }

    pub fn is_reserve(&self) -> bool {
        self.0 == PERMANENT_RESERVE_ADDRESS
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
