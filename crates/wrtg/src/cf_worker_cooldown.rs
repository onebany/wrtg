//! Per-CF-Worker HTTP 429 cooldown with exponential backoff.
//! Thin wiring over [`crate::cooldown429::Cooldown429`].
//!
//! Cloudflare rate-limits a Worker (and, on the free plan, cuts it off for the
//! rest of the UTC day once the request quota is spent) with HTTP 429. Without
//! a cooldown every new client connection re-dialled every configured Worker,
//! which burned more quota while already over budget, added the full connect
//! latency to each session, and buried the syslog ring buffer in 429 warnings.
//!
//! Defaults are deliberately longer than the CF-proxy ones: a 429 here is
//! usually a spent daily quota that only resets at 00:00 UTC, so probing every
//! 45 s is pointless. Backing off to 15 min keeps the recovery probe cheap
//! (~96 requests/day) while still picking the Worker back up quickly once
//! Cloudflare lifts the limit.

use std::sync::LazyLock;
use std::time::Duration;

use crate::cooldown429::Cooldown429;
use crate::ws::WsConnectError;

const DEFAULT_COOLDOWN_SEC: u64 = 60;
const DEFAULT_MAX_COOLDOWN_SEC: u64 = 900;

fn base_cooldown() -> Duration {
    static D: LazyLock<Duration> = LazyLock::new(|| {
        std::env::var("WRTG_CF_WORKER_429_COOLDOWN_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(DEFAULT_COOLDOWN_SEC))
    });
    *D
}

fn max_cooldown() -> Duration {
    static D: LazyLock<Duration> = LazyLock::new(|| {
        std::env::var("WRTG_CF_WORKER_429_MAX_COOLDOWN_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(DEFAULT_MAX_COOLDOWN_SEC))
    });
    *D
}

static CF_WORKER_429: Cooldown429 = Cooldown429::new(base_cooldown, max_cooldown, "CF worker");

/// Is this Worker currently rate-limited (429) and best skipped?
pub fn worker_rate_limited(worker: &str) -> bool {
    CF_WORKER_429.is_active(worker)
}

pub fn worker_cooldown_remaining(worker: &str) -> Duration {
    CF_WORKER_429.remaining(worker)
}

/// Record a 429 from `worker`. Ignores every other failure: a timeout or a
/// dead TLS endpoint says nothing about the request quota.
pub fn mark_worker_429(worker: &str, err: &WsConnectError) {
    if err.http_status() != Some(429) {
        return;
    }
    CF_WORKER_429.mark(worker, err);
    log::warn!(
        "CF worker {worker} rate-limited (HTTP 429) — skipping it for {}s",
        worker_cooldown_remaining(worker).as_secs().max(1)
    );
}

/// A Worker answered normally again — drop any cooldown it had.
pub fn clear_worker_429(worker: &str) {
    CF_WORKER_429.clear(worker);
}

/// Split `workers` into the ones usable now and the count on cooldown, so
/// callers can skip the rate-limited ones without re-querying per entry.
pub fn usable_workers(workers: Vec<String>) -> (Vec<String>, usize) {
    let total = workers.len();
    let usable: Vec<String> = workers
        .into_iter()
        .filter(|w| !worker_rate_limited(w))
        .collect();
    let cooling = total - usable.len();
    (usable, cooling)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::WsHandshakeError;
    use std::collections::HashMap;

    fn handshake_err(code: u16) -> WsConnectError {
        WsConnectError::Handshake(WsHandshakeError {
            status_code: code,
            status_line: format!("HTTP/1.1 {code}"),
            headers: HashMap::new(),
        })
    }

    #[test]
    fn only_429_marks_a_cooldown() {
        clear_worker_429("only429.example");
        mark_worker_429("only429.example", &handshake_err(502));
        assert!(!worker_rate_limited("only429.example"));
        mark_worker_429("only429.example", &handshake_err(429));
        assert!(worker_rate_limited("only429.example"));
        clear_worker_429("only429.example");
        assert!(!worker_rate_limited("only429.example"));
    }

    #[test]
    fn timeout_does_not_mark() {
        clear_worker_429("timeout.example");
        mark_worker_429("timeout.example", &WsConnectError::Timeout);
        assert!(!worker_rate_limited("timeout.example"));
    }

    #[test]
    fn usable_workers_filters_cooling_ones() {
        let a = "usable-a.example".to_string();
        let b = "usable-b.example".to_string();
        clear_worker_429(&a);
        clear_worker_429(&b);
        mark_worker_429(&b, &handshake_err(429));
        let (usable, cooling) = usable_workers(vec![a.clone(), b.clone()]);
        assert_eq!(usable, vec![a.clone()]);
        assert_eq!(cooling, 1);
        clear_worker_429(&b);
    }
}
