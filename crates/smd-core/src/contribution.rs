use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::address::{Address, NetworkId};
use crate::crypto::{CONTRIBUTION_DOMAIN, sha256_hex, sign_hex, verify_hex};
use crate::wallet::Wallet;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServiceType {
    StandardUpload,
    RareDataUpload,
    CdcReconstructionUpload,
}

impl ServiceType {
    pub const fn protocol_name(self) -> &'static str {
        match self {
            Self::StandardUpload => "STANDARD_UPLOAD",
            Self::RareDataUpload => "RARE_DATA_UPLOAD",
            Self::CdcReconstructionUpload => "CDC_RECONSTRUCTION_UPLOAD",
        }
    }
}

impl FromStr for ServiceType {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_uppercase().as_str() {
            "STANDARD_UPLOAD" | "STANDARD" => Ok(Self::StandardUpload),
            "RARE_DATA_UPLOAD" | "RARE" => Ok(Self::RareDataUpload),
            "CDC_RECONSTRUCTION_UPLOAD" | "CDC" => Ok(Self::CdcReconstructionUpload),
            _ => bail!("unsupported SMD service type: {value}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContributionReceipt {
    pub version: u32,
    pub network_id: NetworkId,
    pub provider_address: String,
    pub provider_public_key: String,
    pub receiver_address: String,
    pub receiver_public_key: String,
    pub session_id: String,
    pub content_hash: String,
    pub bytes_delivered: u64,
    pub service_type: ServiceType,
    pub epoch: u64,
    pub nonce: u64,
    pub content_verified: bool,
    pub receiver_signature: String,
}

#[derive(Serialize)]
struct SignableReceipt<'a> {
    version: u32,
    network_id: NetworkId,
    provider_address: &'a str,
    provider_public_key: &'a str,
    receiver_address: &'a str,
    session_id: &'a str,
    content_hash: &'a str,
    bytes_delivered: u64,
    service_type: ServiceType,
    epoch: u64,
    nonce: u64,
    content_verified: bool,
}

impl ContributionReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn confirmed(
        provider: &Wallet,
        receiver: &Wallet,
        session_id: String,
        content_hash: String,
        bytes_delivered: u64,
        service_type: ServiceType,
        epoch: u64,
        nonce: u64,
    ) -> Result<Self> {
        if provider.network() != receiver.network() {
            bail!("provider and receiver wallets belong to different networks");
        }
        let mut receipt = Self {
            version: 1,
            network_id: provider.network(),
            provider_address: provider.address()?.to_string(),
            provider_public_key: provider.public_key_hex(),
            receiver_address: receiver.address()?.to_string(),
            receiver_public_key: receiver.public_key_hex(),
            session_id,
            content_hash,
            bytes_delivered,
            service_type,
            epoch,
            nonce,
            content_verified: true,
            receiver_signature: String::new(),
        };
        receipt.receiver_signature = sign_hex(
            receiver.signing_key(),
            CONTRIBUTION_DOMAIN,
            &receipt.signing_bytes()?,
        );
        Ok(receipt)
    }

    pub fn verify(&self) -> Result<()> {
        if self.version != 1 || self.network_id != NetworkId::Devnet {
            bail!("unsupported contribution receipt version or network");
        }
        if self.session_id.is_empty() || self.session_id.len() > 128 {
            bail!("invalid contribution session id");
        }
        if self.provider_address.len() > 128 || self.receiver_address.len() > 128 {
            bail!("contribution address exceeds size limit");
        }
        if self.bytes_delivered == 0 || !self.content_verified {
            bail!("contribution data was not verified");
        }
        if self.provider_address == self.receiver_address {
            bail!("provider may not self-confirm a contribution receipt");
        }
        if self.content_hash.len() != 64 {
            bail!("content hash must be a 32-byte hexadecimal SHA-256 value");
        }
        hex::decode(&self.content_hash).context("invalid content hash")?;
        let provider_key =
            crate::crypto::decode_array::<32>(&self.provider_public_key, "provider public key")?;
        let receiver_key =
            crate::crypto::decode_array::<32>(&self.receiver_public_key, "receiver public key")?;
        if Address::from_public_key(self.network_id, &provider_key)?.as_str()
            != self.provider_address
            || Address::from_public_key(self.network_id, &receiver_key)?.as_str()
                != self.receiver_address
        {
            bail!("contribution address does not match public key");
        }
        verify_hex(
            &self.receiver_public_key,
            &self.receiver_signature,
            CONTRIBUTION_DOMAIN,
            &self.signing_bytes()?,
        )
    }

    pub fn id(&self) -> Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(self)?))
    }

    fn signing_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&SignableReceipt {
            version: self.version,
            network_id: self.network_id,
            provider_address: &self.provider_address,
            provider_public_key: &self.provider_public_key,
            receiver_address: &self.receiver_address,
            session_id: &self.session_id,
            content_hash: &self.content_hash,
            bytes_delivered: self.bytes_delivered,
            service_type: self.service_type,
            epoch: self.epoch,
            nonce: self.nonce,
            content_verified: self.content_verified,
        })?)
    }
}
