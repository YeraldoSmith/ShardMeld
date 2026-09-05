# SMD v0.1 devnet acceptance evidence

Status: **DEVNET / EXPERIMENTAL — NOT MAINNET — NOT REAL-MONEY READY**.

The final release build was exercised with:

```bash
./target/release/shardmeld smd devnet scenario \
  --ledger /tmp/shardmeld-smd-v01/smd.db \
  --json /tmp/shardmeld-smd-v01/scenario.json
```

Observed deterministic state:

- one receiver-signed CDC reconstruction receipt for 2 GiB;
- weighted score: 2,684,354,560 bytes;
- protocol subsidy: 25.00000000 SMD;
- Alice transferred 5.00000000 SMD to Bob;
- Bob transferred 2.00000000 SMD to `SMD_PERMANENT_RESERVE`;
- final Alice balance: 20.00000000 SMD;
- final Bob balance: 3.00000000 SMD;
- final reserve: 1,000,002.00000000 SMD;
- database reopen retained 3 accounts, 2 transactions, and 1 rewarded receipt;
- minted, emitted, reserve, circulating, and account-sum invariants passed.
- deterministic audit root after restart:
  `4d637400600dc09fa853b2f8bac9c68fef102cfe11e0956c0983a28ace34bd3c`.

`scenario.json` and `audit.json` store amounts as integer atomic units. This controlled local
run proves implementation consistency and persistence only. It does not prove
public-network consensus, Sybil resistance, market value, or mainnet safety.
