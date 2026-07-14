//! A small time-keyed set: each key stays "active" until its per-entry TTL
//! elapses, then is pruned lazily on the next access. Backs the per-IP / per-DC
//! / per-domain cooldown and blacklist maps (`ip_fail`, `fronting`,
//! `ws_blacklist`), which previously each reimplemented this pattern.
//!
//! Constructible in a `static` (`const fn new`), so callers don't need
//! `LazyLock`. Lock poisoning is recovered from (`into_inner`) rather than
//! propagated as a panic — one poisoned critical section must not cascade
//! through a resilience-focused daemon.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

fn new_map<K>() -> Mutex<HashMap<K, Instant>> {
    Mutex::new(HashMap::new())
}

pub struct TtlMap<K: 'static> {
    // `HashMap::new` isn't const, so the map is built lazily; a fn-pointer
    // initializer keeps `new()` const so callers can hold a `TtlMap` in a static.
    inner: LazyLock<Mutex<HashMap<K, Instant>>>,
}

impl<K: Eq + Hash> TtlMap<K> {
    pub const fn new() -> Self {
        Self {
            inner: LazyLock::new(new_map),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<K, Instant>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Mark `key` active for `ttl` from now.
    pub fn mark(&self, key: K, ttl: Duration) {
        self.lock().insert(key, Instant::now() + ttl);
    }

    /// Mark `key` only if it is missing or already expired. Returns whether a
    /// new entry was inserted (used to avoid pushing cooldown expiry forward on
    /// every concurrent failure).
    pub fn mark_if_absent(&self, key: K, ttl: Duration) -> bool {
        let mut map = self.lock();
        let now = Instant::now();
        if let Some(expiry) = map.get(&key) {
            if now < *expiry {
                return false;
            }
            map.remove(&key);
        }
        map.insert(key, now + ttl);
        true
    }

    /// Is `key` still within its TTL? Expired entries are removed on access.
    pub fn is_active(&self, key: &K) -> bool {
        let mut map = self.lock();
        match map.get(key) {
            Some(expiry) if Instant::now() < *expiry => true,
            Some(_) => {
                map.remove(key);
                false
            }
            None => false,
        }
    }

    /// Remove `key`; returns whether it was present (and unexpired-aware only in
    /// the sense of raw presence — use for "clear cooldown" semantics).
    pub fn clear(&self, key: &K) -> bool {
        self.lock().remove(key).is_some()
    }

    pub fn clear_all(&self) {
        self.lock().clear();
    }

    #[cfg(test)]
    pub fn contains(&self, key: &K) -> bool {
        self.lock().contains_key(key)
    }

    /// Insert an already-expired entry (test helper for exercising pruning).
    #[cfg(test)]
    pub fn mark_expired(&self, key: K) {
        self.lock()
            .insert(key, Instant::now() - Duration::from_secs(1));
    }
}

impl<K: Eq + Hash> Default for TtlMap<K> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_is_active_clear() {
        let m: TtlMap<(i32, bool)> = TtlMap::new();
        assert!(!m.is_active(&(1, false)));
        m.mark((1, false), Duration::from_secs(60));
        assert!(m.is_active(&(1, false)));
        assert!(!m.is_active(&(2, false)));
        assert!(m.clear(&(1, false)));
        assert!(!m.is_active(&(1, false)));
    }

    #[test]
    fn mark_if_absent_does_not_extend_active_entry() {
        let m: TtlMap<i32> = TtlMap::new();
        assert!(m.mark_if_absent(1, Duration::from_secs(60)));
        assert!(!m.mark_if_absent(1, Duration::from_secs(120)));
        m.mark_expired(1);
        assert!(m.mark_if_absent(1, Duration::from_secs(30)));
        assert!(m.is_active(&1));
    }

    #[test]
    fn expired_entry_is_pruned_on_access() {
        let m: TtlMap<i32> = TtlMap::new();
        m.mark_expired(7);
        assert!(m.contains(&7));
        assert!(!m.is_active(&7)); // access prunes it
        assert!(!m.contains(&7));
    }
}
