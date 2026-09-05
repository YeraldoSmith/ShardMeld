use anyhow::Result;

use crate::ContributionReceipt;

pub trait ConsensusEngine {
    fn order_receipts(&self, receipts: &mut [ContributionReceipt]) -> Result<()>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DevnetAuthorityConsensus;

impl ConsensusEngine for DevnetAuthorityConsensus {
    fn order_receipts(&self, receipts: &mut [ContributionReceipt]) -> Result<()> {
        receipts.sort_by_cached_key(|receipt| receipt.id().unwrap_or_default());
        Ok(())
    }
}
