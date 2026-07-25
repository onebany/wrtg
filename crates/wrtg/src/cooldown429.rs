//! Generic per-key HTTP 429 cooldown with exponential backoff.
//!
//! Backs both the CF-proxy domain cooldown ([`crate::cf_proxy_cooldown`]) and
//! the CF-Worker cooldown ([`crate::cf_worker_cooldown`]), which previously
//! only existed for the former — so a rate-limited Worker was re-dialled by
//! every single connection, burning quota while it was already over budget.
//!
//! Constructible in a `static` (`const fn new`), so callers don't need
//! `LazyLock`. Lock poisoning is recovered from rather than propagated: one
//! poisoned critical section must not cascade through a resilience-focused
//! daemon.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::ws::{retry_after_from_err, WsConnectError};

#[derive(Clone, Default)]
struct CooldownState {
    until: Option<Instant>,
    strikes: u32,
}

fn new_map() -> Mutex<HashMap<String, CooldownState>> {
    Mutex::new(HashMap::new())
}

pub struct Cooldown429 {
    // `HashMap::new` isn't const, so the map is built lazily; a fn-pointer
    // initializer keeps `new()` const so callers can hold one in a static.
    inner: LazyLock<Mutex<HashMap<String, CooldownState>>>,
    base: fn() -> Duration,
    max: fn() -> Duration,
    /// Label used in log lines ("CF proxy" / "CF worker").
    name: &'static str,
}

impl Cooldown429 {
    pub const fn new(base: fn() -> Duration, max: fn() -> Duration, name: &'static str) -> Self {
        Self {
            inner: LazyLock::new(new_map),
            base,
            max,
            name,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, CooldownState>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Backoff for the next strike: an explicit `Retry-After` wins, otherwise
    /// the base delay doubles per consecutive strike, capped at `max`.
    fn next_delay(&self, prev: &CooldownState, retry_after: Duration) -> Duration {
        let max = (self.max)();
        if retry_after > Duration::ZERO {
            return retry_after.min(max);
        }
        let expired = prev.until.is_none_or(|u| u.elapsed() > max);
        let strikes = if expired { 0 } else { prev.strikes };
        let mut delay = (self.base)();
        for _ in 0..strikes {
            delay = delay.saturating_mul(2);
            if delay >= max {
                return max;
            }
        }
        delay.min(max)
    }

    /// Time left on `key`'s cooldown; `ZERO` when it may be used again.
    /// Expired entries are dropped on access.
    pub fn remaining(&self, key: &str) -> Duration {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Duration::ZERO;
        }
        let mut map = self.lock();
        let Some(state) = map.get(&key).cloned() else {
            return Duration::ZERO;
        };
        let Some(until) = state.until else {
            return Duration::ZERO;
        };
        let now = Instant::now();
        if until <= now {
            map.remove(&key);
            return Duration::ZERO;
        }
        until - now
    }

    /// Is `key` currently rate-limited?
    pub fn is_active(&self, key: &str) -> bool {
        self.remaining(key) > Duration::ZERO
    }

    pub fn mark(&self, key: &str, err: &WsConnectError) {
        let retry_after = retry_after_from_err(err);
        self.mark_for(key, retry_after);
    }

    /// Mark `key` rate-limited, honouring an explicit `Retry-After` if given.
    pub fn mark_for(&self, key: &str, retry_after: Duration) {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        let max = (self.max)();
        let mut map = self.lock();
        let prev = map.get(&key).cloned().unwrap_or_default();
        let delay = self.next_delay(&prev, retry_after);
        let expired = prev.until.is_none_or(|u| u.elapsed() > max);
        let strikes = if expired {
            1
        } else {
            prev.strikes.saturating_add(1)
        };
        map.insert(
            key.clone(),
            CooldownState {
                until: Some(Instant::now() + delay),
                strikes,
            },
        );
        log::debug!(
            "{} cooldown {key}: {:.0}s after HTTP 429",
            self.name,
            delay.as_secs_f64().ceil()
        );
    }

    pub fn clear(&self, key: &str) {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        self.lock().remove(&key);
    }

    #[cfg(test)]
    pub fn clear_all(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Duration {
        Duration::from_secs(45)
    }
    fn max() -> Duration {
        Duration::from_secs(300)
    }

    static CD: Cooldown429 = Cooldown429::new(base, max, "test");

    #[test]
    fn exponential_backoff_caps_at_max() {
        let prev = CooldownState {
            until: Some(Instant::now()),
            strikes: 4,
        };
        assert_eq!(CD.next_delay(&prev, Duration::ZERO), max());
    }

    #[test]
    fn retry_after_wins_over_backoff() {
        let prev = CooldownState::default();
        assert_eq!(
            CD.next_delay(&prev, Duration::from_secs(120)),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn retry_after_clamped_to_max() {
        let prev = CooldownState::default();
        assert_eq!(CD.next_delay(&prev, Duration::from_secs(600)), max());
    }

    #[test]
    fn mark_then_active_then_clear() {
        static M: Cooldown429 = Cooldown429::new(base, max, "test-mark");
        M.clear_all();
        assert!(!M.is_active("a.example"));
        M.mark_for("A.Example", Duration::from_secs(60));
        // Keys are matched case-insensitively.
        assert!(M.is_active("a.example"));
        M.clear("a.example");
        assert!(!M.is_active("a.example"));
    }

    #[test]
    fn empty_key_is_ignored() {
        static E: Cooldown429 = Cooldown429::new(base, max, "test-empty");
        E.mark_for("  ", Duration::from_secs(60));
        assert!(!E.is_active(""));
    }
}
