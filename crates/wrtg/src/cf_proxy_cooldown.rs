//! Per-CF-proxy-domain HTTP 429 cooldown with exponential backoff.
//! Thin wiring over [`crate::cooldown429::Cooldown429`].

use std::sync::LazyLock;
use std::time::Duration;

use crate::cooldown429::Cooldown429;
use crate::ws::WsConnectError;

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

static CFPROXY_429: Cooldown429 = Cooldown429::new(base_cooldown, max_cooldown, "CF proxy");

pub fn cf_proxy_cooldown_remaining(domain: &str) -> Duration {
    CFPROXY_429.remaining(domain)
}

pub fn mark_cf_proxy_429_cooldown(domain: &str, err: &WsConnectError) {
    CFPROXY_429.mark(domain, err);
}

pub fn clear_cf_proxy_429_cooldown(domain: &str) {
    CFPROXY_429.clear(domain);
}
