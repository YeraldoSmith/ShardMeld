use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub const TRANSACTION_DOMAIN: &[u8] = b"ShardMeld/SMD/devnet/transaction/v1\0";
pub const CONTRIBUTION_DOMAIN: &[u8] = b"ShardMeld/SMD/devnet/contribution-receipt/v1\0";

pub fn sign_hex(signing_key: &SigningKey, domain: &[u8], payload: &[u8]) -> String {
    let message = domain_message(domain, payload);
    hex::encode(signing_key.sign(&message).to_bytes())
}

pub fn verify_hex(
    public_key_hex: &str,
    signature_hex: &str,
    domain: &[u8],
    payload: &[u8],
) -> Result<()> {
    let public_key_bytes = decode_array::<32>(public_key_hex, "public key")?;
    let signature_bytes = decode_array::<64>(signature_hex, "signature")?;
    let public_key = VerifyingKey::from_bytes(&public_key_bytes).context("invalid public key")?;
    let signature = Signature::from_bytes(&signature_bytes);
    public_key
        .verify(&domain_message(domain, payload), &signature)
        .context("signature verification failed")
}

pub fn decode_array<const N: usize>(value: &str, label: &str) -> Result<[u8; N]> {
    if value.len() > N * 2 {
        bail!("{label} exceeds maximum encoded size");
    }
    let bytes = hex::decode(value).with_context(|| format!("invalid {label} encoding"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid {label} length"))
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn domain_message(domain: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(domain.len() + payload.len());
    message.extend_from_slice(domain);
    message.extend_from_slice(payload);
    message
}
