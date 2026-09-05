use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::address::{Address, NetworkId};
use crate::amount::Amount;
use crate::crypto::{TRANSACTION_DOMAIN, sha256_hex, sign_hex, verify_hex};
use crate::wallet::Wallet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transaction {
    pub version: u32,
    pub network_id: NetworkId,
    pub from: String,
    pub from_public_key: String,
    pub to: String,
    pub amount: Amount,
    pub nonce: u64,
    pub created_epoch: u64,
    pub expiry_epoch: u64,
    pub signature: String,
}

#[derive(Serialize)]
struct SignableTransaction<'a> {
    version: u32,
    network_id: NetworkId,
    from: &'a str,
    to: &'a str,
    amount_atomic: u64,
    nonce: u64,
    created_epoch: u64,
    expiry_epoch: u64,
}

impl Transaction {
    pub fn signed(
        wallet: &Wallet,
        to: &Address,
        amount: Amount,
        nonce: u64,
        created_epoch: u64,
        expiry_epoch: u64,
    ) -> Result<Self> {
        let from = wallet.address()?.to_string();
        let mut transaction = Self {
            version: 1,
            network_id: wallet.network(),
            from,
            from_public_key: wallet.public_key_hex(),
            to: to.to_string(),
            amount,
            nonce,
            created_epoch,
            expiry_epoch,
            signature: String::new(),
        };
        transaction.signature = sign_hex(
            wallet.signing_key(),
            TRANSACTION_DOMAIN,
            &transaction.signing_bytes()?,
        );
        Ok(transaction)
    }

    pub fn verify_signature(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported SMD transaction version");
        }
        if self.from.len() > 128 || self.to.len() > 128 {
            bail!("transaction address exceeds size limit");
        }
        let public_key: [u8; 32] =
            crate::crypto::decode_array(&self.from_public_key, "transaction public key")?;
        let expected = Address::from_public_key(self.network_id, &public_key)?;
        if expected.as_str() != self.from {
            bail!("transaction sender does not match public key");
        }
        verify_hex(
            &self.from_public_key,
            &self.signature,
            TRANSACTION_DOMAIN,
            &self.signing_bytes()?,
        )
    }

    pub fn id(&self) -> Result<String> {
        let encoded = serde_json::to_vec(self)?;
        Ok(sha256_hex(&encoded))
    }

    fn signing_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&SignableTransaction {
            version: self.version,
            network_id: self.network_id,
            from: &self.from,
            to: &self.to,
            amount_atomic: self.amount.atomic(),
            nonce: self.nonce,
            created_epoch: self.created_epoch,
            expiry_epoch: self.expiry_epoch,
        })?)
    }
}
