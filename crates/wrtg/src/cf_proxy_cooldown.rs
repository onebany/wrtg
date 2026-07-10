//! Per-CF-domain HTTP 429 cooldown with exponential backoff.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::ws::{retry_after_from_err, WsConnectError};

#[derive(Clone, Default)]
struct CooldownState {
    until: Option<Instant>,
    strikes: u32,
}

static CFPROXY_429: LazyLock<Mutex<HashMap<String, CooldownState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn base_cooldown() -> Duration {
    static D: LazyLock<Duration> = LazyLock::new(|| {
        std::env::var("WRTG_CFPROXY_429_COOLDOWN_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(45))
    });
    *D
}

fn max_cooldown() -> Duration {
    static D: LazyLock<Duration> = LazyLock::new(|| {
        std::env::var("WRTG_CFPROXY_429_MAX_COOLDOWN_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(300))
    });
    *D
}

fn next_delay(prev: &CooldownState, retry_after: Duration) -> Duration {
    let max = max_cooldown();
    if retry_after > Duration::ZERO {
        return retry_after.min(max);
    }
    let expired = prev.until.is_none_or(|u| u.elapsed() > max);
    let strikes = if expired { 0 } else { prev.strikes };
    let mut delay = base_cooldown();
    for _ in 0..strikes {
        delay = delay.saturating_mul(2);
        if delay >= max {
            return max;
        }
    }
    delay.min(max)
}

pub fn cf_proxy_cooldown_remaining(domain: &str) -> Duration {
    let domain = domain.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return Duration::ZERO;
    }
    let mut map = CFPROXY_429.lock().unwrap();
    let state = match map.get(&domain) {
        Some(s) => s.clone(),
        None => return Duration::ZERO,
    };
    let Some(until) = state.until else {
        return Duration::ZERO;
    };
    let now = Instant::now();
    if until <= now {
        map.remove(&domain);
        return Duration::ZERO;
    }
    until - now
}

pub fn mark_cf_proxy_429_cooldown(domain: &str, err: &WsConnectError) {
    let domain = domain.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return;
    }
    let retry_after = retry_after_from_err(err);
    let mut map = CFPROXY_429.lock().unwrap();
    let prev = map.get(&domain).cloned().unwrap_or_default();
    let delay = next_delay(&prev, retry_after);
    let expired = prev.until.is_none_or(|u| u.elapsed() > max_cooldown());
    let strikes = if expired {
        1
    } else {
        prev.strikes.saturating_add(1)
    };
    map.insert(
        domain.clone(),
        CooldownState {
            until: Some(Instant::now() + delay),
            strikes,
        },
    );
    log::debug!(
        "CF proxy cooldown {domain}: {:.0}s after HTTP 429",
        delay.as_secs_f64().ceil()
    );
}

pub fn clear_cf_proxy_429_cooldown(domain: &str) {
    let domain = domain.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return;
    }
    CFPROXY_429.lock().unwrap().remove(&domain);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_caps_at_max() {
        let max = Duration::from_secs(300);
        let prev = CooldownState {
            until: Some(Instant::now()),
            strikes: 4,
        };
        assert_eq!(next_delay(&prev, Duration::ZERO), max);
    }

    #[test]
    fn retry_after_wins_over_backoff() {
        let prev = CooldownState::default();
        assert_eq!(
            next_delay(&prev, Duration::from_secs(120)),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn retry_after_clamped_to_max() {
        let prev = CooldownState::default();
        assert_eq!(
            next_delay(&prev, Duration::from_secs(600)),
            Duration::from_secs(300)
        );
    }
}
