# ShardMeld SMD v0.1 protocol draft

> **DEVNET / EXPERIMENTAL**  
> **NOT MAINNET**  
> **NOT REAL-MONEY READY**

Status: implementation-matched draft for the isolated `smd-core` crate. This
document describes the tested v0.1 devnet rules. It is not a promise of a
future mainnet design and does not claim Sybil resistance.

## 1. Monetary units and supply

All ledger amounts are unsigned integer atomic units. Floating-point values
are forbidden in monetary state transitions.

| Parameter | Value |
| --- | ---: |
| Symbol | `SMD` |
| Decimals | 8 |
| Atomic units per SMD | 100,000,000 |
| Maximum supply | 12,000,000 SMD |
| Genesis permanent reserve | 1,000,000 SMD |
| Maximum network emission | 11,000,000 SMD |

The implementation uses checked `u64` addition and subtraction and `u128`
intermediates for score and reward multiplication. The maximum supply is far
below the `u64` limit.

The following equations are validated after state transitions:

```text
MintedSupply = GenesisReserve + NetworkEmittedSupply
MintedSupply <= 12,000,000 SMD
NetworkEmittedSupply <= 11,000,000 SMD
CirculatingSupply = MintedSupply - PermanentReserveBalance
sum(Account.balance) = MintedSupply
```

Only reward settlement may increase minted supply. Ordinary transfers conserve
the sum of account balances.

## 2. Permanent reserve

The protocol-owned address is the literal `SMD_PERMANENT_RESERVE`. It has no
spendable private key. Genesis places 1,000,000 SMD in this account.

Transfers from ordinary devnet addresses into the reserve are accepted. The
reserve cannot be a transaction sender, and self-transfers are rejected. Every
atomic transaction batch checks that the new reserve balance is not below its
previous balance. Funds sent to the reserve permanently leave circulation.

No founder, authority, or server bypass exists in core transaction validation.
The devnet authority orders receipts; it cannot bypass signature, balance,
nonce, reserve, supply, or reward checks.

## 3. Wallets and addresses

Wallets use Ed25519 keys generated locally with the operating system random
source. A devnet address is Bech32 with the `smddev` human-readable prefix and
a 20-byte SHA-256-derived public-key payload. Mainnet has a reserved `smd`
prefix but is rejected by the v0.1 ledger.

Transactions carry the sender public key. Validation derives the address again
and rejects mismatches. Private keys are never part of transactions,
contribution receipts, or ledger tables.

The current CLI deliberately supports only files marked
`smd-devnet-test-wallet`. They are created with mode `0600` on Unix, are size
limited on import, redact secrets from debug formatting, and zeroize decoded
backup secret strings on drop. These files contain private key material and
must not be treated as production wallets. macOS Keychain storage is deferred
until a production wallet threat model is approved.

## 4. Transactions

SMD v0.1 uses an account model. A transaction contains:

```text
version
network_id
from
from_public_key
to
amount
nonce
created_epoch
expiry_epoch
signature
```

The Ed25519 signature uses the domain
`ShardMeld/SMD/devnet/transaction/v1\0` and covers every semantic field except
the redundant public-key encoding and signature itself. The sender address is
cryptographically bound to the public key before signature acceptance.

A transaction is accepted only when:

- version and network are supported;
- address encodings and signature are valid;
- the sender exists and is not the permanent reserve;
- the nonce exactly equals the account nonce;
- `created_epoch <= current_epoch <= expiry_epoch`;
- amount is greater than zero and the balance is sufficient;
- all arithmetic is checked;
- the state transition conserves supply.

Multiple transactions may be submitted as one SQLite transaction. Any failure
rolls the entire batch back.

## 5. Ledger

The independent SQLite ledger stores:

- `accounts` — address, balance, nonce, optional registered public key;
- `transactions` — immutable accepted transaction data;
- `epochs` — deterministic reward settlement summaries;
- `reward_receipts` — signed contribution evidence and reward components;
- `supply_state` — minted, emitted, reserve, and circulating totals;
- `metadata` — schema version and network binding.

Foreign keys, schema checks, bounded external serialization, checked Rust
arithmetic, and atomic SQLite transactions are used together. Opening an
existing database verifies the supply and account-sum invariants before it is
used.

## 6. Useful contribution receipts

The v0.1 Proof of Useful Distribution input is a receiver-confirmed receipt:

```text
version
network_id
provider_address
provider_public_key
receiver_address
receiver_public_key
session_id
content_hash
bytes_delivered
service_type
epoch
nonce
content_verified
receiver_signature
```

The receiver signs with the domain
`ShardMeld/SMD/devnet/contribution-receipt/v1\0`. Both addresses are derived
again from their public keys. The provider cannot self-confirm a receipt. Empty
sessions, zero bytes, malformed SHA-256 hashes, unverified content, bad
signatures, repeated sessions, and repeated provider/receiver receipt nonces
are rejected before rewards are considered.

Service factors are versioned protocol rules using basis points:

| Service | Factor |
| --- | ---: |
| `STANDARD_UPLOAD` | 1.00x |
| `RARE_DATA_UPLOAD` | 1.50x |
| `CDC_RECONSTRUCTION_UPLOAD` | 1.25x |

The fixed-point score is:

```text
score_bytes = bytes_delivered * service_factor / 10,000
score_bytes = score_bytes / (1 + prior_pair_receipts_in_epoch)
```

No floating-point operation participates in scoring.

## 7. Anti-fraud boundary

`AntiFraudPolicy` v1 implements:

- unique receipt IDs;
- unique session IDs;
- provider/receiver/nonce replay rejection;
- high-frequency provider/receiver reward decay;
- rejection after three same-content receipts for the same pair and epoch;
- a 1,000 SMD protocol subsidy cap per provider and epoch;
- rejection of self-confirmed receipts.

Receiver signatures do **not** solve Sybil attacks. Peer diversity, challenge
protocols, stake, random audits, and network reputation are future policy
inputs. Devnet SMD must not be exchanged for real assets.

## 8. Reward issuance and decay

Receipts enter a pending pool. `DevnetAuthorityConsensus` deterministically
orders receipt IDs, then the ledger revalidates and settles them. Time alone
never emits SMD; an epoch with no pending verified contribution emits zero.

Provider income is stored as two independent integer components:

```text
ProviderIncome = ProtocolSubsidy + UserResourceFee
```

In v0.1, pricing is disabled and `UserResourceFee` is always zero.

`VersionedEmissionPolicy` v1 starts at 10 SMD per scored GiB. Its phase is
selected from cumulative network emission. Boundaries asymptotically divide
the remaining emission pool in half: 50%, 75%, 87.5%, and so on. The rate is
halved at every boundary, never falls below one atomic unit per scored GiB,
and every quote is clamped to the unissued network pool. This makes later
verified contribution progressively scarcer without issuing coins merely
because time passed.

## 9. Consensus, free lane, and pricing interfaces

The replaceable `ConsensusEngine` interface controls only ordering. The
authority implementation cannot mutate balances directly.

`FreeLanePolicy` v0.1 guarantees that basic access is free.
`ProtocolPricingEngine` v0.1 is disabled and always quotes a zero user fee.
There is no paid download path or provider-defined pricing.

## 10. CLI surface

The isolated command group is available under `shardmeld smd`:

```text
wallet create|address|balance|receive|export-backup|import-backup
send
ledger status|transactions
reserve status
contribution record|status
rewards status
devnet genesis|mine-rewards|scenario
```

Every invocation prints the devnet and real-money warning to standard error.
The `scenario` command runs the complete deterministic acceptance flow and
reopens the database before reporting success.

## 11. Security and scope exclusions

The v0.1 implementation does not provide:

- a mainnet or public consensus network;
- complete Sybil resistance;
- exchange, fiat, or real-asset integration;
- production key storage or account recovery;
- anonymous payments or smart contracts;
- PoW, DHT-based consensus, or stake;
- real resource fees, paid downloads, or market pricing;
- automatic seizure or authority transfer overrides;
- a GUI.

SMD is isolated from `meld-core`; existing CDC and BitTorrent commands neither
open an SMD ledger nor require a wallet. No reward is automatically issued by
the existing upload paths in v0.1. Contributions must be explicitly recorded
and receiver-signed through the devnet interface.
