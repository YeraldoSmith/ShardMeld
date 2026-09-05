use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::amount::Amount;

pub const MAX_SUPPLY: Amount = Amount::from_atomic(1_200_000_000_000_000);
pub const GENESIS_RESERVE: Amount = Amount::from_atomic(100_000_000_000_000);
pub const MAX_NETWORK_EMISSION: Amount = Amount::from_atomic(1_100_000_000_000_000);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupplyState {
    pub minted_supply: Amount,
    pub network_emitted_supply: Amount,
    pub reserve_balance: Amount,
    pub circulating_supply: Amount,
}

impl SupplyState {
    pub fn genesis() -> Self {
        Self {
            minted_supply: GENESIS_RESERVE,
            network_emitted_supply: Amount::ZERO,
            reserve_balance: GENESIS_RESERVE,
            circulating_supply: Amount::ZERO,
        }
    }

    pub fn from_components(network_emitted: Amount, reserve_balance: Amount) -> Result<Self> {
        let minted = GENESIS_RESERVE.checked_add(network_emitted)?;
        let circulating = minted.checked_sub(reserve_balance)?;
        let state = Self {
            minted_supply: minted,
            network_emitted_supply: network_emitted,
            reserve_balance,
            circulating_supply: circulating,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(self) -> Result<()> {
        if self.minted_supply.atomic() > MAX_SUPPLY.atomic() {
            bail!("SMD minted supply exceeds the 12,000,000 SMD cap");
        }
        if self.network_emitted_supply.atomic() > MAX_NETWORK_EMISSION.atomic() {
            bail!("SMD network emission exceeds the 11,000,000 SMD cap");
        }
        if self.minted_supply != GENESIS_RESERVE.checked_add(self.network_emitted_supply)? {
            bail!("invalid SMD minted supply accounting");
        }
        if self.circulating_supply != self.minted_supply.checked_sub(self.reserve_balance)? {
            bail!("invalid SMD circulating supply accounting");
        }
        Ok(())
    }
}
