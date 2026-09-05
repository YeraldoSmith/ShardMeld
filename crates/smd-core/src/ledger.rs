use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction as SqlTransaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::address::{Address, NetworkId};
use crate::amount::Amount;
use crate::consensus::ConsensusEngine;
use crate::contribution::ContributionReceipt;
use crate::emission::VersionedEmissionPolicy;
use crate::pricing::{ProtocolPricingEngine, V01PricingEngine};
use crate::rewards::{AntiFraudPolicy, RewardSummary};
use crate::supply::{GENESIS_RESERVE, MAX_NETWORK_EMISSION, SupplyState};
use crate::transaction::Transaction;

const MAX_SERIALIZED_RECEIPT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub address: String,
    pub balance: Amount,
    pub nonce: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionRecord {
    pub transaction_id: String,
    pub transaction: Transaction,
    pub accepted_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewardReceiptRecord {
    pub receipt_id: String,
    pub receipt: ContributionReceipt,
    pub score_bytes: u64,
    pub protocol_subsidy: Amount,
    pub user_resource_fee: Amount,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerStatus {
    pub protocol_version: u32,
    pub network_id: NetworkId,
    pub accounts: u64,
    pub transactions: u64,
    pub contribution_receipts: u64,
    pub pending_receipts: u64,
    pub supply: SupplyState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LedgerAuditReport {
    pub audit_version: u32,
    pub network_id: NetworkId,
    pub state_root_sha256: String,
    pub accounts: u64,
    pub transactions: u64,
    pub contribution_receipts: u64,
    pub epochs: u64,
    pub supply: SupplyState,
    pub invariants_valid: bool,
}

pub struct Ledger {
    connection: Connection,
    network: NetworkId,
    anti_fraud: AntiFraudPolicy,
}

impl Ledger {
    pub fn open(path: &Path, network: NetworkId) -> Result<Self> {
        if network != NetworkId::Devnet {
            bail!("SMD v0.1 enables only devnet");
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open SMD ledger {}", path.display()))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut ledger = Self {
            connection,
            network,
            anti_fraud: AntiFraudPolicy::default(),
        };
        ledger.initialize()?;
        ledger.verify_invariants()?;
        Ok(ledger)
    }

    fn initialize(&mut self) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS metadata (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS accounts (
                 address TEXT PRIMARY KEY,
                 balance INTEGER NOT NULL CHECK(balance >= 0),
                 nonce INTEGER NOT NULL CHECK(nonce >= 0),
                 public_key TEXT
             );
             CREATE TABLE IF NOT EXISTS transactions (
                 transaction_id TEXT PRIMARY KEY,
                 transaction_json TEXT NOT NULL,
                 from_address TEXT NOT NULL,
                 to_address TEXT NOT NULL,
                 amount INTEGER NOT NULL CHECK(amount > 0),
                 nonce INTEGER NOT NULL CHECK(nonce >= 0),
                 accepted_epoch INTEGER NOT NULL CHECK(accepted_epoch >= 0),
                 UNIQUE(from_address, nonce)
             );
             CREATE TABLE IF NOT EXISTS epochs (
                 epoch INTEGER PRIMARY KEY CHECK(epoch >= 0),
                 receipts_processed INTEGER NOT NULL CHECK(receipts_processed >= 0),
                 protocol_subsidy INTEGER NOT NULL CHECK(protocol_subsidy >= 0)
             );
             CREATE TABLE IF NOT EXISTS reward_receipts (
                 receipt_id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL UNIQUE,
                 provider_address TEXT NOT NULL,
                 receiver_address TEXT NOT NULL,
                 content_hash TEXT NOT NULL,
                 epoch INTEGER NOT NULL CHECK(epoch >= 0),
                 receipt_nonce INTEGER NOT NULL CHECK(receipt_nonce >= 0),
                 receipt_json TEXT NOT NULL,
                 score_bytes INTEGER NOT NULL CHECK(score_bytes >= 0),
                 protocol_subsidy INTEGER NOT NULL DEFAULT 0 CHECK(protocol_subsidy >= 0),
                 user_resource_fee INTEGER NOT NULL DEFAULT 0 CHECK(user_resource_fee >= 0),
                 emission_phase INTEGER,
                 status TEXT NOT NULL CHECK(status IN ('pending', 'rewarded')),
                 UNIQUE(provider_address, receiver_address, receipt_nonce)
             );
             CREATE TABLE IF NOT EXISTS supply_state (
                 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                 minted_supply INTEGER NOT NULL CHECK(minted_supply >= 0),
                 network_emitted_supply INTEGER NOT NULL CHECK(network_emitted_supply >= 0),
                 reserve_balance INTEGER NOT NULL CHECK(reserve_balance >= 0),
                 circulating_supply INTEGER NOT NULL CHECK(circulating_supply >= 0)
             );",
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO metadata(key, value) VALUES ('schema_version', '1')",
            [],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO metadata(key, value) VALUES ('network_id', ?1)",
            [self.network.as_str()],
        )?;
        let stored_network: String = tx.query_row(
            "SELECT value FROM metadata WHERE key = 'network_id'",
            [],
            |row| row.get(0),
        )?;
        if stored_network != self.network.as_str() {
            bail!("ledger belongs to {stored_network}, not {}", self.network);
        }
        tx.execute(
            "INSERT OR IGNORE INTO accounts(address, balance, nonce, public_key)
             VALUES (?1, ?2, 0, NULL)",
            params![Address::reserve().as_str(), amount_to_sql(GENESIS_RESERVE)?],
        )?;
        let genesis = SupplyState::genesis();
        tx.execute(
            "INSERT OR IGNORE INTO supply_state(
                 singleton, minted_supply, network_emitted_supply,
                 reserve_balance, circulating_supply
             ) VALUES (1, ?1, ?2, ?3, ?4)",
            params![
                amount_to_sql(genesis.minted_supply)?,
                amount_to_sql(genesis.network_emitted_supply)?,
                amount_to_sql(genesis.reserve_balance)?,
                amount_to_sql(genesis.circulating_supply)?,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn network(&self) -> NetworkId {
        self.network
    }

    pub fn account(&self, address: &Address) -> Result<Account> {
        let row = self
            .connection
            .query_row(
                "SELECT balance, nonce FROM accounts WHERE address = ?1",
                [address.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let (balance, nonce) = row.unwrap_or((0, 0));
        Ok(Account {
            address: address.to_string(),
            balance: amount_from_sql(balance)?,
            nonce: u64_from_sql(nonce, "account nonce")?,
        })
    }

    pub fn status(&self) -> Result<LedgerStatus> {
        Ok(LedgerStatus {
            protocol_version: 1,
            network_id: self.network,
            accounts: count(&self.connection, "accounts")?,
            transactions: count(&self.connection, "transactions")?,
            contribution_receipts: count(&self.connection, "reward_receipts")?,
            pending_receipts: u64_from_sql(
                self.connection.query_row(
                    "SELECT COUNT(*) FROM reward_receipts WHERE status = 'pending'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?,
                "pending receipt count",
            )?,
            supply: read_supply(&self.connection)?,
        })
    }

    pub fn submit_transaction(&mut self, transaction: &Transaction, epoch: u64) -> Result<String> {
        self.submit_transactions_atomically(std::slice::from_ref(transaction), epoch)
            .map(|mut ids| ids.remove(0))
    }

    pub fn submit_transactions_atomically(
        &mut self,
        transactions: &[Transaction],
        epoch: u64,
    ) -> Result<Vec<String>> {
        if transactions.is_empty() {
            return Ok(Vec::new());
        }
        let tx = self.connection.transaction()?;
        let before = read_supply_tx(&tx)?;
        let mut ids = Vec::with_capacity(transactions.len());
        for transaction in transactions {
            ids.push(apply_transaction(&tx, self.network, transaction, epoch)?);
        }
        let after = read_supply_tx(&tx)?;
        if after.reserve_balance < before.reserve_balance {
            bail!("permanent reserve balance may never decrease");
        }
        after.validate()?;
        verify_account_sum(&tx, after.minted_supply)?;
        tx.commit()?;
        Ok(ids)
    }

    pub fn submit_contribution(&mut self, receipt: &ContributionReceipt) -> Result<String> {
        receipt.verify()?;
        if receipt.network_id != self.network {
            bail!("contribution receipt belongs to another network");
        }
        let receipt_json = serde_json::to_string(receipt)?;
        if receipt_json.len() > MAX_SERIALIZED_RECEIPT_BYTES {
            bail!("contribution receipt exceeds size limit");
        }
        let receipt_id = receipt.id()?;
        let tx = self.connection.transaction()?;
        let prior_pair: u32 = tx.query_row(
            "SELECT COUNT(*) FROM reward_receipts
             WHERE provider_address = ?1 AND receiver_address = ?2 AND epoch = ?3",
            params![
                receipt.provider_address,
                receipt.receiver_address,
                sql_u64(receipt.epoch)?
            ],
            |row| row.get(0),
        )?;
        let prior_content: u32 = tx.query_row(
            "SELECT COUNT(*) FROM reward_receipts
             WHERE provider_address = ?1 AND receiver_address = ?2
               AND content_hash = ?3 AND epoch = ?4",
            params![
                receipt.provider_address,
                receipt.receiver_address,
                receipt.content_hash,
                sql_u64(receipt.epoch)?,
            ],
            |row| row.get(0),
        )?;
        let score = self.anti_fraud.score(
            receipt.bytes_delivered,
            receipt.service_type,
            prior_pair,
            prior_content,
        )?;
        tx.execute(
            "INSERT INTO reward_receipts(
                 receipt_id, session_id, provider_address, receiver_address,
                 content_hash, epoch, receipt_nonce, receipt_json, score_bytes,
                 protocol_subsidy, user_resource_fee, emission_phase, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 0, NULL, 'pending')",
            params![
                receipt_id,
                receipt.session_id,
                receipt.provider_address,
                receipt.receiver_address,
                receipt.content_hash,
                sql_u64(receipt.epoch)?,
                sql_u64(receipt.nonce)?,
                receipt_json,
                sql_u64(score)?,
            ],
        )?;
        tx.commit()?;
        Ok(receipt_id)
    }

    pub fn mine_rewards(
        &mut self,
        epoch: u64,
        consensus: &dyn ConsensusEngine,
    ) -> Result<RewardSummary> {
        let mut receipts = self.pending_receipts(epoch)?;
        consensus.order_receipts(&mut receipts)?;
        let tx = self.connection.transaction()?;
        let before = read_supply_tx(&tx)?;
        let mut emitted = before.network_emitted_supply;
        let mut total_score = 0_u64;
        let mut total_subsidy = Amount::ZERO;
        let mut total_fees = Amount::ZERO;
        let mut last_phase = VersionedEmissionPolicy::phase(emitted);
        let pricing = V01PricingEngine;

        for receipt in &receipts {
            let receipt_id = receipt.id()?;
            let (status, score_sql): (String, i64) = tx.query_row(
                "SELECT status, score_bytes FROM reward_receipts WHERE receipt_id = ?1",
                [&receipt_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if status != "pending" {
                bail!("consensus selected a non-pending contribution receipt");
            }
            let score = u64_from_sql(score_sql, "contribution score")?;
            let quote = VersionedEmissionPolicy::quote(score, emitted)?;
            last_phase = quote.phase;
            let already_for_address: i64 = tx.query_row(
                "SELECT COALESCE(SUM(protocol_subsidy), 0) FROM reward_receipts
                 WHERE provider_address = ?1 AND epoch = ?2 AND status = 'rewarded'",
                params![receipt.provider_address, sql_u64(epoch)?],
                |row| row.get(0),
            )?;
            let address_remaining = self
                .anti_fraud
                .max_protocol_reward_per_address_epoch
                .checked_sub(amount_from_sql(already_for_address)?)?;
            let subsidy = Amount::from_atomic(
                quote
                    .protocol_subsidy
                    .atomic()
                    .min(address_remaining.atomic()),
            );
            let fee = pricing.user_resource_fee(receipt.bytes_delivered);
            credit_account(
                &tx,
                &receipt.provider_address,
                &receipt.provider_public_key,
                subsidy.checked_add(fee)?,
            )?;
            emitted = emitted.checked_add(subsidy)?;
            if emitted > MAX_NETWORK_EMISSION {
                bail!("reward would exceed maximum network emission");
            }
            total_score = total_score
                .checked_add(score)
                .context("total contribution score overflow")?;
            total_subsidy = total_subsidy.checked_add(subsidy)?;
            total_fees = total_fees.checked_add(fee)?;
            tx.execute(
                "UPDATE reward_receipts SET
                    protocol_subsidy = ?1, user_resource_fee = ?2,
                    emission_phase = ?3, status = 'rewarded'
                 WHERE receipt_id = ?4 AND status = 'pending'",
                params![
                    amount_to_sql(subsidy)?,
                    amount_to_sql(fee)?,
                    i64::from(quote.phase),
                    receipt_id,
                ],
            )?;
        }

        let after = SupplyState::from_components(emitted, before.reserve_balance)?;
        write_supply(&tx, after)?;
        tx.execute(
            "INSERT INTO epochs(epoch, receipts_processed, protocol_subsidy)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(epoch) DO UPDATE SET
                receipts_processed = epochs.receipts_processed + excluded.receipts_processed,
                protocol_subsidy = epochs.protocol_subsidy + excluded.protocol_subsidy",
            params![
                sql_u64(epoch)?,
                sql_u64(receipts.len() as u64)?,
                amount_to_sql(total_subsidy)?,
            ],
        )?;
        verify_account_sum(&tx, after.minted_supply)?;
        tx.commit()?;
        Ok(RewardSummary {
            epoch,
            receipts_processed: receipts.len() as u64,
            total_score_bytes: total_score,
            protocol_subsidy: total_subsidy,
            user_resource_fees: total_fees,
            emission_phase: last_phase,
        })
    }

    pub fn transactions(&self, limit: u32) -> Result<Vec<TransactionRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT transaction_id, transaction_json, accepted_epoch
             FROM transactions ORDER BY rowid DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::from(limit.min(1_000))], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (transaction_id, json, epoch) = row?;
            Ok(TransactionRecord {
                transaction_id,
                transaction: serde_json::from_str(&json)?,
                accepted_epoch: u64_from_sql(epoch, "accepted epoch")?,
            })
        })
        .collect()
    }

    pub fn reward_receipts(&self, limit: u32) -> Result<Vec<RewardReceiptRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT receipt_id, receipt_json, score_bytes, protocol_subsidy,
                    user_resource_fee, status
             FROM reward_receipts ORDER BY rowid DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([i64::from(limit.min(1_000))], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (receipt_id, json, score, subsidy, fee, status) = row?;
            Ok(RewardReceiptRecord {
                receipt_id,
                receipt: serde_json::from_str(&json)?,
                score_bytes: u64_from_sql(score, "contribution score")?,
                protocol_subsidy: amount_from_sql(subsidy)?,
                user_resource_fee: amount_from_sql(fee)?,
                status,
            })
        })
        .collect()
    }

    pub fn verify_invariants(&self) -> Result<()> {
        let supply = read_supply(&self.connection)?;
        supply.validate()?;
        let reserve = self.account(&Address::reserve())?.balance;
        if reserve != supply.reserve_balance || reserve < GENESIS_RESERVE {
            bail!("permanent reserve invariant failed");
        }
        let sum: i64 = self.connection.query_row(
            "SELECT COALESCE(SUM(balance), 0) FROM accounts",
            [],
            |row| row.get(0),
        )?;
        if amount_from_sql(sum)? != supply.minted_supply {
            bail!("account balances do not equal minted supply");
        }
        Ok(())
    }

    pub fn audit(&self) -> Result<LedgerAuditReport> {
        self.verify_invariants()?;
        let status = self.status()?;
        let mut hasher = Sha256::new();
        hash_field(&mut hasher, b"ShardMeld/SMD/ledger-state/v1\0");
        hash_field(&mut hasher, self.network.as_str().as_bytes());

        hash_field(&mut hasher, b"metadata");
        let mut metadata = self
            .connection
            .prepare("SELECT key, value FROM metadata ORDER BY key")?;
        let metadata_rows = metadata.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in metadata_rows {
            let (key, value) = row?;
            hash_field(&mut hasher, key.as_bytes());
            hash_field(&mut hasher, value.as_bytes());
        }

        hash_field(&mut hasher, b"supply_state");
        for amount in [
            status.supply.minted_supply,
            status.supply.network_emitted_supply,
            status.supply.reserve_balance,
            status.supply.circulating_supply,
        ] {
            hash_field(&mut hasher, &amount.atomic().to_be_bytes());
        }

        hash_field(&mut hasher, b"accounts");
        let mut accounts = self.connection.prepare(
            "SELECT address, balance, nonce, public_key
             FROM accounts ORDER BY address",
        )?;
        let account_rows = accounts.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in account_rows {
            let (address, balance, nonce, public_key) = row?;
            hash_field(&mut hasher, address.as_bytes());
            hash_field(
                &mut hasher,
                &u64_from_sql(balance, "account balance")?.to_be_bytes(),
            );
            hash_field(
                &mut hasher,
                &u64_from_sql(nonce, "account nonce")?.to_be_bytes(),
            );
            hash_optional_field(&mut hasher, public_key.as_deref());
        }

        hash_field(&mut hasher, b"transactions");
        let mut transactions = self.connection.prepare(
            "SELECT transaction_id, transaction_json, from_address, to_address,
                    amount, nonce, accepted_epoch
             FROM transactions ORDER BY transaction_id",
        )?;
        let transaction_rows = transactions.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        for row in transaction_rows {
            let (id, json, from, to, amount, nonce, accepted_epoch) = row?;
            for field in [id, json, from, to] {
                hash_field(&mut hasher, field.as_bytes());
            }
            for (value, label) in [
                (amount, "transaction amount"),
                (nonce, "transaction nonce"),
                (accepted_epoch, "transaction accepted epoch"),
            ] {
                hash_field(&mut hasher, &u64_from_sql(value, label)?.to_be_bytes());
            }
        }

        hash_field(&mut hasher, b"reward_receipts");
        let mut receipts = self.connection.prepare(
            "SELECT receipt_id, session_id, provider_address, receiver_address,
                    content_hash, epoch, receipt_nonce, receipt_json, score_bytes,
                    protocol_subsidy, user_resource_fee, emission_phase, status
             FROM reward_receipts ORDER BY receipt_id",
        )?;
        let receipt_rows = receipts.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, String>(12)?,
            ))
        })?;
        for row in receipt_rows {
            let (
                id,
                session_id,
                provider,
                receiver,
                content_hash,
                epoch,
                nonce,
                json,
                score,
                subsidy,
                fee,
                emission_phase,
                status,
            ) = row?;
            for field in [
                id,
                session_id,
                provider,
                receiver,
                content_hash,
                json,
                status,
            ] {
                hash_field(&mut hasher, field.as_bytes());
            }
            for (value, label) in [
                (epoch, "receipt epoch"),
                (nonce, "receipt nonce"),
                (score, "receipt score"),
                (subsidy, "receipt subsidy"),
                (fee, "receipt fee"),
            ] {
                hash_field(&mut hasher, &u64_from_sql(value, label)?.to_be_bytes());
            }
            hash_optional_i64(&mut hasher, emission_phase);
        }

        hash_field(&mut hasher, b"epochs");
        let mut epochs = self.connection.prepare(
            "SELECT epoch, receipts_processed, protocol_subsidy FROM epochs ORDER BY epoch",
        )?;
        let epoch_rows = epochs.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut epoch_count = 0_u64;
        for row in epoch_rows {
            let (epoch, processed, subsidy) = row?;
            hash_field(&mut hasher, &u64_from_sql(epoch, "epoch")?.to_be_bytes());
            hash_field(
                &mut hasher,
                &u64_from_sql(processed, "processed receipt count")?.to_be_bytes(),
            );
            hash_field(
                &mut hasher,
                &u64_from_sql(subsidy, "epoch subsidy")?.to_be_bytes(),
            );
            epoch_count = epoch_count.checked_add(1).context("epoch count overflow")?;
        }

        Ok(LedgerAuditReport {
            audit_version: 1,
            network_id: self.network,
            state_root_sha256: hex::encode(hasher.finalize()),
            accounts: status.accounts,
            transactions: status.transactions,
            contribution_receipts: status.contribution_receipts,
            epochs: epoch_count,
            supply: status.supply,
            invariants_valid: true,
        })
    }

    fn pending_receipts(&self, epoch: u64) -> Result<Vec<ContributionReceipt>> {
        let mut statement = self.connection.prepare(
            "SELECT receipt_json FROM reward_receipts
             WHERE epoch = ?1 AND status = 'pending' ORDER BY receipt_id",
        )?;
        let rows = statement.query_map([sql_u64(epoch)?], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
}

fn apply_transaction(
    tx: &SqlTransaction<'_>,
    network: NetworkId,
    transaction: &Transaction,
    epoch: u64,
) -> Result<String> {
    if transaction.network_id != network {
        bail!("transaction belongs to another network");
    }
    transaction.verify_signature()?;
    let from = Address::parse_for_network(&transaction.from, network)?;
    let to = Address::parse_for_network(&transaction.to, network)?;
    if from.is_reserve() {
        bail!("permanent reserve may never be a transaction sender");
    }
    if from == to {
        bail!("self-transfers are not accepted");
    }
    if transaction.amount == Amount::ZERO {
        bail!("transaction amount must be greater than zero");
    }
    if transaction.created_epoch > epoch || transaction.expiry_epoch < epoch {
        bail!("transaction is not valid in the current epoch");
    }
    let sender = tx
        .query_row(
            "SELECT balance, nonce, public_key FROM accounts WHERE address = ?1",
            [from.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
        .context("transaction sender does not exist")?;
    let sender_balance = amount_from_sql(sender.0)?;
    let sender_nonce = u64_from_sql(sender.1, "sender nonce")?;
    if sender_nonce != transaction.nonce {
        bail!("incorrect transaction nonce");
    }
    if sender
        .2
        .as_ref()
        .is_some_and(|key| key != &transaction.from_public_key)
    {
        bail!("sender public key does not match registered account key");
    }
    let new_sender_balance = sender_balance.checked_sub(transaction.amount)?;
    let new_nonce = sender_nonce
        .checked_add(1)
        .context("account nonce overflow")?;
    let receiver_balance = tx
        .query_row(
            "SELECT balance FROM accounts WHERE address = ?1",
            [to.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(amount_from_sql)
        .transpose()?
        .unwrap_or(Amount::ZERO);
    let new_receiver_balance = receiver_balance.checked_add(transaction.amount)?;
    let transaction_id = transaction.id()?;
    let json = serde_json::to_string(transaction)?;
    if json.len() > 16 * 1024 {
        bail!("transaction exceeds size limit");
    }
    tx.execute(
        "INSERT INTO transactions(
            transaction_id, transaction_json, from_address, to_address,
            amount, nonce, accepted_epoch
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            transaction_id,
            json,
            from.as_str(),
            to.as_str(),
            amount_to_sql(transaction.amount)?,
            sql_u64(transaction.nonce)?,
            sql_u64(epoch)?,
        ],
    )?;
    tx.execute(
        "UPDATE accounts SET balance = ?1, nonce = ?2,
            public_key = COALESCE(public_key, ?3) WHERE address = ?4",
        params![
            amount_to_sql(new_sender_balance)?,
            sql_u64(new_nonce)?,
            transaction.from_public_key,
            from.as_str(),
        ],
    )?;
    tx.execute(
        "INSERT INTO accounts(address, balance, nonce, public_key)
         VALUES (?1, ?2, 0, NULL)
         ON CONFLICT(address) DO UPDATE SET balance = excluded.balance",
        params![to.as_str(), amount_to_sql(new_receiver_balance)?],
    )?;
    if to.is_reserve() {
        let before = read_supply_tx(tx)?;
        let reserve = before.reserve_balance.checked_add(transaction.amount)?;
        write_supply(
            tx,
            SupplyState::from_components(before.network_emitted_supply, reserve)?,
        )?;
    }
    Ok(transaction_id)
}

fn credit_account(
    tx: &SqlTransaction<'_>,
    address: &str,
    public_key: &str,
    amount: Amount,
) -> Result<()> {
    let existing = tx
        .query_row(
            "SELECT balance, public_key FROM accounts WHERE address = ?1",
            [address],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let balance = existing
        .as_ref()
        .map(|row| amount_from_sql(row.0))
        .transpose()?
        .unwrap_or(Amount::ZERO)
        .checked_add(amount)?;
    if existing
        .as_ref()
        .and_then(|row| row.1.as_ref())
        .is_some_and(|registered| registered != public_key)
    {
        bail!("reward provider public key conflicts with account");
    }
    tx.execute(
        "INSERT INTO accounts(address, balance, nonce, public_key)
         VALUES (?1, ?2, 0, ?3)
         ON CONFLICT(address) DO UPDATE SET
            balance = excluded.balance,
            public_key = COALESCE(accounts.public_key, excluded.public_key)",
        params![address, amount_to_sql(balance)?, public_key],
    )?;
    Ok(())
}

fn read_supply(connection: &Connection) -> Result<SupplyState> {
    connection
        .query_row(
            "SELECT minted_supply, network_emitted_supply, reserve_balance,
                    circulating_supply FROM supply_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .map_err(Into::into)
        .and_then(supply_from_sql)
}

fn read_supply_tx(tx: &SqlTransaction<'_>) -> Result<SupplyState> {
    let values = tx.query_row(
        "SELECT minted_supply, network_emitted_supply, reserve_balance,
                circulating_supply FROM supply_state WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )?;
    supply_from_sql(values)
}

fn supply_from_sql(values: (i64, i64, i64, i64)) -> Result<SupplyState> {
    let state = SupplyState {
        minted_supply: amount_from_sql(values.0)?,
        network_emitted_supply: amount_from_sql(values.1)?,
        reserve_balance: amount_from_sql(values.2)?,
        circulating_supply: amount_from_sql(values.3)?,
    };
    state.validate()?;
    Ok(state)
}

fn write_supply(tx: &SqlTransaction<'_>, state: SupplyState) -> Result<()> {
    state.validate()?;
    tx.execute(
        "UPDATE supply_state SET minted_supply = ?1, network_emitted_supply = ?2,
            reserve_balance = ?3, circulating_supply = ?4 WHERE singleton = 1",
        params![
            amount_to_sql(state.minted_supply)?,
            amount_to_sql(state.network_emitted_supply)?,
            amount_to_sql(state.reserve_balance)?,
            amount_to_sql(state.circulating_supply)?,
        ],
    )?;
    Ok(())
}

fn verify_account_sum(tx: &SqlTransaction<'_>, expected: Amount) -> Result<()> {
    let sum: i64 = tx.query_row(
        "SELECT COALESCE(SUM(balance), 0) FROM accounts",
        [],
        |row| row.get(0),
    )?;
    if amount_from_sql(sum)? != expected {
        bail!("transaction would violate supply conservation");
    }
    Ok(())
}

fn amount_to_sql(amount: Amount) -> Result<i64> {
    i64::try_from(amount.atomic()).context("SMD amount exceeds SQLite integer range")
}

fn amount_from_sql(value: i64) -> Result<Amount> {
    Ok(Amount::from_atomic(u64_from_sql(value, "SMD amount")?))
}

fn sql_u64(value: u64) -> Result<i64> {
    i64::try_from(value).context("value exceeds SQLite integer range")
}

fn u64_from_sql(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).with_context(|| format!("negative {label} in ledger"))
}

fn count(connection: &Connection, table: &str) -> Result<u64> {
    let sql = match table {
        "accounts" => "SELECT COUNT(*) FROM accounts",
        "transactions" => "SELECT COUNT(*) FROM transactions",
        "reward_receipts" => "SELECT COUNT(*) FROM reward_receipts",
        _ => bail!("invalid internal table name"),
    };
    let value: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    u64_from_sql(value, "row count")
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn hash_optional_field(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hash_field(hasher, b"some");
            hash_field(hasher, value.as_bytes());
        }
        None => hash_field(hasher, b"none"),
    }
}

fn hash_optional_i64(hasher: &mut Sha256, value: Option<i64>) {
    match value {
        Some(value) => {
            hash_field(hasher, b"some");
            hash_field(hasher, &value.to_be_bytes());
        }
        None => hash_field(hasher, b"none"),
    }
}
