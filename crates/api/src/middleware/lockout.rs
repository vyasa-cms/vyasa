//! Progressive lockout for repeated failed logins.
//!
//! The per-IP rate limiter caps how *fast* an attacker can guess, but it
//! treats a single account under attack from many addresses the same as
//! ordinary traffic, and it resets completely every window. Lockout closes
//! that gap by counting failures per `(account, IP)` and making each
//! subsequent attempt wait longer.
//!
//! The delay is returned as a rejection rather than by sleeping: holding the
//! connection open is exactly the resource an attacker wants to consume.
//!
//! State is in-process, matching the rate limiter. A multi-node deployment
//! needs a shared store; that is noted rather than pretended.
//!
//! An account is held by the fixed-size digest of its address
//! ([`crate::middleware::rate_limit::address_digest`]): every spelling of
//! one address is one account (each was a fresh counter when the address
//! was held as typed), and nothing a client types is kept. Every attempt
//! counts, whatever its shape: malformed input is an unknown address, and
//! an account whose address registration's rule would refuse (an
//! administrator's only needs an `@`) is protected like any other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::middleware::rate_limit::address_digest;

/// Failures tolerated before any delay applies, so a person who mistypes
/// their password twice is not punished.
const FREE_ATTEMPTS: u32 = 3;

/// Longest a caller is ever locked out, so a compromised-looking account
/// still becomes usable again without an admin.
const MAX_DELAY: Duration = Duration::from_secs(300);

/// A failure record older than this is forgotten entirely.
const FORGET_AFTER: Duration = Duration::from_secs(900);

/// One `(account, ip)` pair's failure history.
#[derive(Clone, Copy)]
struct Record {
    failures: u32,
    last: Instant,
}

/// An account (its address's digest) and a client key.
type Key = ([u8; 32], String);

/// Tracks failed logins and computes how long a caller must wait.
pub struct Lockout {
    records: Arc<Mutex<HashMap<Key, Record>>>,
}

impl Default for Lockout {
    fn default() -> Self {
        Self::new()
    }
}

impl Lockout {
    /// Creates an empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Delay for the `n`th failure: doubling from one second, capped.
    fn delay_for(failures: u32) -> Duration {
        if failures <= FREE_ATTEMPTS {
            return Duration::ZERO;
        }
        let steps = failures - FREE_ATTEMPTS - 1;
        // Saturating so a very long run of failures cannot overflow into a
        // short delay.
        let secs = 1u64.checked_shl(steps.min(20)).unwrap_or(u64::MAX);
        Duration::from_secs(secs).min(MAX_DELAY)
    }

    fn key(account: &str, ip: &str) -> Key {
        (address_digest(account), ip.to_owned())
    }

    /// Returns how long the caller must wait before this attempt is allowed,
    /// or `None` when it may proceed now.
    #[must_use]
    pub fn check(&self, account: &str, ip: &str) -> Option<Duration> {
        let map = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let record = map.get(&Self::key(account, ip)).copied()?;
        if now.duration_since(record.last) > FORGET_AFTER {
            return None;
        }
        let required = Self::delay_for(record.failures);
        let elapsed = now.duration_since(record.last);
        required.checked_sub(elapsed).filter(|d| !d.is_zero())
    }

    /// Records a failed attempt.
    pub fn record_failure(&self, account: &str, ip: &str) {
        let mut map = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        // Opportunistic sweep: without it a long-running process accumulates
        // one entry per attacked address forever.
        map.retain(|_, r| now.duration_since(r.last) <= FORGET_AFTER);
        let entry = map.entry(Self::key(account, ip)).or_insert(Record {
            failures: 0,
            last: now,
        });
        entry.failures = entry.failures.saturating_add(1);
        entry.last = now;
    }

    /// Clears the history for a pair after a successful login.
    pub fn record_success(&self, account: &str, ip: &str) {
        let mut map = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        map.remove(&Self::key(account, ip));
    }
}

#[cfg(test)]
impl Lockout {
    /// How many `(account, ip)` pairs are held, and the bytes their keys
    /// take.
    pub(crate) fn footprint(&self) -> (usize, usize) {
        let map = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes = map
            .keys()
            .map(|(account, ip)| account.len() + ip.len())
            .sum();
        (map.len(), bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{Lockout, FREE_ATTEMPTS, MAX_DELAY};

    #[test]
    fn an_account_is_held_by_a_fixed_size_key_however_long_the_address() {
        // Whatever a client types in the email field is what the lockout
        // counts under; held as typed, a 2 MB "address" stayed 2 MB for
        // fifteen minutes.
        let lock = Lockout::new();
        let huge = format!("{}@example.test", "a".repeat(2 * 1024 * 1024));
        lock.record_failure(&huge, "10.0.0.1");
        lock.record_failure("ada@example.test", "10.0.0.1");
        let (entries, bytes) = lock.footprint();
        assert_eq!(entries, 2);
        assert!(bytes <= 2 * (32 + "10.0.0.1".len()), "{bytes} bytes");
    }

    #[test]
    fn every_spelling_of_an_address_is_one_account() {
        // The column is case-insensitive, so every one of these signs in
        // to the same account; counted apart, each was a fresh counter.
        let lock = Lockout::new();
        for spelling in [
            "ada@example.test",
            "ADA@example.test",
            " Ada@Example.Test ",
            "ada@EXAMPLE.TEST",
        ] {
            lock.record_failure(spelling, "10.0.0.1");
        }
        assert!(
            lock.check("ada@example.test", "10.0.0.1").is_some(),
            "four failures on one account start the delay"
        );
        assert_eq!(lock.footprint().0, 1);
    }

    #[test]
    fn a_few_mistypes_are_not_punished() {
        let lock = Lockout::new();
        for _ in 0..FREE_ATTEMPTS {
            lock.record_failure("ada@example.test", "10.0.0.1");
            assert!(lock.check("ada@example.test", "10.0.0.1").is_none());
        }
    }

    #[test]
    fn the_delay_grows_with_each_further_failure() {
        let lock = Lockout::new();
        for _ in 0..=FREE_ATTEMPTS {
            lock.record_failure("ada@example.test", "10.0.0.1");
        }
        let first = lock.check("ada@example.test", "10.0.0.1");
        assert!(first.is_some(), "the fourth failure must start delaying");
        lock.record_failure("ada@example.test", "10.0.0.1");
        let second = lock
            .check("ada@example.test", "10.0.0.1")
            .expect("still delayed");
        assert!(
            second > first.expect("checked above"),
            "each failure must cost more than the last"
        );
    }

    #[test]
    fn the_delay_is_capped() {
        assert_eq!(Lockout::delay_for(1_000), MAX_DELAY);
        // A count near u32::MAX must not wrap around to a short delay.
        assert_eq!(Lockout::delay_for(u32::MAX), MAX_DELAY);
    }

    #[test]
    fn a_successful_login_clears_the_history() {
        let lock = Lockout::new();
        for _ in 0..10 {
            lock.record_failure("ada@example.test", "10.0.0.1");
        }
        assert!(lock.check("ada@example.test", "10.0.0.1").is_some());
        lock.record_success("ada@example.test", "10.0.0.1");
        assert!(lock.check("ada@example.test", "10.0.0.1").is_none());
    }

    #[test]
    fn attackers_are_tracked_per_address_not_only_per_account() {
        // One address grinding an account must not lock the real owner out
        // from somewhere else.
        let lock = Lockout::new();
        for _ in 0..10 {
            lock.record_failure("ada@example.test", "10.0.0.1");
        }
        assert!(lock.check("ada@example.test", "10.0.0.1").is_some());
        assert!(lock.check("ada@example.test", "192.168.1.5").is_none());
    }

    #[test]
    fn an_unseen_pair_is_never_delayed() {
        let lock = Lockout::new();
        assert!(lock.check("nobody@example.test", "10.0.0.1").is_none());
    }
}
