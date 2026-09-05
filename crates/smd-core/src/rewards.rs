use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::amount::{ATOMIC_UNITS_PER_SMD, Amount};
use crate::contribution::ServiceType;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AntiFraudPolicy {
    pub version: u32,
    pub max_same_content_per_pair_epoch: u32,
    pub max_protocol_reward_per_address_epoch: Amount,
}

impl Default for AntiFraudPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            max_same_content_per_pair_epoch: 3,
            max_protocol_reward_per_address_epoch: Amount::from_atomic(
                1_000 * ATOMIC_UNITS_PER_SMD,
            ),
        }
    }
}

impl AntiFraudPolicy {
    pub fn score(
        &self,
        bytes: u64,
        service_type: ServiceType,
        prior_pair_receipts: u32,
        prior_same_content_receipts: u32,
    ) -> Result<u64> {
        if prior_same_content_receipts >= self.max_same_content_per_pair_epoch {
            bail!("abnormal repeated content loop detected");
        }
        let factor_basis_points = match service_type {
            ServiceType::StandardUpload => 10_000_u128,
            ServiceType::RareDataUpload => 15_000_u128,
            ServiceType::CdcReconstructionUpload => 12_500_u128,
        };
        let weighted = u128::from(bytes)
            .checked_mul(factor_basis_points)
            .context("contribution score overflow")?
            / 10_000;
        let decayed = weighted / u128::from(prior_pair_receipts.saturating_add(1));
        u64::try_from(decayed).context("contribution score exceeds supported range")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewardSummary {
    pub epoch: u64,
    pub receipts_processed: u64,
    pub total_score_bytes: u64,
    pub protocol_subsidy: Amount,
    pub user_resource_fees: Amount,
    pub emission_phase: u32,
}
