# quotum spend caps on NEAR

A NEAR function-call key allowance bounds an agent's fees and transaction count, not value. A contract on the agent's wallet enforces a daily payment cap for inference and pays only the configured provider.

## two caps

| cap | enforcement | bounds | reset |
|---|---|---|---|
| hard | NEAR function-call key: receiver = wallet, method = `pay`, allowance = 0.008 NEAR | fees and transaction count, including transactions in flight; not payment value | `bell()` deletes and re-adds the key |
| soft | wallet contract: daily cap = 0.05 NEAR, max price = 0.01 NEAR | payment value to the configured provider | `bell()` records spent and unspent, then starts the next day |

These are the demo defaults. The locked wallet has no full-access key. The owner can set policy, revoke or restore the agent, withdraw, and ring the bell early; anyone can ring after 24 hours. The agent can only call `pay`. The allowance is a lifetime counter until the bell re-arms it. The NEAR demo records unspent budget; it does not execute quotum's buyback or burn.

## quickstart

Requires Rust 1.95 or newer, Node 22 or newer with npm, Python 3, and network access for dependencies, the faucet, and NEAR testnet RPC. The script installs the Wasm target if needed. Optional builder: `cargo install cargo-near --version 0.22.0 --locked --no-default-features`.

```bash
./reproduce.sh
```

This builds and tests the contract, installs locked JavaScript dependencies, creates faucet accounts, deploys and locks a testnet wallet, and runs the scenario. Five payments succeed, the sixth exceeds the soft cap, forbidden actions are refused, retries reach the allowance limit, and a bell enables the next day's payment. Each run writes a JSON record and a Markdown transaction table under `runs/`.

Throwaway credentials stay outside the repository in a hidden testnet credentials directory under the home directory. `SPENDCAP_CREDS_DIR` overrides the directory. `NEAR_RPC_URL` overrides the testnet RPC; the script checks `chain_id` before signing. Demo settings: `PRICE_NEAR`, `DAILY_CAP_NEAR`, `ALLOWANCE_NEAR`, `PAY_GAS_TGAS`, and `DAY_LENGTH_SEC`.

## verify

Replace the run ID with the new file produced by quickstart. With cargo-near:

```bash
python3 -I verify/verify.py runs/<run-id>.json contract/target/near/quotum_spendcap.wasm
```

With the plain Cargo fallback:

```bash
python3 -I verify/verify.py runs/<run-id>.json contract/target/wasm32-unknown-unknown/release/quotum_spendcap.wasm
```

The verifier uses Python stdlib and archival testnet RPC. To check a saved run's recorded code hash without a local Wasm build, omit the last argument; that skips the local-binary comparison. A new build can differ from a recorded run because the Rust toolchain and builder are not pinned. Use a new run with its own Wasm for the local-binary comparison.

## latest testnet evidence

Recorded on 2026-11-02, `run-2026-11-02T11-28-17-721Z`, on the current contract. `verify/verify.py` against archival testnet RPC reports 27 passed and 0 failed, including comparison to the local Wasm built from this repository.

Wallet: [agent.qsc-mvz6dr8l7057.testnet](https://testnet.nearblocks.io/address/agent.qsc-mvz6dr8l7057.testnet). Recorded Wasm SHA-256: `0e8605b2f01e395b95702fba34f1709851b6729afff158f864a05ad2b232295d` (178181 bytes).

| step | testnet transaction |
|---|---|
| deploy + init | [C84RQzFKZTFtNVuLKs71yhGzTJ6mJ72Xa1WaKLMXQCQE](https://testnet.nearblocks.io/txns/C84RQzFKZTFtNVuLKs71yhGzTJ6mJ72Xa1WaKLMXQCQE) |
| lock wallet | [hoZnnwteEwJKWejQr19FxsZN18YGKaiTctCvDwzaYWR](https://testnet.nearblocks.io/txns/hoZnnwteEwJKWejQr19FxsZN18YGKaiTctCvDwzaYWR) |
| day 0 paid 1 | [9nwCUrzRuss68dZcKKqA6GAJNYj8AQLKQteziHJc1Zrf](https://testnet.nearblocks.io/txns/9nwCUrzRuss68dZcKKqA6GAJNYj8AQLKQteziHJc1Zrf) |
| day 0 paid 2 | [3a4czPo9y9oZXY7d76mr9z376TQK62HNhujWR8cwS3N7](https://testnet.nearblocks.io/txns/3a4czPo9y9oZXY7d76mr9z376TQK62HNhujWR8cwS3N7) |
| day 0 paid 3 | [CHvzrenk8rGdddw5MvR6mBqc2kwcqdb7zyWboFwMdMeV](https://testnet.nearblocks.io/txns/CHvzrenk8rGdddw5MvR6mBqc2kwcqdb7zyWboFwMdMeV) |
| day 0 paid 4 | [uLhFVhQXsdyKf7jrcRQePssdvRGY4bN89NtXVPAWHFw](https://testnet.nearblocks.io/txns/uLhFVhQXsdyKf7jrcRQePssdvRGY4bN89NtXVPAWHFw) |
| day 0 paid 5 | [6AskhWtU6iGqu8cfXfLQvRNWDVbaZpr3kiTPXGVcMBET](https://testnet.nearblocks.io/txns/6AskhWtU6iGqu8cfXfLQvRNWDVbaZpr3kiTPXGVcMBET) |
| soft-cap denial | [57cpVbNoUWBBxNkZMzksncWGyqRYir6w4kiqt1B7qy1L](https://testnet.nearblocks.io/txns/57cpVbNoUWBBxNkZMzksncWGyqRYir6w4kiqt1B7qy1L) |
| stranger early bell denied | [7Yy7mBDQaa8E4MKnTDSP5ByHJgs7q5j5iXenRX8CPE5E](https://testnet.nearblocks.io/txns/7Yy7mBDQaa8E4MKnTDSP5ByHJgs7q5j5iXenRX8CPE5E) |
| retry storm 1 | [Acbnznq1ueAwgC8LGsaR6k1ZMJbr7f99YkkXvjiXnGi6](https://testnet.nearblocks.io/txns/Acbnznq1ueAwgC8LGsaR6k1ZMJbr7f99YkkXvjiXnGi6) |
| retry storm 2 | [Cse2D7gLih768TCohc3udb7vgPs3XybRALczWPQAPxNJ](https://testnet.nearblocks.io/txns/Cse2D7gLih768TCohc3udb7vgPs3XybRALczWPQAPxNJ) |
| retry storm 3 | [2mgmWzRKqBKmJWWMhuc2W9CnSgCjnjCRZ2g3MZTTPgdL](https://testnet.nearblocks.io/txns/2mgmWzRKqBKmJWWMhuc2W9CnSgCjnjCRZ2g3MZTTPgdL) |
| retry storm 4 | [3PhzsMrsAy6xS38NL48y4wdDpbV5rGL2cJ2JqM5iFo8X](https://testnet.nearblocks.io/txns/3PhzsMrsAy6xS38NL48y4wdDpbV5rGL2cJ2JqM5iFo8X) |
| retry storm 5 | [61Hgr6MxDBezGSXGu2iXwCbui2Cgczvg83GDWNFVAtQt](https://testnet.nearblocks.io/txns/61Hgr6MxDBezGSXGu2iXwCbui2Cgczvg83GDWNFVAtQt) |
| bell / reset | [2eHm9KeuyjZ9Yvs3iE2SH76YeKdzBxwJfYGP2N9uo5Ba](https://testnet.nearblocks.io/txns/2eHm9KeuyjZ9Yvs3iE2SH76YeKdzBxwJfYGP2N9uo5Ba) |
| day 1 paid | [CPFLA2y9oJFJHBbhk7cesebMaGENH3Gkqb7eg9nR7PN1](https://testnet.nearblocks.io/txns/CPFLA2y9oJFJHBbhk7cesebMaGENH3Gkqb7eg9nR7PN1) |

The two allowance refusals after storm 5 have no transaction hashes because the protocol did not include them. The provider received 0.06 NEAR across six successful requests.

Full records: [run](runs/run-2026-11-02T11-28-17-721Z.md) and [JSON](runs/run-2026-11-02T11-28-17-721Z.json).

## honest limits

- The allowance bounds fees and transaction count, not value; value is bounded by the contract. Recommended: the owner funds the wallet with about one day's cap at a time, so the worst case for a contract bug is one day of budget.
- The soft cap is whatever the owner sets. quotum's holder vote is not on NEAR yet: `set_policy` is owner-only, and the owner can also ring the bell early.
- Days end at the bell, not in a rolling 24-hour window. Time alone does not reset spend. A payment just before the bell and another just after it count toward different days.
- Spend is charged when a transfer is scheduled. Failed transfers do not restore the cap. `spent_today` and `total_paid` count scheduled amounts, not confirmed provider receipts.
- Each bell adds a permanent day record. The owner must fund its storage stake above the 0.05 NEAR reserve. A bell without that funding fails.
- Revocation and key replacement block later-executing payments. Transfers already scheduled by successful calls cannot be recalled. Key updates are asynchronous; `agent_key_active` records contract authorization, not confirmed access-key existence.
- Part of the allowance is unusable headroom.
- The endpoint is a local mock; payments are in testnet NEAR, not a stablecoin.
- Once locked, the contract cannot be upgraded. That is by design for the demo; production needs an upgrade path such as a timelocked owner.
- Not yet used: NEAR Intents, Chain Signatures, or the new gas keys.

The mock's replay guard is in memory and resets on restart. It trusts one RPC, does not bind payment to prompt content, and the contract does not require unique request IDs. The owner retains the explicit management powers listed above. Allowance admission headroom was measured empirically; re-measure before production use.

## what was built in the hackathon window

quotum existed before this hackathon. The existing service enforced inference caps off-chain; this repository contains the NEAR integration built for the 2026-11-01 to 2026-11-14 window:

- Rust policy wallet using near-sdk 5.30.0: daily ledger, cap, max price, fixed provider, bell, kill switch, and 21 unit tests.
- An agent function-call key scoped to `pay` on its own wallet, with an allowance re-armed by the bell.
- A local inference mock that checks final NEAR payments and rejects payment-hash reuse during its process lifetime.
- Reproduction script, agent probes, a recorded testnet run, and independent Python verifier.

[Compare the NEAR integration](https://github.com/quotumorg/near-spendcap/compare/pre-hackathon...hackathon-submission).
The `pre-hackathon` tag contains only a README stub and MIT license. It is a locally prepared comparison anchor, not a historical snapshot of the existing quotum service; the baseline service code is not included. Both tags were prepared locally on 2026-11-02 without backdating.

## license

MIT, copyright quotum. See [LICENSE](LICENSE).
