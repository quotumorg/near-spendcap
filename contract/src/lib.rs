//! quotum spend-cap agent wallet for NEAR.
//!
//! The contract is deployed ON the agent's own account, so the account is a
//! policy wallet. Two independent caps:
//!
//! * Hard cap (protocol): the agent holds only a function-call access key on
//!   this account, restricted to receiver = this account and method = `pay`,
//!   with an `allowance` in NEAR. The NEAR runtime refuses any transaction
//!   signed by that key once the allowance cannot cover its gas, and refuses
//!   any other method, any other receiver and any attached deposit. The key is
//!   re-armed (deleted and re-added with a fresh allowance) at every `bell`.
//! * Soft cap (policy): `pay` records spend per day and panics with
//!   `SPENDCAP_DENIED` if the payment would exceed the seat's daily cap or
//!   the max price per request. Agent payments go only to the configured
//!   inference provider; the owner can also withdraw idle funds.
//!
//! A day ends at the `bell`: the owner (seat holder / quotum operator) may ring
//! it at any time; anyone may ring it once `day_length_sec` has elapsed. The
//! agent's key cannot ring it. The bell closes the day record, emits the
//! unspent amount (in quotum the unspent cap is bought back and burned at the
//! daily bell; here it is only emitted as an event), resets the soft cap and
//! re-arms the hard cap.

use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{
    env, near, require, AccountId, Allowance, NearToken, PanicOnDefault, Promise, PublicKey,
};

const EVENT_STANDARD: &str = "quotum-spendcap";
const EVENT_VERSION: &str = "1.0.0";
/// Balance that must remain on the wallet after a payment, on top of storage.
const RESERVE: NearToken = NearToken::from_millinear(50);
const MAX_REQUEST_ID_LEN: usize = 64;

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq)]
pub struct DayRecord {
    pub day: u64,
    pub started_ns: u64,
    pub closed_ns: u64,
    pub daily_cap: U128,
    pub spent: U128,
    pub unspent: U128,
    pub requests: u32,
}

#[near(serializers = [json])]
pub struct Status {
    pub owner_id: AccountId,
    pub provider_id: AccountId,
    pub agent_key: PublicKey,
    pub agent_allowance: U128,
    pub daily_cap: U128,
    pub max_price: U128,
    pub day_length_sec: u64,
    pub day: u64,
    pub day_started_ns: u64,
    pub spent_today: U128,
    pub remaining_today: U128,
    pub requests_today: u32,
    pub total_paid: U128,
    pub now_ns: u64,
    pub bell_open_to_anyone: bool,
    pub agent_key_active: bool,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct SpendCap {
    owner_id: AccountId,
    provider_id: AccountId,
    agent_key: PublicKey,
    agent_allowance: NearToken,
    daily_cap: NearToken,
    max_price: NearToken,
    day_length_ns: u64,
    day: u64,
    day_started_ns: u64,
    spent_today: NearToken,
    requests_today: u32,
    total_paid: NearToken,
    history: LookupMap<u64, DayRecord>,
    /// false after `revoke_agent`: the bell then does NOT re-arm the key (kill switch sticks).
    agent_key_active: bool,
}

fn emit(event: &str, data: String) {
    env::log_str(&format!(
        "EVENT_JSON:{{\"standard\":\"{EVENT_STANDARD}\",\"version\":\"{EVENT_VERSION}\",\"event\":\"{event}\",\"data\":[{data}]}}"
    ));
}

fn deny(reason: &str) -> ! {
    env::panic_str(&format!("SPENDCAP_DENIED: {reason}"))
}

#[near]
impl SpendCap {
    /// Must be called by the wallet account itself (deploy + init in one tx).
    /// Adds the agent's function-call key with the hard-cap allowance.
    #[init]
    pub fn new(
        owner_id: AccountId,
        provider_id: AccountId,
        agent_key: PublicKey,
        agent_allowance: U128,
        daily_cap: U128,
        max_price: U128,
        day_length_sec: u64,
    ) -> Self {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "init must be called by the wallet account itself"
        );
        require!(day_length_sec > 0, "day_length_sec must be > 0");
        require!(max_price.0 > 0 && max_price.0 <= daily_cap.0, "need 0 < max_price <= daily_cap");
        let mut this = Self {
            owner_id,
            provider_id,
            agent_key,
            agent_allowance: NearToken::from_yoctonear(agent_allowance.0),
            daily_cap: NearToken::from_yoctonear(daily_cap.0),
            max_price: NearToken::from_yoctonear(max_price.0),
            day_length_ns: day_length_sec.saturating_mul(1_000_000_000),
            day: 0,
            day_started_ns: env::block_timestamp(),
            spent_today: NearToken::from_yoctonear(0),
            requests_today: 0,
            total_paid: NearToken::from_yoctonear(0),
            history: LookupMap::new(b"h"),
            agent_key_active: false,
        };
        this.arm_agent_key().detach();
        emit(
            "init",
            format!(
                "{{\"owner_id\":\"{}\",\"provider_id\":\"{}\",\"agent_key\":\"{}\",\"agent_allowance\":\"{}\",\"daily_cap\":\"{}\",\"max_price\":\"{}\",\"day_length_sec\":{}}}",
                this.owner_id,
                this.provider_id,
                String::from(&this.agent_key),
                this.agent_allowance.as_yoctonear(),
                this.daily_cap.as_yoctonear(),
                this.max_price.as_yoctonear(),
                day_length_sec
            ),
        );
        this
    }

    /// Pay the inference provider for one request. Called by the agent's
    /// function-call key (predecessor = this account). Panics with
    /// `SPENDCAP_DENIED` over the soft cap; state is then unchanged.
    pub fn pay(&mut self, request_id: String, amount: U128) -> Promise {
        require!(
            env::predecessor_account_id() == env::current_account_id(),
            "pay must be signed by a key of this wallet"
        );
        // Key actions are asynchronous. Reject stale receipts even before
        // DeleteKey executes, or if a key-management promise fails.
        require!(self.agent_key_active, "agent key revoked");
        require!(env::signer_account_pk() == self.agent_key, "pay must use the current agent key");
        if request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_LEN {
            deny("request_id must be 1..=64 bytes");
        }
        let amount = NearToken::from_yoctonear(amount.0);
        if amount.is_zero() {
            deny("amount must be > 0");
        }
        if amount > self.max_price {
            deny(&format!(
                "price {} > max_price {} (day {}, request {})",
                amount.as_yoctonear(),
                self.max_price.as_yoctonear(),
                self.day,
                request_id
            ));
        }
        let new_spent = self.spent_today.checked_add(amount)
            .unwrap_or_else(|| deny("spend overflow"));
        if new_spent > self.daily_cap {
            deny(&format!(
                "over daily cap: spent {} + {} > cap {} (day {}, request {})",
                self.spent_today.as_yoctonear(),
                amount.as_yoctonear(),
                self.daily_cap.as_yoctonear(),
                self.day,
                request_id
            ));
        }
        let storage = env::storage_byte_cost().saturating_mul(env::storage_usage() as u128);
        let floor = storage.saturating_add(RESERVE).saturating_add(amount);
        if env::account_balance() < floor {
            deny("wallet balance would fall below storage + reserve");
        }
        self.spent_today = new_spent;
        self.requests_today += 1;
        self.total_paid = self.total_paid.saturating_add(amount);
        emit(
            "paid",
            format!(
                "{{\"request_id\":{},\"amount\":\"{}\",\"provider_id\":\"{}\",\"day\":{},\"spent_today\":\"{}\",\"daily_cap\":\"{}\",\"requests_today\":{}}}",
                near_sdk::serde_json::to_string(&request_id).unwrap(),
                amount.as_yoctonear(),
                self.provider_id,
                self.day,
                self.spent_today.as_yoctonear(),
                self.daily_cap.as_yoctonear(),
                self.requests_today
            ),
        );
        Promise::new(self.provider_id.clone()).transfer(amount)
    }

    /// Close the current day. Owner: any time. Anyone else: only after
    /// `day_length_sec`. Resets the soft cap and re-arms the hard cap
    /// (unless the agent was revoked: the kill switch survives the bell).
    pub fn bell(&mut self) {
        let now = env::block_timestamp();
        let caller = env::predecessor_account_id();
        let elapsed = now.saturating_sub(self.day_started_ns) >= self.day_length_ns;
        require!(
            caller == self.owner_id || elapsed,
            "bell: only the owner may ring before the day has elapsed"
        );
        require!(
            caller != env::current_account_id(),
            "bell: the wallet's own keys cannot ring the bell"
        );
        let unspent = self.daily_cap.saturating_sub(self.spent_today);
        let rec = DayRecord {
            day: self.day,
            started_ns: self.day_started_ns,
            closed_ns: now,
            daily_cap: U128(self.daily_cap.as_yoctonear()),
            spent: U128(self.spent_today.as_yoctonear()),
            unspent: U128(unspent.as_yoctonear()),
            requests: self.requests_today,
        };
        emit(
            "bell",
            format!(
                "{{\"day\":{},\"rung_by\":\"{}\",\"early\":{},\"daily_cap\":\"{}\",\"spent\":\"{}\",\"unspent\":\"{}\",\"requests\":{},\"next_day\":{},\"agent_allowance\":\"{}\"}}",
                rec.day,
                caller,
                !elapsed,
                rec.daily_cap.0,
                rec.spent.0,
                rec.unspent.0,
                rec.requests,
                self.day + 1,
                self.agent_allowance.as_yoctonear()
            ),
        );
        self.history.insert(self.day, rec);
        // LookupMap writes are deferred. Flush before measuring the new
        // storage stake; a failed check rolls the receipt back on-chain.
        self.history.flush();
        let storage = env::storage_byte_cost().saturating_mul(env::storage_usage() as u128);
        require!(
            env::account_balance() >= storage.saturating_add(RESERVE),
            "bell would leave less than storage + reserve"
        );
        self.day += 1;
        self.day_started_ns = now;
        self.spent_today = NearToken::from_yoctonear(0);
        self.requests_today = 0;
        if self.agent_key_active {
            self.arm_agent_key().detach();
        }
    }

    // ---- owner (seat holder / quotum operator) ----

    /// Change the soft cap (immediate), max price (immediate) and/or the hard
    /// cap allowance (applied at the next bell or `set_agent_key`).
    pub fn set_policy(
        &mut self,
        daily_cap: Option<U128>,
        max_price: Option<U128>,
        agent_allowance: Option<U128>,
    ) {
        self.assert_owner();
        if let Some(c) = daily_cap {
            self.daily_cap = NearToken::from_yoctonear(c.0);
        }
        if let Some(p) = max_price {
            self.max_price = NearToken::from_yoctonear(p.0);
        }
        if let Some(a) = agent_allowance {
            self.agent_allowance = NearToken::from_yoctonear(a.0);
        }
        require!(
            !self.max_price.is_zero() && self.max_price <= self.daily_cap,
            "need 0 < max_price <= daily_cap"
        );
        emit(
            "policy",
            format!(
                "{{\"daily_cap\":\"{}\",\"max_price\":\"{}\",\"agent_allowance\":\"{}\"}}",
                self.daily_cap.as_yoctonear(),
                self.max_price.as_yoctonear(),
                self.agent_allowance.as_yoctonear()
            ),
        );
    }

    /// Kill switch: delete the agent key. Re-add with `set_agent_key`.
    pub fn revoke_agent(&mut self) -> Promise {
        self.assert_owner();
        require!(self.agent_key_active, "agent key already revoked");
        self.agent_key_active = false;
        emit("revoke", format!("{{\"agent_key\":\"{}\"}}", String::from(&self.agent_key)));
        Promise::new(env::current_account_id()).delete_key(self.agent_key.clone())
    }

    /// Replace (or, after a revoke, restore) the agent key. The old key is
    /// deleted if still active; the new one gets the current allowance.
    pub fn set_agent_key(&mut self, agent_key: PublicKey) -> Promise {
        self.assert_owner();
        let me = env::current_account_id();
        let mut p = Promise::new(me.clone());
        if self.agent_key_active {
            p = p.delete_key(self.agent_key.clone());
        }
        self.agent_key = agent_key;
        self.agent_key_active = true;
        p.add_access_key_allowance(self.agent_key.clone(), self.allowance(), me, "pay")
    }

    /// Withdraw idle wallet balance to the owner (storage + reserve stay).
    pub fn withdraw(&mut self, amount: U128) -> Promise {
        self.assert_owner();
        let amount = NearToken::from_yoctonear(amount.0);
        let storage = env::storage_byte_cost().saturating_mul(env::storage_usage() as u128);
        require!(
            env::account_balance() >= storage.saturating_add(RESERVE).saturating_add(amount),
            "withdraw would leave less than storage + reserve"
        );
        Promise::new(self.owner_id.clone()).transfer(amount)
    }

    // ---- views ----

    pub fn get_status(&self) -> Status {
        let now = env::block_timestamp();
        Status {
            owner_id: self.owner_id.clone(),
            provider_id: self.provider_id.clone(),
            agent_key: self.agent_key.clone(),
            agent_allowance: U128(self.agent_allowance.as_yoctonear()),
            daily_cap: U128(self.daily_cap.as_yoctonear()),
            max_price: U128(self.max_price.as_yoctonear()),
            day_length_sec: self.day_length_ns / 1_000_000_000,
            day: self.day,
            day_started_ns: self.day_started_ns,
            spent_today: U128(self.spent_today.as_yoctonear()),
            remaining_today: U128(self.daily_cap.saturating_sub(self.spent_today).as_yoctonear()),
            requests_today: self.requests_today,
            total_paid: U128(self.total_paid.as_yoctonear()),
            now_ns: now,
            bell_open_to_anyone: now.saturating_sub(self.day_started_ns) >= self.day_length_ns,
            agent_key_active: self.agent_key_active,
        }
    }

    pub fn get_day(&self, day: u64) -> Option<DayRecord> {
        self.history.get(&day).cloned()
    }

    // ---- internal ----

    fn assert_owner(&self) {
        require!(env::predecessor_account_id() == self.owner_id, "owner only");
    }

    fn allowance(&self) -> Allowance {
        Allowance::limited(self.agent_allowance).unwrap_or_else(|| env::panic_str("allowance must be > 0"))
    }

    /// (Re-)add the agent key: receiver = this wallet, method = `pay`,
    /// allowance = hard cap. Deletes the existing key first when it is active
    /// (re-arming resets the allowance; NEAR has no "top up allowance" action).
    fn arm_agent_key(&mut self) -> Promise {
        let me = env::current_account_id();
        let mut p = Promise::new(me.clone());
        if self.agent_key_active {
            p = p.delete_key(self.agent_key.clone());
        }
        self.agent_key_active = true;
        p.add_access_key_allowance(self.agent_key.clone(), self.allowance(), me, "pay")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::mock::MockAction;
    use near_sdk::test_utils::{get_created_receipts, get_logs, VMContextBuilder};
    use near_sdk::testing_env;

    fn wallet() -> AccountId {
        "agent.seat.testnet".parse().unwrap()
    }
    fn owner() -> AccountId {
        "seat.testnet".parse().unwrap()
    }
    fn provider() -> AccountId {
        "provider.seat.testnet".parse().unwrap()
    }
    fn key() -> PublicKey {
        "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp".parse().unwrap()
    }
    const MILLI: u128 = 1_000_000_000_000_000_000_000;

    fn ctx(pred: AccountId, ts: u64) {
        let mut b = VMContextBuilder::new();
        b.current_account_id(wallet())
            .signer_account_id(pred.clone())
            .signer_account_pk(key())
            .predecessor_account_id(pred)
            .block_timestamp(ts)
            .account_balance(NearToken::from_near(5));
        testing_env!(b.build());
    }

    fn setup() -> SpendCap {
        ctx(wallet(), 1_000);
        SpendCap::new(owner(), provider(), key(), U128(5 * MILLI), U128(50 * MILLI), U128(10 * MILLI), 86_400)
    }

    #[test]
    fn pays_until_cap_then_denies() {
        let mut c = setup();
        for i in 0..5 {
            let _ = c.pay(format!("r{i}"), U128(10 * MILLI));
        }
        assert_eq!(c.get_status().spent_today.0, 50 * MILLI);
        assert_eq!(c.get_status().requests_today, 5);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = c.pay("r5".into(), U128(10 * MILLI));
        }));
        assert!(r.is_err());
    }

    #[test]
    #[should_panic(expected = "SPENDCAP_DENIED: price")]
    fn denies_over_max_price() {
        let mut c = setup();
        let _ = c.pay("big".into(), U128(11 * MILLI));
    }

    #[test]
    #[should_panic(expected = "pay must be signed by a key of this wallet")]
    fn pay_only_from_wallet() {
        let mut c = setup();
        ctx(owner(), 2_000);
        let _ = c.pay("x".into(), U128(MILLI));
    }

    #[test]
    fn owner_bell_resets_day() {
        let mut c = setup();
        let _ = c.pay("a".into(), U128(10 * MILLI));
        ctx(owner(), 5_000);
        c.bell();
        let s = c.get_status();
        assert_eq!(s.day, 1);
        assert_eq!(s.spent_today.0, 0);
        let d0 = c.get_day(0).unwrap();
        assert_eq!(d0.spent.0, 10 * MILLI);
        assert_eq!(d0.unspent.0, 40 * MILLI);
    }

    #[test]
    #[should_panic(expected = "only the owner may ring")]
    fn stranger_cannot_ring_early() {
        let mut c = setup();
        ctx("stranger.testnet".parse().unwrap(), 5_000);
        c.bell();
    }

    #[test]
    fn stranger_can_ring_after_day() {
        let mut c = setup();
        ctx("stranger.testnet".parse().unwrap(), 1_000 + 86_400 * 1_000_000_000);
        c.bell();
        assert_eq!(c.get_status().day, 1);
    }

    #[test]
    #[should_panic(expected = "wallet's own keys cannot ring")]
    fn wallet_cannot_ring_own_bell() {
        let mut c = setup();
        ctx(wallet(), 1_000 + 86_400 * 1_000_000_000);
        c.bell();
    }

    #[test]
    fn revoke_survives_bell_and_can_be_restored() {
        let mut c = setup();
        assert!(c.get_status().agent_key_active);
        ctx(owner(), 5_000);
        let _ = c.revoke_agent();
        assert!(!c.get_status().agent_key_active);
        c.bell(); // must not panic and must not re-arm the revoked key
        assert!(!c.get_status().agent_key_active);
        assert_eq!(c.get_status().day, 1);
        let _ = c.set_agent_key(key());
        assert!(c.get_status().agent_key_active);
    }

    #[test]
    #[should_panic(expected = "owner only")]
    fn policy_owner_only() {
        let mut c = setup();
        ctx(wallet(), 2_000);
        c.set_policy(Some(U128(1_000 * MILLI)), None, None);
    }

    fn assert_pay_denied(c: &mut SpendCap, request_id: &str, amount: u128) {
        let before = (c.spent_today, c.requests_today, c.total_paid);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            c.pay(request_id.into(), U128(amount)).detach();
        }));
        assert!(result.is_err(), "payment must be denied");
        assert_eq!((c.spent_today, c.requests_today, c.total_paid), before);
        assert!(get_created_receipts().is_empty(), "denial must not schedule actions");
    }

    #[test]
    fn revoked_agent_cannot_pay_while_delete_key_is_pending() {
        let mut c = setup();
        ctx(owner(), 2_000);
        c.revoke_agent().detach();
        // Model a previously admitted pay receipt executing before DeleteKey.
        ctx(wallet(), 3_000);
        assert_pay_denied(&mut c, "queued-before-revoke", MILLI);
    }

    #[test]
    fn replaced_agent_cannot_pay_with_a_previously_admitted_receipt() {
        let mut c = setup();
        let replacement: PublicKey = vec![0u8; 33].try_into().unwrap();
        ctx(owner(), 2_000);
        c.set_agent_key(replacement.clone()).detach();
        ctx(wallet(), 3_000); // still signed by the old key
        assert_pay_denied(&mut c, "queued-before-rotation", MILLI);

        let mut b = VMContextBuilder::new();
        b.current_account_id(wallet())
            .predecessor_account_id(wallet())
            .signer_account_id(wallet())
            .signer_account_pk(replacement)
            .block_timestamp(4_000)
            .account_balance(NearToken::from_near(5));
        testing_env!(b.build());
        c.pay("new-key".into(), U128(MILLI)).detach();
        assert_eq!(c.spent_today.as_yoctonear(), MILLI);
    }

    #[test]
    fn spend_addition_cannot_saturate_past_u128_cap() {
        let mut c = setup();
        // Exercise the accepted U128 policy domain, without pretending this
        // accumulated spend is reachable with the demo's funding and lifetime.
        c.daily_cap = NearToken::from_yoctonear(u128::MAX);
        c.spent_today = NearToken::from_yoctonear(u128::MAX - 1);
        ctx(wallet(), 2_000);
        assert_pay_denied(&mut c, "overflow", 2);
    }

    #[test]
    #[should_panic(expected = "bell would leave less than storage + reserve")]
    fn public_bell_cannot_stake_the_wallet_reserve() {
        let mut c = setup();
        let usage = 1_000;
        let mut b = VMContextBuilder::new();
        b.current_account_id(wallet())
            .predecessor_account_id("stranger.testnet".parse().unwrap())
            .block_timestamp(1_000 + 86_400 * 1_000_000_000)
            .storage_usage(usage)
            .account_balance(env::storage_byte_cost().saturating_mul(usage as u128).saturating_add(RESERVE));
        testing_env!(b.build());
        c.bell();
    }

    #[test]
    fn paid_event_escapes_untrusted_request_id() {
        let mut c = setup();
        ctx(wallet(), 2_000);
        let request_id = "quote\"\\\n\t\u{0}";
        c.pay(request_id.into(), U128(MILLI)).detach();
        let logs = get_logs();
        let event: near_sdk::serde_json::Value = near_sdk::serde_json::from_str(
            logs.last().unwrap().strip_prefix("EVENT_JSON:").unwrap(),
        ).expect("accepted request IDs must produce valid event JSON");
        assert_eq!(event["data"][0]["request_id"], request_id);
        assert_eq!(event["data"][0]["amount"], MILLI.to_string());
    }

    #[test]
    fn duplicate_request_ids_still_consume_the_cap_and_pay_only_provider() {
        let mut c = setup();
        for i in 0..5 {
            ctx(wallet(), 2_000 + i);
            c.pay("same-id".into(), U128(10 * MILLI)).detach();
            let receipts = get_created_receipts();
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0].receiver_id, provider());
            assert!(matches!(receipts[0].actions.as_slice(),
                [MockAction::Transfer { deposit, .. }] if deposit.as_yoctonear() == 10 * MILLI));
        }
        ctx(wallet(), 3_000);
        assert_pay_denied(&mut c, "same-id", 1);
    }

    #[test]
    fn elapsed_time_alone_does_not_reset_spend() {
        let mut c = setup();
        for _ in 0..5 {
            c.pay("full".into(), U128(10 * MILLI)).detach();
        }
        ctx(wallet(), u64::MAX);
        assert_pay_denied(&mut c, "after-many-days", 1);
        assert_eq!(c.day, 0);
    }

    #[test]
    fn bell_boundary_is_exact_and_cannot_be_repeated_or_backdated() {
        let mut c = setup();
        let deadline = 1_000 + 86_400 * 1_000_000_000;
        for ts in [999, deadline - 1] {
            ctx("stranger.testnet".parse().unwrap(), ts);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.bell()));
            assert!(r.is_err());
            assert_eq!(c.day, 0);
        }
        ctx("stranger.testnet".parse().unwrap(), deadline);
        c.bell();
        for ts in [deadline, deadline - 1] {
            ctx("stranger.testnet".parse().unwrap(), ts);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.bell()));
            assert!(r.is_err());
            assert_eq!(c.day, 1);
        }
    }

    #[test]
    fn public_bell_after_revoke_schedules_no_key_actions() {
        let mut c = setup();
        ctx(owner(), 2_000);
        c.revoke_agent().detach();
        ctx("stranger.testnet".parse().unwrap(), 1_000 + 86_400 * 1_000_000_000);
        c.bell();
        assert!(!c.agent_key_active);
        assert!(get_created_receipts().is_empty());
    }

    #[test]
    fn stranger_provider_and_wallet_cannot_manage_policy_keys_or_withdraw() {
        let mut c = setup();
        for caller in ["stranger.testnet".parse().unwrap(), provider(), wallet()] {
            for method in 0..4 {
                ctx(caller.clone(), 2_000);
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match method {
                        0 => c.set_policy(Some(U128(100 * MILLI)), None, None),
                        1 => c.revoke_agent().detach(),
                        2 => c.set_agent_key(key()).detach(),
                        _ => c.withdraw(U128(MILLI)).detach(),
                    }
                }));
                assert!(r.is_err());
                assert!(get_created_receipts().is_empty());
                assert!(c.agent_key_active);
                assert_eq!(c.daily_cap.as_yoctonear(), 50 * MILLI);
            }
        }
    }

    #[test]
    fn provider_cannot_call_pay_or_reinitialize_wallet() {
        let mut c = setup();
        ctx(provider(), 2_000);
        assert_pay_denied(&mut c, "provider", MILLI);
        let r = std::panic::catch_unwind(|| {
            SpendCap::new(owner(), provider(), key(), U128(MILLI), U128(MILLI), U128(MILLI), 1)
        });
        assert!(r.is_err());
        assert!(get_created_receipts().is_empty());
    }

    #[test]
    fn failed_transfer_result_does_not_restore_spending_capacity() {
        let mut c = setup();
        for _ in 0..5 {
            c.pay("attempt".into(), U128(10 * MILLI)).detach();
        }
        ctx(wallet(), 2_000);
        let mut b = VMContextBuilder::new();
        b.current_account_id(wallet())
            .predecessor_account_id(wallet())
            .signer_account_id(wallet())
            .signer_account_pk(key())
            .account_balance(NearToken::from_near(5));
        testing_env!(b.build(), near_sdk::test_vm_config(), near_sdk::RuntimeFeesConfig::test(),
            std::collections::HashMap::new(), vec![near_sdk::PromiseResult::Failed]);
        assert_pay_denied(&mut c, "retry-after-refund", MILLI);
        assert_eq!(c.total_paid.as_yoctonear(), 50 * MILLI);
    }
}
