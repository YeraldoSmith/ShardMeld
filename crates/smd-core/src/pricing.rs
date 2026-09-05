use crate::amount::Amount;

pub trait FreeLanePolicy {
    fn basic_access_is_free(&self) -> bool;
}

pub trait ProtocolPricingEngine {
    fn enabled(&self) -> bool;
    fn user_resource_fee(&self, bytes: u64) -> Amount;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct V01FreeLanePolicy;

impl FreeLanePolicy for V01FreeLanePolicy {
    fn basic_access_is_free(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct V01PricingEngine;

impl ProtocolPricingEngine for V01PricingEngine {
    fn enabled(&self) -> bool {
        false
    }

    fn user_resource_fee(&self, _bytes: u64) -> Amount {
        Amount::ZERO
    }
}
