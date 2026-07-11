//! Opt-in TLS fronting fallback: TCP to target IP, Host `kws{N}.web.telegram.org`,
//! SNI from `WRTG_FRONTING_SNI`. Cooldown after failure via `WRTG_FRONTING_COOLDOWN_SEC`.

use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::timeout;

use crate::mtproto::ws_domains;
use crate::ttl_map::TtlMap;
use crate::ws::{connect_ws_fronted, is_ws_redirect_err, RawWebSocket};

const DEFAULT_COOLDOWN_SEC: u64 = 1800;

fn cooldown() -> Duration {
    static D: OnceLock<Duration> = OnceLock::new();
    *D.get_or_init(|| {
        let secs = std::env::var("WRTG_FRONTING_COOLDOWN_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_COOLDOWN_SEC);
        Duration::from_secs(secs)
    })
}

pub fn fronting_sni() -> Option<&'static str> {
    static SNI: OnceLock<Option<String>> = OnceLock::new();
    SNI.get_or_init(|| {
        std::env::var("WRTG_FRONTING_SNI")
            .ok()
            .map(|s| s.trim().trim_matches('\r').to_string())
            .filter(|s| !s.is_empty())
    })
    .as_deref()
}

pub fn fronting_enabled() -> bool {
    fronting_sni().is_some()
}

static FRONTING_FAIL: TtlMap<(String, i32)> = TtlMap::new();

pub fn mark_fronting_failed(ip: &str, dc: i32) {
    if ip.is_empty() {
        return;
    }
    FRONTING_FAIL.mark((ip.to_string(), dc), cooldown());
    log::info!(
        "Fronting {ip} DC{dc} marked failed for {}s",
        cooldown().as_secs()
    );
}

pub fn clear_fronting_fail(ip: &str, dc: i32) {
    if ip.is_empty() {
        return;
    }
    if FRONTING_FAIL.clear(&(ip.to_string(), dc)) {
        log::debug!("Fronting {ip} DC{dc} fail cooldown cleared");
    }
}

pub fn should_skip_fronting(ip: &str, dc: i32) -> bool {
    if ip.is_empty() || !fronting_enabled() {
        return true;
    }
    FRONTING_FAIL.is_active(&(ip.to_string(), dc))
}

pub async fn try_ws_fronting(
    target_ip: &str,
    dc: i32,
    is_media: bool,
    relay_init: &[u8],
    label: &str,
    connect_timeout: Duration,
) -> Result<(RawWebSocket, String), (bool, bool)> {
    let sni = match fronting_sni() {
        Some(s) => s,
        None => return Err((false, false)),
    };

    let mut all_blocked = true;
    let mut timed_out = false;
    let domains = ws_domains(dc, is_media);

    for domain in &domains {
        log::info!("[{label}] DC{dc} -> trying fronting WSS {domain} via {target_ip} sni={sni}");
        match timeout(
            connect_timeout,
            connect_ws_fronted(target_ip, domain, sni, "/apiws", connect_timeout),
        )
        .await
        {
            Err(_) => {
                log::warn!("[{label}] DC{dc} fronting {domain} timeout");
                all_blocked = false;
                timed_out = true;
                continue;
            }
            Ok(Err(e)) => {
                let redirect = is_ws_redirect_err(&e);
                let io_err = e.into_io();
                log::warn!("[{label}] DC{dc} fronting {domain} failed: {io_err}");
                if !redirect {
                    all_blocked = false;
                }
                if io_err.kind() == std::io::ErrorKind::TimedOut {
                    timed_out = true;
                }
                continue;
            }
            Ok(Ok(mut ws)) => {
                if let Err(e) = ws.send(relay_init).await {
                    ws.close().await;
                    log::warn!("[{label}] DC{dc} fronting relay init failed: {e}");
                    all_blocked = false;
                    continue;
                }
                return Ok((ws, domain.clone()));
            }
        }
    }
    Err((all_blocked, timed_out))
}

pub fn reset_all() {
    FRONTING_FAIL.clear_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn fronting_skip_when_disabled() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        assert!(should_skip_fronting("1.2.3.4", 2));
    }

    #[test]
    fn fronting_cooldown_roundtrip() {
        let _g = TEST_LOCK.lock().unwrap();
        reset_all();
        mark_fronting_failed("8.8.8.8", 4);
        FRONTING_FAIL.mark(("8.8.8.8".to_string(), 4), Duration::from_secs(60));
        assert!(should_skip_fronting("8.8.8.8", 4));
        clear_fronting_fail("8.8.8.8", 4);
        if fronting_enabled() {
            assert!(!should_skip_fronting("8.8.8.8", 4));
        }
        reset_all();
    }
}
