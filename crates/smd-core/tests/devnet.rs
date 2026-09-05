use smd_core::{
    Address, Amount, AntiFraudPolicy, ContributionReceipt, DevnetAuthorityConsensus,
    FreeLanePolicy, GENESIS_RESERVE, Ledger, MAX_NETWORK_EMISSION, MAX_SUPPLY, NetworkId,
    ProtocolPricingEngine, ServiceType, Transaction, V01FreeLanePolicy, V01PricingEngine,
    VersionedEmissionPolicy, Wallet, run_devnet_scenario,
};
use tempfile::TempDir;

fn wallet(byte: u8) -> Wallet {
    Wallet::from_secret_bytes(NetworkId::Devnet, [byte; 32])
}

fn open_ledger(directory: &TempDir) -> Ledger {
    Ledger::open(&directory.path().join("smd.db"), NetworkId::Devnet).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn receipt(
    provider: &Wallet,
    receiver: &Wallet,
    session: &str,
    content_byte: u8,
    bytes: u64,
    service: ServiceType,
    epoch: u64,
    nonce: u64,
) -> ContributionReceipt {
    ContributionReceipt::confirmed(
        provider,
        receiver,
        session.to_owned(),
        hex::encode([content_byte; 32]),
        bytes,
        service,
        epoch,
        nonce,
    )
    .unwrap()
}

fn reward_provider(
    ledger: &mut Ledger,
    provider: &Wallet,
    receiver: &Wallet,
    epoch: u64,
) -> Amount {
    let receipt = receipt(
        provider,
        receiver,
        &format!("session-{epoch}"),
        epoch as u8,
        1_073_741_824,
        ServiceType::StandardUpload,
        epoch,
        epoch,
    );
    ledger.submit_contribution(&receipt).unwrap();
    ledger
        .mine_rewards(epoch, &DevnetAuthorityConsensus)
        .unwrap()
        .protocol_subsidy
}

#[test]
fn legal_signature_passes_and_modified_signature_fails() {
    let alice = wallet(1);
    let bob = wallet(2);
    let transaction = Transaction::signed(
        &alice,
        &bob.address().unwrap(),
        "1.00000000".parse().unwrap(),
        0,
        1,
        2,
    )
    .unwrap();
    transaction.verify_signature().unwrap();

    let mut altered = transaction;
    altered.amount = "2.00000000".parse().unwrap();
    assert!(altered.verify_signature().is_err());
}

#[test]
fn normal_transfer_nonce_balance_and_reserve_rules_hold() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let alice = wallet(3);
    let bob = wallet(4);
    let reward = reward_provider(&mut ledger, &alice, &bob, 1);
    assert!(reward > Amount::ZERO);

    let transfer_amount: Amount = "3.00000000".parse().unwrap();
    let transfer =
        Transaction::signed(&alice, &bob.address().unwrap(), transfer_amount, 0, 1, 5).unwrap();
    ledger.submit_transaction(&transfer, 1).unwrap();
    assert_eq!(
        ledger.account(&bob.address().unwrap()).unwrap().balance,
        transfer_amount
    );
    assert_eq!(ledger.account(&alice.address().unwrap()).unwrap().nonce, 1);

    assert!(ledger.submit_transaction(&transfer, 1).is_err());
    let too_much = Transaction::signed(&bob, &alice.address().unwrap(), reward, 0, 1, 5).unwrap();
    assert!(ledger.submit_transaction(&too_much, 1).is_err());

    let reserve_before = ledger.status().unwrap().supply.reserve_balance;
    let burn: Amount = "1.00000000".parse().unwrap();
    let to_reserve = Transaction::signed(&bob, &Address::reserve(), burn, 0, 1, 5).unwrap();
    ledger.submit_transaction(&to_reserve, 1).unwrap();
    let status = ledger.status().unwrap();
    assert_eq!(
        status.supply.reserve_balance,
        reserve_before.checked_add(burn).unwrap()
    );
    assert_eq!(
        status.supply.circulating_supply,
        status
            .supply
            .minted_supply
            .checked_sub(status.supply.reserve_balance)
            .unwrap()
    );

    let mut forged_reserve =
        Transaction::signed(&alice, &bob.address().unwrap(), burn, 1, 1, 5).unwrap();
    forged_reserve.from = Address::reserve().to_string();
    assert!(ledger.submit_transaction(&forged_reserve, 1).is_err());
    assert_eq!(
        ledger.status().unwrap().supply.reserve_balance,
        status.supply.reserve_balance
    );
}

#[test]
fn invalid_and_replayed_contribution_receipts_never_reward() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let alice = wallet(5);
    let bob = wallet(6);
    let valid = receipt(
        &alice,
        &bob,
        "unique-session",
        7,
        10_000_000,
        ServiceType::RareDataUpload,
        2,
        0,
    );
    ledger.submit_contribution(&valid).unwrap();
    assert!(ledger.submit_contribution(&valid).is_err());

    let same_session = receipt(
        &alice,
        &bob,
        "unique-session",
        8,
        10_000_000,
        ServiceType::StandardUpload,
        2,
        1,
    );
    assert!(ledger.submit_contribution(&same_session).is_err());

    let repeated_nonce = receipt(
        &alice,
        &bob,
        "different-session-same-nonce",
        11,
        10_000_000,
        ServiceType::StandardUpload,
        2,
        0,
    );
    assert!(ledger.submit_contribution(&repeated_nonce).is_err());

    let mut corrupted = receipt(
        &alice,
        &bob,
        "corrupt-session",
        9,
        10_000_000,
        ServiceType::StandardUpload,
        2,
        2,
    );
    corrupted.content_verified = false;
    assert!(ledger.submit_contribution(&corrupted).is_err());

    let mut unsigned = receipt(
        &alice,
        &bob,
        "unsigned-session",
        10,
        10_000_000,
        ServiceType::StandardUpload,
        2,
        3,
    );
    unsigned.receiver_signature.clear();
    assert!(ledger.submit_contribution(&unsigned).is_err());

    let summary = ledger.mine_rewards(2, &DevnetAuthorityConsensus).unwrap();
    assert_eq!(summary.receipts_processed, 1);
    assert!(summary.protocol_subsidy > Amount::ZERO);
    assert_eq!(ledger.status().unwrap().contribution_receipts, 1);
}

#[test]
fn repeated_pairs_decay_and_content_loops_are_stopped() {
    let policy = AntiFraudPolicy::default();
    let first = policy
        .score(1_000, ServiceType::StandardUpload, 0, 0)
        .unwrap();
    let repeated = policy
        .score(1_000, ServiceType::StandardUpload, 1, 1)
        .unwrap();
    let rare = policy
        .score(1_000, ServiceType::RareDataUpload, 0, 0)
        .unwrap();
    let reconstructed = policy
        .score(1_000, ServiceType::CdcReconstructionUpload, 0, 0)
        .unwrap();
    assert!(repeated < first);
    assert!(rare > first);
    assert!(reconstructed > first);
    assert!(
        policy
            .score(
                1_000,
                ServiceType::StandardUpload,
                3,
                policy.max_same_content_per_pair_epoch,
            )
            .is_err()
    );
}

#[test]
fn no_contribution_means_no_emission_and_pricing_stays_disabled() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let before = ledger.status().unwrap().supply;
    let summary = ledger.mine_rewards(9, &DevnetAuthorityConsensus).unwrap();
    assert_eq!(summary.receipts_processed, 0);
    assert_eq!(summary.protocol_subsidy, Amount::ZERO);
    assert_eq!(ledger.status().unwrap().supply, before);
    assert!(V01FreeLanePolicy.basic_access_is_free());
    assert!(!V01PricingEngine.enabled());
    assert_eq!(V01PricingEngine.user_resource_fee(u64::MAX), Amount::ZERO);
}

#[test]
fn atomic_batch_rolls_back_every_change_on_midway_failure() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let alice = wallet(7);
    let bob = wallet(8);
    reward_provider(&mut ledger, &alice, &bob, 1);
    let before = ledger.status().unwrap();
    let good = Transaction::signed(
        &alice,
        &bob.address().unwrap(),
        "1.00000000".parse().unwrap(),
        0,
        1,
        10,
    )
    .unwrap();
    let duplicate_nonce = Transaction::signed(
        &alice,
        &Address::reserve(),
        "1.00000000".parse().unwrap(),
        0,
        1,
        10,
    )
    .unwrap();
    assert!(
        ledger
            .submit_transactions_atomically(&[good, duplicate_nonce], 1)
            .is_err()
    );
    assert_eq!(ledger.status().unwrap(), before);
    assert_eq!(
        ledger.account(&bob.address().unwrap()).unwrap().balance,
        Amount::ZERO
    );
}

#[test]
fn cross_network_replay_and_expired_transactions_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let mainnet_alice = Wallet::from_secret_bytes(NetworkId::Mainnet, [27; 32]);
    let mainnet_bob = Wallet::from_secret_bytes(NetworkId::Mainnet, [28; 32]);
    let cross_network = Transaction::signed(
        &mainnet_alice,
        &mainnet_bob.address().unwrap(),
        Amount::from_atomic(1),
        0,
        1,
        10,
    )
    .unwrap();
    assert!(ledger.submit_transaction(&cross_network, 1).is_err());

    let devnet_alice = wallet(29);
    let receiver = wallet(30);
    reward_provider(&mut ledger, &devnet_alice, &receiver, 1);
    let expired = Transaction::signed(
        &devnet_alice,
        &receiver.address().unwrap(),
        Amount::from_atomic(1),
        0,
        1,
        2,
    )
    .unwrap();
    assert!(ledger.submit_transaction(&expired, 3).is_err());
}

#[test]
fn restart_preserves_balances_nonces_supply_and_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("smd.db");
    let report = run_devnet_scenario(&path).unwrap();
    assert_eq!(report.ledger_after_restart.transactions, 2);
    assert_eq!(report.ledger_after_restart.contribution_receipts, 1);
    assert!(report.ledger_after_restart.supply.reserve_balance > GENESIS_RESERVE);
    assert_eq!(report.bob_balance, "3.00000000".parse().unwrap());

    let reopened = Ledger::open(&path, NetworkId::Devnet).unwrap();
    reopened.verify_invariants().unwrap();
    assert_eq!(
        reopened.account(&Address::reserve()).unwrap().balance,
        report.ledger_after_restart.supply.reserve_balance
    );
    assert_eq!(reopened.transactions(10).unwrap().len(), 2);
    assert_eq!(reopened.reward_receipts(10).unwrap().len(), 1);
}

#[test]
fn reserve_balance_never_decreases() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let alice = wallet(9);
    let receiver = wallet(10);
    reward_provider(&mut ledger, &alice, &receiver, 1);
    let mut previous = ledger.status().unwrap().supply.reserve_balance;
    for nonce in 0..10 {
        let transaction = Transaction::signed(
            &alice,
            &Address::reserve(),
            Amount::from_atomic(1),
            nonce,
            1,
            10,
        )
        .unwrap();
        ledger.submit_transaction(&transaction, 1).unwrap();
        let current = ledger.status().unwrap().supply.reserve_balance;
        assert!(current >= previous);
        previous = current;
    }
}

#[test]
fn minted_supply_never_exceeds_cap() {
    for remaining in 0..100_u64 {
        let emitted = Amount::from_atomic(MAX_NETWORK_EMISSION.atomic() - remaining);
        let quote = VersionedEmissionPolicy::quote(u64::MAX, emitted).unwrap();
        let final_emitted = emitted.checked_add(quote.protocol_subsidy).unwrap();
        let final_minted = GENESIS_RESERVE.checked_add(final_emitted).unwrap();
        assert!(final_emitted <= MAX_NETWORK_EMISSION);
        assert!(final_minted <= MAX_SUPPLY);
    }
}

#[test]
fn integer_overflow_is_rejected() {
    assert!(
        Amount::from_atomic(u64::MAX)
            .checked_add(Amount::from_atomic(1))
            .is_err()
    );
    assert!(Amount::ZERO.checked_sub(Amount::from_atomic(1)).is_err());
}

#[test]
fn per_epoch_address_reward_cap_is_enforced() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let provider = wallet(21);
    let receiver = wallet(22);
    let huge = receipt(
        &provider,
        &receiver,
        "large-contribution",
        23,
        200 * 1_073_741_824,
        ServiceType::StandardUpload,
        4,
        0,
    );
    ledger.submit_contribution(&huge).unwrap();
    let reward = ledger.mine_rewards(4, &DevnetAuthorityConsensus).unwrap();
    assert_eq!(reward.protocol_subsidy, "1000.00000000".parse().unwrap());
}

#[test]
fn abnormal_same_content_cycle_is_rejected_by_ledger() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let provider = wallet(24);
    let receiver = wallet(25);
    for nonce in 0..3 {
        let candidate = receipt(
            &provider,
            &receiver,
            &format!("loop-session-{nonce}"),
            26,
            1_000,
            ServiceType::StandardUpload,
            7,
            nonce,
        );
        ledger.submit_contribution(&candidate).unwrap();
    }
    let fourth = receipt(
        &provider,
        &receiver,
        "loop-session-3",
        26,
        1_000,
        ServiceType::StandardUpload,
        7,
        3,
    );
    assert!(ledger.submit_contribution(&fourth).is_err());
}

#[test]
fn transaction_conserves_supply() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let alice = wallet(11);
    let bob = wallet(12);
    reward_provider(&mut ledger, &alice, &bob, 1);
    let minted = ledger.status().unwrap().supply.minted_supply;
    for nonce in 0..20 {
        let transaction = Transaction::signed(
            &alice,
            &bob.address().unwrap(),
            Amount::from_atomic(1),
            nonce,
            1,
            10,
        )
        .unwrap();
        ledger.submit_transaction(&transaction, 1).unwrap();
        let status = ledger.status().unwrap();
        assert_eq!(status.supply.minted_supply, minted);
        ledger.verify_invariants().unwrap();
    }
}

#[test]
fn no_negative_balance() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let unfunded = wallet(31);
    let receiver = wallet(32);
    let transaction = Transaction::signed(
        &unfunded,
        &receiver.address().unwrap(),
        Amount::from_atomic(1),
        0,
        1,
        10,
    )
    .unwrap();
    assert!(ledger.submit_transaction(&transaction, 1).is_err());
    assert_eq!(
        ledger
            .account(&unfunded.address().unwrap())
            .unwrap()
            .balance,
        Amount::ZERO
    );
    ledger.verify_invariants().unwrap();
}

#[test]
fn only_reward_mint_can_increase_supply() {
    let directory = tempfile::tempdir().unwrap();
    let mut ledger = open_ledger(&directory);
    let alice = wallet(13);
    let bob = wallet(14);
    let genesis = ledger.status().unwrap().supply.minted_supply;
    reward_provider(&mut ledger, &alice, &bob, 1);
    let after_reward = ledger.status().unwrap().supply.minted_supply;
    assert!(after_reward > genesis);
    let transaction = Transaction::signed(
        &alice,
        &bob.address().unwrap(),
        Amount::from_atomic(1),
        0,
        1,
        10,
    )
    .unwrap();
    ledger.submit_transaction(&transaction, 1).unwrap();
    assert_eq!(ledger.status().unwrap().supply.minted_supply, after_reward);
}

#[test]
fn explicit_devnet_wallet_file_round_trips() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("alice.devnet-wallet.json");
    let alice = wallet(15);
    alice.export_devnet_test_file(&path).unwrap();
    let imported = Wallet::import_devnet_test_file(&path).unwrap();
    assert_eq!(alice.address().unwrap(), imported.address().unwrap());
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("smd-devnet-test-wallet"));
    assert!(!format!("{alice:?}").contains(&hex::encode([15; 32])));
}
