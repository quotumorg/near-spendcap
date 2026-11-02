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
//!   the max price per request. Only `pay` moves value, and only to the
//!   configured inference provider.
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
        let new_spent = self.spent_today.saturating_add(amount);
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
                "{{\"request_id\":\"{}\",\"amount\":\"{}\",\"provider_id\":\"{}\",\"day\":{},\"spent_today\":\"{}\",\"daily_cap\":\"{}\",\"requests_today\":{}}}",
                request_id,
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
    use near_sdk::test_utils::VMContextBuilder;
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
}
