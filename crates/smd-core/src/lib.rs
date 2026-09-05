//! ShardMeld SMD v0.1 devnet economy layer.
//!
//! This crate is deliberately independent from the BitTorrent engine. It is
//! experimental devnet software and is not a mainnet or real-money system.

mod address;
mod amount;
mod consensus;
mod contribution;
mod crypto;
mod devnet;
mod emission;
mod ledger;
mod pricing;
mod rewards;
mod supply;
mod transaction;
mod wallet;

pub use address::{Address, NetworkId, PERMANENT_RESERVE_ADDRESS};
pub use amount::{ATOMIC_UNITS_PER_SMD, Amount};
pub use consensus::{ConsensusEngine, DevnetAuthorityConsensus};
pub use contribution::{ContributionReceipt, ServiceType};
pub use devnet::{DevnetScenarioReport, run_devnet_scenario};
pub use emission::{EmissionQuote, VersionedEmissionPolicy};
pub use ledger::{Account, Ledger, LedgerStatus, RewardReceiptRecord, TransactionRecord};
pub use pricing::{FreeLanePolicy, ProtocolPricingEngine, V01FreeLanePolicy, V01PricingEngine};
pub use rewards::{AntiFraudPolicy, RewardSummary};
pub use supply::{GENESIS_RESERVE, MAX_NETWORK_EMISSION, MAX_SUPPLY, SupplyState};
pub use transaction::Transaction;
pub use wallet::{DEVNET_WALLET_FILE_KIND, Wallet, WalletBackup};

pub const SMD_PROTOCOL_VERSION: u32 = 1;
pub const SMD_SYMBOL: &str = "SMD";
