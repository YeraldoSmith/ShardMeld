use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::amount::{ATOMIC_UNITS_PER_SMD, Amount};
use crate::supply::MAX_NETWORK_EMISSION;

const GIB: u128 = 1_073_741_824;
const BASE_ATOMIC_PER_GIB: u64 = 10 * ATOMIC_UNITS_PER_SMD;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmissionQuote {
    pub policy_version: u32,
    pub phase: u32,
    pub score_bytes: u64,
    pub protocol_subsidy: Amount,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VersionedEmissionPolicy;

impl VersionedEmissionPolicy {
    pub const VERSION: u32 = 1;

    pub fn phase(network_emitted: Amount) -> u32 {
        let cap = MAX_NETWORK_EMISSION.atomic();
        let emitted = network_emitted.atomic().min(cap);
        let mut phase = 0_u32;
        while phase < 31 {
            let next_boundary = cap.saturating_sub(cap >> (phase + 1));
            if emitted < next_boundary {
                break;
            }
            phase += 1;
        }
        phase
    }

    pub fn quote(score_bytes: u64, network_emitted: Amount) -> Result<EmissionQuote> {
        let phase = Self::phase(network_emitted);
        let rate = (BASE_ATOMIC_PER_GIB >> phase).max(1);
        let raw = u128::from(score_bytes)
            .checked_mul(u128::from(rate))
            .context("SMD reward calculation overflow")?
            / GIB;
        let raw = if score_bytes > 0 { raw.max(1) } else { 0 };
        let remaining = MAX_NETWORK_EMISSION
            .atomic()
            .checked_sub(network_emitted.atomic())
            .context("network emission already exceeds cap")?;
        let subsidy = raw.min(u128::from(remaining));
        Ok(EmissionQuote {
            policy_version: Self::VERSION,
            phase,
            score_bytes,
            protocol_subsidy: Amount::from_atomic(
                u64::try_from(subsidy).context("SMD reward does not fit atomic amount")?,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::VersionedEmissionPolicy;
    use crate::{Amount, MAX_NETWORK_EMISSION};

    #[test]
    fn reward_rate_decreases_by_emitted_supply_phase() {
        let bytes = 1_073_741_824;
        let early = VersionedEmissionPolicy::quote(bytes, Amount::ZERO).unwrap();
        let half = VersionedEmissionPolicy::quote(
            bytes,
            Amount::from_atomic(MAX_NETWORK_EMISSION.atomic() / 2),
        )
        .unwrap();
        assert_eq!(early.phase, 0);
        assert_eq!(half.phase, 1);
        assert!(early.protocol_subsidy > half.protocol_subsidy);
    }

    #[test]
    fn emission_quote_never_crosses_cap() {
        let almost = Amount::from_atomic(MAX_NETWORK_EMISSION.atomic() - 3);
        let quote = VersionedEmissionPolicy::quote(u64::MAX, almost).unwrap();
        assert!(quote.protocol_subsidy.atomic() <= 3);
    }
}
