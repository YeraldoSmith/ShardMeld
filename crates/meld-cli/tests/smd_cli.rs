use std::path::Path;
use std::process::{Command, Output};

fn shardmeld(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_shardmeld"))
        .args(arguments)
        .output()
        .unwrap()
}

fn success(arguments: &[&str]) -> String {
    let output = shardmeld(arguments);
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn cli_completes_devnet_economy_flow_and_persists_state() {
    let directory = tempfile::tempdir().unwrap();
    let alice = directory.path().join("alice.wallet.json");
    let bob = directory.path().join("bob.wallet.json");
    let alice_backup = directory.path().join("alice.backup.json");
    let alice_imported = directory.path().join("alice.imported.json");
    let ledger = directory.path().join("smd.db");

    success(&["smd", "wallet", "create", "--out", path(&alice)]);
    success(&["smd", "wallet", "create", "--out", path(&bob)]);
    let alice_address = success(&["smd", "wallet", "address", "--wallet", path(&alice)])
        .trim()
        .to_owned();
    let bob_address = success(&["smd", "wallet", "receive", "--wallet", path(&bob)])
        .trim()
        .to_owned();
    assert!(alice_address.starts_with("smddev1"));
    assert!(bob_address.starts_with("smddev1"));
    assert_ne!(alice_address, bob_address);

    success(&["smd", "devnet", "genesis", "--ledger", path(&ledger)]);
    success(&[
        "smd",
        "contribution",
        "record",
        "--ledger",
        path(&ledger),
        "--provider-wallet",
        path(&alice),
        "--receiver-wallet",
        path(&bob),
        "--session-id",
        "cli-integration-session",
        "--content-hash",
        &"ab".repeat(32),
        "--bytes",
        "1073741824",
        "--service-type",
        "CDC",
        "--epoch",
        "1",
        "--nonce",
        "0",
    ]);
    let mined = success(&[
        "smd",
        "devnet",
        "mine-rewards",
        "--ledger",
        path(&ledger),
        "--epoch",
        "1",
    ]);
    assert!(mined.contains("receipts=1"));
    assert!(mined.contains("user_fees=0.00000000 SMD"));

    success(&[
        "smd",
        "send",
        "--wallet",
        path(&alice),
        "--ledger",
        path(&ledger),
        "--to",
        &bob_address,
        "--amount",
        "3.00000000",
        "--epoch",
        "1",
        "--expiry-epoch",
        "10",
    ]);
    success(&[
        "smd",
        "send",
        "--wallet",
        path(&bob),
        "--ledger",
        path(&ledger),
        "--to",
        "SMD_PERMANENT_RESERVE",
        "--amount",
        "1.00000000",
        "--epoch",
        "1",
        "--expiry-epoch",
        "10",
    ]);

    let bob_balance = success(&[
        "smd",
        "wallet",
        "balance",
        "--wallet",
        path(&bob),
        "--ledger",
        path(&ledger),
    ]);
    assert!(bob_balance.contains("balance=2.00000000 SMD"));
    let reserve = success(&["smd", "reserve", "status", "--ledger", path(&ledger)]);
    assert!(reserve.contains("balance=1000001.00000000 SMD"));
    let status = success(&["smd", "ledger", "status", "--ledger", path(&ledger)]);
    assert!(status.contains("transactions=2"));
    assert!(status.contains("receipts=1"));

    success(&[
        "smd",
        "wallet",
        "export-backup",
        "--wallet",
        path(&alice),
        "--out",
        path(&alice_backup),
    ]);
    success(&[
        "smd",
        "wallet",
        "import-backup",
        "--backup",
        path(&alice_backup),
        "--out",
        path(&alice_imported),
    ]);
    let imported_address = success(&[
        "smd",
        "wallet",
        "address",
        "--wallet",
        path(&alice_imported),
    ]);
    assert_eq!(imported_address.trim(), alice_address);
}
