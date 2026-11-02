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

contract changed after the 2026-11-02 testnet run; rerun ./reproduce.sh for fresh hashes

Changes after that run:

- `pay` rejects revoked keys and receipts signed by a replaced agent key.
- Spend addition rejects overflow instead of saturating at the cap.
- The bell checks storage plus reserve after writing its day record.
- Paid events JSON-escape the request ID.

Recorded on 2026-11-02, `run-2026-11-02T02-06-16-847Z`. The supplied verification record reports 28 passed and 0 failed, including comparison to that run's local Wasm. This preparation did not repeat network verification. Explorer links could not be checked offline; earlier reports describe an indexer delay.

Wallet: [agent.qsc-mvymazzqe8df.testnet](https://testnet.nearblocks.io/address/agent.qsc-mvymazzqe8df.testnet). Recorded Wasm SHA-256: `39db08b11276e2e858c99d31bda972ce859b2be8a6ea91d5b8b738a9dba07648` (176127 bytes).

| step | testnet transaction |
|---|---|
| deploy + init | [FCuQUBvvAgttmciA8Wh3K7eCMTmU4bEkPtxfJEG3mNMd](https://testnet.nearblocks.io/txns/FCuQUBvvAgttmciA8Wh3K7eCMTmU4bEkPtxfJEG3mNMd) |
| lock wallet | [54mDyamh3hDaSAKUWWUqnbJomBZh3DvUtDZp7MDFVbAq](https://testnet.nearblocks.io/txns/54mDyamh3hDaSAKUWWUqnbJomBZh3DvUtDZp7MDFVbAq) |
| day 0 paid 1 | [9NM46N3VmSHWXAU2b8tYWzNadgWg5yHia7c3eBH5jEHf](https://testnet.nearblocks.io/txns/9NM46N3VmSHWXAU2b8tYWzNadgWg5yHia7c3eBH5jEHf) |
| day 0 paid 2 | [Cj1Fe62Ai3ePvRQ7EwMUXmPto4PQZAyJBktTD6sqoQmJ](https://testnet.nearblocks.io/txns/Cj1Fe62Ai3ePvRQ7EwMUXmPto4PQZAyJBktTD6sqoQmJ) |
| day 0 paid 3 | [AqkNXyqiygRD7AMFJvrckBhti75G6THMg1crY3yRCcmV](https://testnet.nearblocks.io/txns/AqkNXyqiygRD7AMFJvrckBhti75G6THMg1crY3yRCcmV) |
| day 0 paid 4 | [E9a5zzJH4KWQiWVewfSNgg7ShHVCXWhMGQ9HRxQk75bo](https://testnet.nearblocks.io/txns/E9a5zzJH4KWQiWVewfSNgg7ShHVCXWhMGQ9HRxQk75bo) |
| day 0 paid 5 | [4E5nKzHJVpoZCuvFb6H1iZpVQ3gdNUszaW33wtiUDP1a](https://testnet.nearblocks.io/txns/4E5nKzHJVpoZCuvFb6H1iZpVQ3gdNUszaW33wtiUDP1a) |
| soft-cap denial | [Fq4zdzdfbjcPpxYBrbtmpuKX4iintut9HhUus3vG31XW](https://testnet.nearblocks.io/txns/Fq4zdzdfbjcPpxYBrbtmpuKX4iintut9HhUus3vG31XW) |
| stranger early bell denied | [5TmMwKYmGW42pc5e5Z1kJyAmtPMvaoxwGc5UZD4VWW6T](https://testnet.nearblocks.io/txns/5TmMwKYmGW42pc5e5Z1kJyAmtPMvaoxwGc5UZD4VWW6T) |
| retry storm 1 | [7aWAo6wASQAwm8Rhb9iZ5J2Gj4ucyQQLA65VrCGLqa5h](https://testnet.nearblocks.io/txns/7aWAo6wASQAwm8Rhb9iZ5J2Gj4ucyQQLA65VrCGLqa5h) |
| retry storm 2 | [DEt8ZRQ6qMfrUSt9yYRxPmLj8wueps2CgwdEVWiCHm4a](https://testnet.nearblocks.io/txns/DEt8ZRQ6qMfrUSt9yYRxPmLj8wueps2CgwdEVWiCHm4a) |
| retry storm 3 | [B56oA5o95DU8CPXWQWptszE7sN86QkMn312G5WukvzQ](https://testnet.nearblocks.io/txns/B56oA5o95DU8CPXWQWptszE7sN86QkMn312G5WukvzQ) |
| retry storm 4 | [86gD39WDKZcwDzaexY4uzpGjs1SrDKpY44MeCQvYwUY6](https://testnet.nearblocks.io/txns/86gD39WDKZcwDzaexY4uzpGjs1SrDKpY44MeCQvYwUY6) |
| retry storm 5 | [G49hTogrCU2dMLHTpPE99PiDx8L68ru36ZGP1NwedFSk](https://testnet.nearblocks.io/txns/G49hTogrCU2dMLHTpPE99PiDx8L68ru36ZGP1NwedFSk) |
| retry storm 6 | [2Rm4sqYG2WTJeVBYiDFRRAPqBuDVGbvGTq6ZmEBYoant](https://testnet.nearblocks.io/txns/2Rm4sqYG2WTJeVBYiDFRRAPqBuDVGbvGTq6ZmEBYoant) |
| bell / reset | [3TTiR9JPBynWERetKde2o3UQ8ecFutJHGVoUhS5CNhiu](https://testnet.nearblocks.io/txns/3TTiR9JPBynWERetKde2o3UQ8ecFutJHGVoUhS5CNhiu) |
| day 1 paid | [CgKKWHBSe4VuUkA1LVeEdPZJzhxMLAhhWkKj9rY6G7ve](https://testnet.nearblocks.io/txns/CgKKWHBSe4VuUkA1LVeEdPZJzhxMLAhhWkKj9rY6G7ve) |

The subsequent allowance refusals have no transaction hashes because the protocol did not include them. The provider received 0.06 NEAR across six successful requests.

Full records: [latest run](runs/run-2026-11-02T02-06-16-847Z.md), [latest JSON](runs/run-2026-11-02T02-06-16-847Z.json), and [demo reference run](runs/run-2026-11-02T01-20-08-017Z.md).

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
- Reproduction script, agent probes, two recorded testnet runs, and independent Python verifier.

[Compare the NEAR integration](https://github.com/quotumorg/near-spendcap/compare/pre-hackathon...hackathon-submission).
The `pre-hackathon` tag contains only a README stub and MIT license. It is a locally prepared comparison anchor, not a historical snapshot of the existing quotum service; the baseline service code is not included. Both tags were prepared locally on 2026-11-02 without backdating.

## license

MIT, copyright quotum. See [LICENSE](LICENSE).
