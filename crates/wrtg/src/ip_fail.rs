//! Per-IP cooldown after WS connect timeouts (skip direct WS to FRONT_IP).
//! Per-DC adaptive WS connect timeout after WS failure (`dc_fail_until`).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

static IP_FAIL: LazyLock<Mutex<HashMap<(String, i32), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static DC_FAIL: LazyLock<Mutex<HashMap<(i32, bool), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
    let expiry = Instant::now() + cooldown();
    IP_FAIL.lock().unwrap().insert((ip.to_string(), dc), expiry);
    log::info!(
        "IP {ip} DC{dc} marked failed for {}s (skip direct WS)",
        cooldown().as_secs()
    );
}

pub fn clear_ip_fail(ip: &str, dc: i32) {
    if ip.is_empty() {
        return;
    }
    if IP_FAIL
        .lock()
        .unwrap()
        .remove(&(ip.to_string(), dc))
        .is_some()
    {
        log::debug!("IP {ip} DC{dc} fail cooldown cleared");
    }
}

pub fn should_skip_direct_ws(ip: &str, dc: i32) -> bool {
    if ip.is_empty() {
        return false;
    }
    let mut map = IP_FAIL.lock().unwrap();
    let key = (ip.to_string(), dc);
    let Some(expiry) = map.get(&key) else {
        return false;
    };
    if Instant::now() < *expiry {
        return true;
    }
    map.remove(&key);
    log::info!("IP {ip} DC{dc} fail cooldown expired, direct WS retry allowed");
    false
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
    let mut map = DC_FAIL.lock().unwrap();
    let key = (dc, is_media);
    let Some(expiry) = map.get(&key) else {
        return ws_timeout_normal();
    };
    if Instant::now() < *expiry {
        ws_timeout_fast()
    } else {
        map.remove(&key);
        log::debug!("DC{dc} fail cooldown expired, WS timeout back to normal");
        ws_timeout_normal()
    }
}

pub fn mark_dc_failed(dc: i32, is_media: bool) {
    let expiry = Instant::now() + dc_fail_cooldown();
    DC_FAIL.lock().unwrap().insert((dc, is_media), expiry);
    log::info!(
        "DC{dc}{} marked failed for {}s (WS timeout {}s)",
        if is_media { "m" } else { "" },
        dc_fail_cooldown().as_secs(),
        ws_timeout_fast().as_secs()
    );
}

pub fn clear_dc_fail(dc: i32, is_media: bool) {
    if DC_FAIL.lock().unwrap().remove(&(dc, is_media)).is_some() {
        log::debug!(
            "DC{dc}{} fail cooldown cleared",
            if is_media { "m" } else { "" }
        );
    }
}

pub fn reset_all() {
    IP_FAIL.lock().unwrap().clear();
    DC_FAIL.lock().unwrap().clear();
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
        IP_FAIL
            .lock()
            .unwrap()
            .insert(key.clone(), Instant::now() - Duration::from_secs(1));
        assert!(!should_skip_direct_ws("9.9.9.9", 3));
        assert!(!IP_FAIL.lock().unwrap().contains_key(&key));
        // Future entry: still skipping.
        IP_FAIL
            .lock()
            .unwrap()
            .insert(key.clone(), Instant::now() + Duration::from_secs(60));
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
        DC_FAIL
            .lock()
            .unwrap()
            .insert((2, false), Instant::now() - Duration::from_secs(1));
        assert_eq!(ws_connect_timeout(2, false), ws_timeout_normal());
        clear_dc_fail(2, false);
        reset_all();
    }
}
