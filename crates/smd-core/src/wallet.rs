use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::address::{Address, NetworkId};
use crate::crypto::decode_array;

pub const DEVNET_WALLET_FILE_KIND: &str = "smd-devnet-test-wallet";

pub struct Wallet {
    network: NetworkId,
    signing_key: SigningKey,
}

impl fmt::Debug for Wallet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Wallet")
            .field("network", &self.network)
            .field("address", &self.address().ok())
            .field("private_key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WalletBackup {
    pub kind: String,
    pub version: u32,
    pub network_id: NetworkId,
    pub address: String,
    #[serde(rename = "private_key_hex")]
    private_key_hex: String,
}

impl fmt::Debug for WalletBackup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WalletBackup")
            .field("kind", &self.kind)
            .field("version", &self.version)
            .field("network_id", &self.network_id)
            .field("address", &self.address)
            .field("private_key_hex", &"[REDACTED]")
            .finish()
    }
}

impl Drop for WalletBackup {
    fn drop(&mut self) {
        self.private_key_hex.zeroize();
    }
}

impl Wallet {
    pub fn generate(network: NetworkId) -> Self {
        Self {
            network,
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    pub fn from_secret_bytes(network: NetworkId, secret: [u8; 32]) -> Self {
        Self {
            network,
            signing_key: SigningKey::from_bytes(&secret),
        }
    }

    pub fn network(&self) -> NetworkId {
        self.network
    }

    pub fn address(&self) -> Result<Address> {
        Address::from_public_key(self.network, &self.signing_key.verifying_key().to_bytes())
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    pub fn backup(&self) -> Result<WalletBackup> {
        Ok(WalletBackup {
            kind: DEVNET_WALLET_FILE_KIND.to_owned(),
            version: 1,
            network_id: self.network,
            address: self.address()?.to_string(),
            private_key_hex: hex::encode(self.signing_key.to_bytes()),
        })
    }

    pub fn export_devnet_test_file(&self, path: &Path) -> Result<()> {
        if self.network != NetworkId::Devnet {
            bail!("explicit wallet files are enabled only for the SMD devnet");
        }
        let backup = serde_json::to_vec_pretty(&self.backup()?)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(path)
            .with_context(|| format!("create devnet wallet file {}", path.display()))?;
        file.write_all(&backup)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    }

    pub fn import_devnet_test_file(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)
            .with_context(|| format!("inspect devnet wallet file {}", path.display()))?;
        if metadata.len() > 4096 {
            bail!("devnet wallet file exceeds 4096-byte limit");
        }
        let bytes = std::fs::read(path)
            .with_context(|| format!("read devnet wallet file {}", path.display()))?;
        let backup: WalletBackup = serde_json::from_slice(&bytes)?;
        Self::from_backup(backup)
    }

    pub fn from_backup(backup: WalletBackup) -> Result<Self> {
        if backup.kind != DEVNET_WALLET_FILE_KIND || backup.version != 1 {
            bail!("unsupported SMD wallet backup format");
        }
        if backup.network_id != NetworkId::Devnet {
            bail!("only devnet wallet backups are accepted in SMD v0.1");
        }
        let secret = decode_array::<32>(&backup.private_key_hex, "private key")?;
        let wallet = Self::from_secret_bytes(backup.network_id, secret);
        if wallet.address()?.as_str() != backup.address {
            bail!("wallet backup address does not match its private key");
        }
        Ok(wallet)
    }
}

#[cfg(test)]
mod tests {
    use super::Wallet;
    use crate::NetworkId;

    #[test]
    fn wallet_addresses_are_stable_and_distinct() {
        let first = Wallet::from_secret_bytes(NetworkId::Devnet, [7; 32]);
        let same = Wallet::from_secret_bytes(NetworkId::Devnet, [7; 32]);
        let second = Wallet::from_secret_bytes(NetworkId::Devnet, [8; 32]);
        assert_eq!(first.address().unwrap(), same.address().unwrap());
        assert_ne!(first.address().unwrap(), second.address().unwrap());
        assert!(first.address().unwrap().as_str().starts_with("smddev1"));
    }

    #[test]
    fn backup_round_trip_preserves_address() {
        let wallet = Wallet::from_secret_bytes(NetworkId::Devnet, [11; 32]);
        let imported = Wallet::from_backup(wallet.backup().unwrap()).unwrap();
        assert_eq!(wallet.address().unwrap(), imported.address().unwrap());
    }
}
