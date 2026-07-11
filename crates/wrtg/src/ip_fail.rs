//! Per-IP cooldown after WS connect timeouts (skip direct WS to FRONT_IP).
//! Per-DC adaptive WS connect timeout after WS failure (`dc_fail_until`).

use std::sync::OnceLock;
use std::time::Duration;

use crate::ttl_map::TtlMap;

static IP_FAIL: TtlMap<(String, i32)> = TtlMap::new();
static DC_FAIL: TtlMap<(i32, bool)> = TtlMap::new();

const DEFAULT_COOLDOWN_SEC: u64 = 3600;
const DEFAULT_DC_FAIL_COOLDOWN_SEC: u64 = 60;
const DEFAULT_WS_TIMEOUT_SEC: u64 = 5;
const DEFAULT_WS_TIMEOUT_FAST_SEC: u64 = 2;

fn cooldown() -> Duration {
    static D: OnceLock<Duration> = OnceLock::new();
    *D.get_or_init(|| {
        let secs = std::env::var("WRTG_IP_FAIL_COOLDOWN_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_COOLDOWN_SEC);
        Duration::from_secs(secs)
    })
}

pub fn mark_ip_failed(ip: &str, dc: i32) {
    if ip.is_empty() {
        return;
    }
    IP_FAIL.mark((ip.to_string(), dc), cooldown());
    log::info!(
        "IP {ip} DC{dc} marked failed for {}s (skip direct WS)",
        cooldown().as_secs()
    );
}

pub fn clear_ip_fail(ip: &str, dc: i32) {
    if ip.is_empty() {
        return;
    }
    if IP_FAIL.clear(&(ip.to_string(), dc)) {
        log::debug!("IP {ip} DC{dc} fail cooldown cleared");
    }
}

pub fn should_skip_direct_ws(ip: &str, dc: i32) -> bool {
    !ip.is_empty() && IP_FAIL.is_active(&(ip.to_string(), dc))
}

fn dc_fail_cooldown() -> Duration {
    static D: OnceLock<Duration> = OnceLock::new();
    *D.get_or_init(|| {
        let secs = std::env::var("WRTG_DC_FAIL_COOLDOWN_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_DC_FAIL_COOLDOWN_SEC);
        Duration::from_secs(secs)
    })
}

fn ws_timeout_normal() -> Duration {
    static D: OnceLock<Duration> = OnceLock::new();
    *D.get_or_init(|| {
        let secs = std::env::var("WRTG_WS_FAIL_TIMEOUT_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_WS_TIMEOUT_SEC);
        Duration::from_secs(secs.max(1))
    })
}

fn ws_timeout_fast() -> Duration {
    static D: OnceLock<Duration> = OnceLock::new();
    *D.get_or_init(|| {
        let secs = std::env::var("WRTG_WS_FAIL_TIMEOUT_FAST_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_WS_TIMEOUT_FAST_SEC);
        Duration::from_secs(secs.max(1))
    })
}

pub fn ws_connect_timeout(dc: i32, is_media: bool) -> Duration {
    if DC_FAIL.is_active(&(dc, is_media)) {
        ws_timeout_fast()
    } else {
        ws_timeout_normal()
    }
}

pub fn mark_dc_failed(dc: i32, is_media: bool) {
    DC_FAIL.mark((dc, is_media), dc_fail_cooldown());
    log::info!(
        "DC{dc}{} marked failed for {}s (WS timeout {}s)",
        if is_media { "m" } else { "" },
        dc_fail_cooldown().as_secs(),
        ws_timeout_fast().as_secs()
    );
}

pub fn clear_dc_fail(dc: i32, is_media: bool) {
    if DC_FAIL.clear(&(dc, is_media)) {
        log::debug!(
            "DC{dc}{} fail cooldown cleared",
            if is_media { "m" } else { "" }
        );
    }
}

pub fn reset_all() {
    IP_FAIL.clear_all();
    DC_FAIL.clear_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Both tests mutate the global IP_FAIL map (incl. reset_all); serialize them.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn ip_fail_roundtrip() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        assert!(!should_skip_direct_ws("1.2.3.4", 2));
        mark_ip_failed("1.2.3.4", 2);
        assert!(should_skip_direct_ws("1.2.3.4", 2));
        assert!(!should_skip_direct_ws("1.2.3.4", 1));
        clear_ip_fail("1.2.3.4", 2);
        assert!(!should_skip_direct_ws("1.2.3.4", 2));
        reset_all();
    }

    #[test]
    fn ip_fail_expiry() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        let key = ("9.9.9.9".to_string(), 3);
        // Already-expired entry: should_skip must drop it and return false.
        IP_FAIL.mark_expired(key.clone());
        assert!(!should_skip_direct_ws("9.9.9.9", 3));
        assert!(!IP_FAIL.contains(&key));
        // Future entry: still skipping.
        IP_FAIL.mark(key.clone(), Duration::from_secs(60));
        assert!(should_skip_direct_ws("9.9.9.9", 3));
        reset_all();
    }

    #[test]
    fn dc_fail_adaptive_timeout() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        assert_eq!(ws_connect_timeout(2, false), ws_timeout_normal());
        mark_dc_failed(2, false);
        assert_eq!(ws_connect_timeout(2, false), ws_timeout_fast());
        assert_eq!(ws_connect_timeout(3, false), ws_timeout_normal());
        DC_FAIL.mark_expired((2, false));
        assert_eq!(ws_connect_timeout(2, false), ws_timeout_normal());
        clear_dc_fail(2, false);
        reset_all();
    }
}
