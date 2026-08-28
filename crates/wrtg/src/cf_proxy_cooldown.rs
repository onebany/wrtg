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

/// How long a domain that answered 503/404 for a DC is left alone.
fn dead_cooldown() -> Duration {
    static D: LazyLock<Duration> = LazyLock::new(|| {
        std::env::var("WRTG_CFPROXY_FAIL_COOLDOWN_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(600))
    });
    *D
}

fn dead_max_cooldown() -> Duration {
    static D: LazyLock<Duration> = LazyLock::new(|| {
        std::env::var("WRTG_CFPROXY_FAIL_MAX_COOLDOWN_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(3600))
    });
    *D
}

static CFPROXY_429: Cooldown429 = Cooldown429::new(base_cooldown, max_cooldown, "CF proxy");

/// Domains that answered a persistent HTTP fault, keyed per `(domain, DC)`.
///
/// The shared pool is uneven: a zone can serve `kws4` and 404 on `kws2`, so a
/// per-domain key would retire a domain that most DCs still reach. Measured on
/// the live pool, the working share ran from 6/20 to 16/20 depending on the DC.
static CFPROXY_DEAD: Cooldown429 = Cooldown429::new(dead_cooldown, dead_max_cooldown, "CF proxy");

fn dead_key(domain: &str, dc: i32) -> String {
    let dc = if dc == 203 { 2 } else { dc };
    format!("{domain}#{dc}")
}

/// Whether this status means the domain will keep failing for this DC.
///
/// 404: the zone has no route for that `kws{N}` host. 503: its origin is down.
/// Both persist for as long as whoever runs the domain leaves it that way, and
/// re-dialling only spends the connect budget of every later session. 502 is
/// excluded — the Worker script serves it for an unreachable destination, which
/// is about Telegram, not the domain.
fn is_persistent_fault(status: Option<u16>) -> bool {
    matches!(status, Some(404) | Some(503))
}

pub fn cf_proxy_cooldown_remaining_for(domain: &str, dc: i32) -> Duration {
    CFPROXY_DEAD.remaining(&dead_key(domain, dc))
}

pub fn mark_cf_proxy_dead(domain: &str, dc: i32, err: &WsConnectError) {
    if !is_persistent_fault(err.http_status()) {
        return;
    }
    let key = dead_key(domain, dc);
    CFPROXY_DEAD.mark(&key, err);
    log::info!(
        "CF proxy {domain} DC{dc} answered HTTP {} — skipping it for {}s",
        err.http_status().unwrap_or(0),
        CFPROXY_DEAD.remaining(&key).as_secs().max(1)
    );
}

pub fn clear_cf_proxy_dead(domain: &str, dc: i32) {
    CFPROXY_DEAD.clear(&dead_key(domain, dc));
}

#[cfg(test)]
pub fn reset_cf_proxy_cooldowns() {
    CFPROXY_429.clear_all();
    CFPROXY_DEAD.clear_all();
}

pub fn cf_proxy_cooldown_remaining(domain: &str) -> Duration {
    CFPROXY_429.remaining(domain)
}

pub fn mark_cf_proxy_429_cooldown(domain: &str, err: &WsConnectError) {
    CFPROXY_429.mark(domain, err);
}

pub fn clear_cf_proxy_429_cooldown(domain: &str) {
    CFPROXY_429.clear(domain);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cooldown maps are process-wide, so these tests would clear each
    /// other's marks when cargo runs them in parallel.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn dead(status: u16) -> WsConnectError {
        WsConnectError::Handshake(crate::ws::WsHandshakeError {
            status_code: status,
            status_line: format!("HTTP/1.1 {status}"),
            headers: std::collections::HashMap::new(),
        })
    }

    #[test]
    fn a_dead_domain_is_cooled_per_dc_not_wholesale() {
        // A shared-pool zone serves some DCs and not others: kws4.<domain> can
        // answer 101 while kws2.<domain> answers 404. Cooling the base domain
        // would take the working DCs down with the broken one.
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_cf_proxy_cooldowns();
        mark_cf_proxy_dead(D, 2, &dead(404));
        assert!(cf_proxy_cooldown_remaining_for(D, 2) > Duration::ZERO);
        assert_eq!(cf_proxy_cooldown_remaining_for(D, 4), Duration::ZERO);
    }

    #[test]
    fn only_a_persistent_http_fault_cools_the_domain() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_cf_proxy_cooldowns();
        mark_cf_proxy_dead(D, 1, &dead(503));
        assert!(cf_proxy_cooldown_remaining_for(D, 1) > Duration::ZERO);

        // 502 is the Worker script's answer for an unreachable destination and
        // says nothing about the domain, so it must not park it.
        reset_cf_proxy_cooldowns();
        mark_cf_proxy_dead(D, 1, &dead(502));
        assert_eq!(cf_proxy_cooldown_remaining_for(D, 1), Duration::ZERO);
    }

    #[test]
    fn a_recovered_domain_is_picked_back_up() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_cf_proxy_cooldowns();
        mark_cf_proxy_dead(D, 3, &dead(503));
        assert!(cf_proxy_cooldown_remaining_for(D, 3) > Duration::ZERO);
        clear_cf_proxy_dead(D, 3);
        assert_eq!(cf_proxy_cooldown_remaining_for(D, 3), Duration::ZERO);
    }

    const D: &str = "example.co.uk";
}
