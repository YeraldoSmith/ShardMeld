use serde::{Deserialize, Serialize};

pub const REPORT_FORMAT: &str = "shardmeld-report";
pub const REPORT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilitiesReport {
    pub report_format: String,
    pub report_version: u32,
    pub engine_version: String,
    pub implemented: Vec<String>,
    pub deferred: Vec<String>,
    pub limits: Vec<String>,
}

pub fn capabilities_report() -> CapabilitiesReport {
    CapabilitiesReport {
        report_format: REPORT_FORMAT.to_owned(),
        report_version: REPORT_VERSION,
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        implemented: vec![
            "authorized-local-cdc-index".to_owned(),
            "exact-sha256-reconstruction".to_owned(),
            "bittorrent-v1-single-file".to_owned(),
            "http-https-udp-trackers".to_owned(),
            "multitracker-tiers".to_owned(),
            "verified-piece-resume".to_owned(),
            "four-peer-concurrency".to_owned(),
            "rarest-first".to_owned(),
            "work-conserving-speed-adaptation".to_owned(),
            "safe-piece-endgame".to_owned(),
            "standard-cancel".to_owned(),
            "magnet-v1-local-metadata-binding".to_owned(),
            "verified-file-upload-seeding".to_owned(),
            "on-demand-index-piece-seeding".to_owned(),
            "smd-v0.1-devnet-wallets".to_owned(),
            "smd-v0.1-account-ledger".to_owned(),
            "smd-v0.1-permanent-reserve".to_owned(),
            "smd-v0.1-fixed-supply".to_owned(),
            "smd-v0.1-signed-contribution-receipts".to_owned(),
            "smd-v0.1-useful-contribution-rewards".to_owned(),
            "smd-v0.1-decreasing-emission".to_owned(),
            "smd-v0.1-free-lane".to_owned(),
            "smd-macos-keychain-wallets".to_owned(),
            "smd-ledger-state-audit".to_owned(),
        ],
        deferred: vec![
            "dht".to_owned(),
            "magnet-metadata-exchange".to_owned(),
            "peer-exchange".to_owned(),
            "bittorrent-v2-hybrid".to_owned(),
            "multi-file-torrents".to_owned(),
            "gui".to_owned(),
            "smd-mainnet".to_owned(),
            "smd-mainnet-wallet-recovery".to_owned(),
            "smd-distributed-consensus".to_owned(),
            "smd-sybil-resistant-rewards".to_owned(),
            "smd-real-money-exchange".to_owned(),
            "smd-real-resource-pricing".to_owned(),
        ],
        limits: vec![
            "explicitly-authorized-index-roots-only".to_owned(),
            "single-file-v1-torrents-only".to_owned(),
            "piece-level-resume".to_owned(),
            "piece-level-endgame".to_owned(),
            "maximum-four-active-peers".to_owned(),
            "serial-upload-connections".to_owned(),
            "manual-upload-tracker-registration".to_owned(),
            "smd-devnet-only".to_owned(),
            "smd-devnet-authority-ordering".to_owned(),
            "smd-explicit-test-wallet-files-contain-private-keys".to_owned(),
            "smd-user-resource-fee-disabled".to_owned(),
            "smd-receiver-signatures-do-not-solve-sybil-attacks".to_owned(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{REPORT_FORMAT, REPORT_VERSION, capabilities_report};

    #[test]
    fn capabilities_are_versioned_and_do_not_claim_deferred_features() {
        let report = capabilities_report();
        assert_eq!(report.report_format, REPORT_FORMAT);
        assert_eq!(report.report_version, REPORT_VERSION);
        assert_eq!(report.engine_version, env!("CARGO_PKG_VERSION"));
        let implemented = report.implemented.iter().collect::<HashSet<_>>();
        assert!(
            report
                .deferred
                .iter()
                .all(|feature| !implemented.contains(feature))
        );
    }
}
