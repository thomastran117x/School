//! Bounded in-process token buckets used when Redis is unavailable.
//!
//! This is a degraded mode for fail-open policies only. Each replica enforces the
//! limit independently, so the effective cluster-wide limit is looser than configured
//! but still finite. Memory is capped: a bucket that has refilled completely is
//! indistinguishable from a missing one and is swept, and when the table is still full
//! new subjects are rejected rather than evicting existing buckets (eviction would let
//! an attacker flushing the table reset its own limit).

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use super::policy::Policy;

/// Retry guidance returned when the table is full.
const FULL_TABLE_RETRY: Duration = Duration::from_secs(1);

/// One in-memory bucket.
struct Entry {
    tokens: f64,
    updated: Instant,
}

/// Thread-safe bounded bucket table.
pub struct LocalFallback {
    max_entries: usize,
    buckets: Mutex<HashMap<String, Entry>>,
}

impl LocalFallback {
    /// Creates a table holding at most `max_entries` buckets.
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Number of buckets currently held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.buckets.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Consumes one token. `Err` carries the suggested retry delay.
    pub fn check(&self, key: &str, policy: &Policy) -> Result<(), Duration> {
        self.check_at(key, policy, Instant::now())
    }

    /// [`Self::check`] with an explicit clock, for deterministic tests.
    pub fn check_at(&self, key: &str, policy: &Policy, now: Instant) -> Result<(), Duration> {
        let capacity = f64::from(policy.capacity);
        let per_second = f64::from(policy.refill_per_minute) / 60.0;
        let mut buckets = self.lock();

        if !buckets.contains_key(key) && buckets.len() >= self.max_entries {
            buckets.retain(|_, entry| refilled(entry, now, per_second) < capacity);
            if buckets.len() >= self.max_entries {
                return Err(FULL_TABLE_RETRY);
            }
        }

        let entry = buckets.entry(key.to_owned()).or_insert(Entry {
            tokens: capacity,
            updated: now,
        });
        entry.tokens = refilled(entry, now, per_second).min(capacity);
        entry.updated = now;
        if entry.tokens >= 1.0 {
            entry.tokens -= 1.0;
            Ok(())
        } else {
            let seconds = (1.0 - entry.tokens) / per_second;
            Err(Duration::from_secs_f64(seconds.max(0.001)))
        }
    }
}

/// Tokens the entry would hold at `now`, before clamping to capacity.
fn refilled(entry: &Entry, now: Instant, per_second: f64) -> f64 {
    let elapsed = now.saturating_duration_since(entry.updated).as_secs_f64();
    entry.tokens + elapsed * per_second
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::rate_limit::policy::FailureMode;

    fn policy(capacity: u32, per_minute: u32) -> Policy {
        Policy {
            name: "test",
            capacity,
            refill_per_minute: per_minute,
            failure: FailureMode::Open,
        }
    }

    #[test]
    fn allows_a_burst_then_limits_and_reports_retry_after() {
        let table = LocalFallback::new(10);
        let policy = policy(3, 60);
        let now = Instant::now();
        for _ in 0..3 {
            assert!(table.check_at("a", &policy, now).is_ok());
        }
        let retry = table.check_at("a", &policy, now).unwrap_err();
        assert!(retry > Duration::ZERO && retry <= Duration::from_secs(1));
    }

    #[test]
    fn tokens_refill_with_time_but_never_beyond_capacity() {
        let table = LocalFallback::new(10);
        let policy = policy(2, 60);
        let start = Instant::now();
        assert!(table.check_at("a", &policy, start).is_ok());
        assert!(table.check_at("a", &policy, start).is_ok());
        assert!(table.check_at("a", &policy, start).is_err());
        let later = start + Duration::from_secs(1);
        assert!(table.check_at("a", &policy, later).is_ok());
        assert!(table.check_at("a", &policy, later).is_err());
        // A long idle period restores only `capacity` tokens.
        let much_later = start + Duration::from_secs(3600);
        assert!(table.check_at("a", &policy, much_later).is_ok());
        assert!(table.check_at("a", &policy, much_later).is_ok());
        assert!(table.check_at("a", &policy, much_later).is_err());
    }

    #[test]
    fn subjects_are_independent() {
        let table = LocalFallback::new(10);
        let policy = policy(1, 1);
        let now = Instant::now();
        assert!(table.check_at("a", &policy, now).is_ok());
        assert!(table.check_at("a", &policy, now).is_err());
        assert!(table.check_at("b", &policy, now).is_ok());
    }

    #[test]
    fn memory_stays_bounded_under_a_key_flood_without_evicting_live_buckets() {
        let table = LocalFallback::new(4);
        let policy = policy(1, 1);
        let now = Instant::now();
        for i in 0..4 {
            assert!(table.check_at(&format!("k{i}"), &policy, now).is_ok());
        }
        // The table is full of drained buckets: new subjects are refused...
        for i in 4..100 {
            assert!(table.check_at(&format!("k{i}"), &policy, now).is_err());
        }
        assert_eq!(table.len(), 4);
        // ...and existing buckets keep their state, so flooding cannot reset them.
        assert!(table.check_at("k0", &policy, now).is_err());
    }

    #[test]
    fn fully_refilled_buckets_are_swept_to_make_room() {
        let table = LocalFallback::new(2);
        let policy = policy(1, 60);
        let now = Instant::now();
        assert!(table.check_at("a", &policy, now).is_ok());
        assert!(table.check_at("b", &policy, now).is_ok());
        let later = now + Duration::from_secs(5);
        assert!(table.check_at("c", &policy, later).is_ok());
        assert!(table.len() <= 2);
    }
}
