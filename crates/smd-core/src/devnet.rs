use std::path::Path;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::address::{Address, NetworkId};
use crate::amount::Amount;
use crate::consensus::DevnetAuthorityConsensus;
use crate::contribution::{ContributionReceipt, ServiceType};
use crate::crypto::sha256_hex;
use crate::ledger::{Ledger, LedgerStatus};
use crate::rewards::RewardSummary;
use crate::transaction::Transaction;
use crate::wallet::Wallet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevnetScenarioReport {
    pub label: String,
    pub network_id: NetworkId,
    pub alice_address: String,
    pub bob_address: String,
    pub contribution_receipt_id: String,
    pub reward: RewardSummary,
    pub alice_to_bob_transaction_id: String,
    pub bob_to_reserve_transaction_id: String,
    pub alice_balance: Amount,
    pub bob_balance: Amount,
    pub ledger_after_restart: LedgerStatus,
}

pub fn run_devnet_scenario(path: &Path) -> Result<DevnetScenarioReport> {
    let alice = Wallet::from_secret_bytes(NetworkId::Devnet, [0xA1; 32]);
    let bob = Wallet::from_secret_bytes(NetworkId::Devnet, [0xB2; 32]);
    let alice_address = alice.address()?;
    let bob_address = bob.address()?;
    let (receipt_id, reward, alice_tx_id, reserve_tx_id, alice_balance, bob_balance) = {
        let mut ledger = Ledger::open(path, NetworkId::Devnet)?;
        let initial = ledger.status()?;
        if initial.transactions != 0 || initial.contribution_receipts != 0 {
            bail!("devnet scenario requires a fresh ledger");
        }
        let receipt = ContributionReceipt::confirmed(
            &alice,
            &bob,
            "smd-v01-scenario-session".to_owned(),
            sha256_hex(b"verified-shardmeld-payload"),
            2 * 1_073_741_824,
            ServiceType::CdcReconstructionUpload,
            1,
            0,
        )?;
        let receipt_id = ledger.submit_contribution(&receipt)?;
        let reward = ledger.mine_rewards(1, &DevnetAuthorityConsensus)?;

        let alice_tx = Transaction::signed(
            &alice,
            &bob_address,
            "5.00000000".parse()?,
            ledger.account(&alice_address)?.nonce,
            1,
            10,
        )?;
        let alice_tx_id = ledger.submit_transaction(&alice_tx, 1)?;
        let reserve_tx = Transaction::signed(
            &bob,
            &Address::reserve(),
            "2.00000000".parse()?,
            ledger.account(&bob_address)?.nonce,
            1,
            10,
        )?;
        let reserve_tx_id = ledger.submit_transaction(&reserve_tx, 1)?;
        ledger.verify_invariants()?;
        (
            receipt_id,
            reward,
            alice_tx_id,
            reserve_tx_id,
            ledger.account(&alice_address)?.balance,
            ledger.account(&bob_address)?.balance,
        )
    };
    let reopened = Ledger::open(path, NetworkId::Devnet)?;
    reopened.verify_invariants()?;
    Ok(DevnetScenarioReport {
        label: "DEVNET / EXPERIMENTAL / NOT REAL-MONEY READY".to_owned(),
        network_id: NetworkId::Devnet,
        alice_address: alice_address.to_string(),
        bob_address: bob_address.to_string(),
        contribution_receipt_id: receipt_id,
        reward,
        alice_to_bob_transaction_id: alice_tx_id,
        bob_to_reserve_transaction_id: reserve_tx_id,
        alice_balance,
        bob_balance,
        ledger_after_restart: reopened.status()?,
    })
}
