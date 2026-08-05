//! Per-CF-Worker HTTP 429/503 cooldown with exponential backoff.
//! Thin wiring over [`crate::cooldown429::Cooldown429`].
//!
//! Cloudflare rate-limits a Worker (and, on the free plan, cuts it off for the
//! rest of the UTC day once the request quota is spent) with HTTP 429. Without
//! a cooldown every new client connection re-dialled every configured Worker,
//! which burned more quota while already over budget, added the full connect
//! latency to each session, and buried the syslog ring buffer in 429 warnings.
//!
//! HTTP 503 gets the same treatment: the Worker script serves it when its
//! `WRTG_TOKEN` secret is not configured (fail-closed against open-relay
//! abuse), which is a persistent deployment fault, not a transient error —
//! yet every connection kept re-dialling the Worker, paying a doomed WSS
//! handshake per session before falling back. Other 5xx are excluded on
//! purpose: the script's 502 means "upstream DC connect failed", which is
//! specific to the requested `dst`, not to the Worker.
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
static CF_WORKER_503: Cooldown429 = Cooldown429::new(base_cooldown, max_cooldown, "CF worker");

/// Is this Worker currently best skipped (429 quota / 503 misconfiguration)?
pub fn worker_rate_limited(worker: &str) -> bool {
    CF_WORKER_429.is_active(worker) || CF_WORKER_503.is_active(worker)
}

pub fn worker_cooldown_remaining(worker: &str) -> Duration {
    CF_WORKER_429
        .remaining(worker)
        .max(CF_WORKER_503.remaining(worker))
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

/// Cloudflare account key of a `*.workers.dev` hostname:
/// `<account-subdomain>.workers.dev`. Anything else (custom domains) has no
/// derivable account, so `None` keeps its cooldown strictly per-host.
fn account_key(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let rest = host.strip_suffix(".workers.dev")?;
    let account = rest.rsplit('.').next().unwrap_or("");
    if account.is_empty() {
        return None;
    }
    Some(format!("{account}.workers.dev"))
}

/// Record a 429 (or 503) from `worker`; a 429 also cools every configured
/// sibling on the same Cloudflare account without dialling them first.
///
/// The Workers Free plan daily request quota is per *account* (100k requests
/// across all Workers, error 1027 served as HTTP 429), so once one
/// `*.workers.dev` sibling is spent, the rest answer 429 too. Dialling them
/// anyway — which the per-worker-only cooldown did on every cooldown expiry —
/// burned more of the missing quota and added a doomed handshake to each
/// connection. Workers on other accounts (or custom domains) keep their own
/// independent cooldown.
pub fn mark_worker_429_with_peers(worker: &str, err: &WsConnectError, peers: &[String]) {
    // 503 (Worker running without its WRTG_TOKEN secret) is a persistent
    // deployment fault: cool just this Worker, not its account siblings —
    // unlike the account-wide 429 quota, the secret is set per Worker. Peer
    // workers still get filtered by their own cooldowns via `usable_workers`.
    if err.http_status() == Some(503) {
        CF_WORKER_503.mark(worker, err);
        log::warn!(
            "CF worker {worker} answered HTTP 503 (misconfigured?) — skipping it for {}s",
            CF_WORKER_503.remaining(worker).as_secs().max(1)
        );
        return;
    }
    if err.http_status() != Some(429) {
        return;
    }
    mark_worker_429(worker, err);
    let Some(account) = account_key(worker) else {
        return;
    };
    let mut cooled = 0usize;
    for peer in peers {
        if peer.eq_ignore_ascii_case(worker) || worker_rate_limited(peer) {
            continue;
        }
        if account_key(peer).as_deref() == Some(account.as_str()) {
            CF_WORKER_429.mark(peer, err);
            cooled += 1;
        }
    }
    if cooled > 0 {
        log::warn!(
            "CF worker 429 is account-wide ({account}) — cooled {cooled} sibling worker(s) without dialling them"
        );
    }
}

/// A Worker answered normally again — drop any cooldown it had.
pub fn clear_worker_429(worker: &str) {
    CF_WORKER_429.clear(worker);
    CF_WORKER_503.clear(worker);
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

    #[test]
    fn account_key_groups_workers_dev_hosts() {
        assert_eq!(
            account_key("square-thunder-aa06.maybebany.workers.dev").as_deref(),
            Some("maybebany.workers.dev")
        );
        assert_eq!(
            account_key("Proud-Surf-5CFE.maybebany.workers.dev.").as_deref(),
            Some("maybebany.workers.dev")
        );
        // Custom domains carry no derivable account.
        assert_eq!(account_key("tg.example.com"), None);
        assert_eq!(account_key("workers.dev"), None);
        assert_eq!(account_key(""), None);
    }

    #[test]
    fn peer_429_cools_same_account_siblings_only() {
        let w1 = "w1.acct-a.workers.dev".to_string();
        let w2 = "w2.acct-a.workers.dev".to_string();
        let other_acct = "w1.acct-b.workers.dev".to_string();
        let custom = "relay.example.com".to_string();
        for w in [&w1, &w2, &other_acct, &custom] {
            clear_worker_429(w);
        }
        let peers = vec![w1.clone(), w2.clone(), other_acct.clone(), custom.clone()];
        mark_worker_429_with_peers(&w1, &handshake_err(429), &peers);
        assert!(worker_rate_limited(&w1));
        assert!(worker_rate_limited(&w2), "same-account sibling must cool");
        assert!(
            !worker_rate_limited(&other_acct),
            "other account keeps its own cooldown"
        );
        assert!(
            !worker_rate_limited(&custom),
            "custom domain keeps its own cooldown"
        );
        for w in [&w1, &w2, &other_acct, &custom] {
            clear_worker_429(w);
        }
    }

    #[test]
    fn peer_non_429_marks_nothing() {
        let w1 = "n1.acct-c.workers.dev".to_string();
        let w2 = "n2.acct-c.workers.dev".to_string();
        clear_worker_429(&w1);
        clear_worker_429(&w2);
        mark_worker_429_with_peers(&w1, &handshake_err(502), &[w1.clone(), w2.clone()]);
        assert!(!worker_rate_limited(&w1));
        assert!(!worker_rate_limited(&w2));
    }

    #[test]
    fn http_503_cools_the_worker() {
        let w = "svc503.example".to_string();
        clear_worker_429(&w);
        mark_worker_429_with_peers(&w, &handshake_err(503), &[w.clone()]);
        assert!(worker_rate_limited(&w), "503 must cool the worker");
        assert!(worker_cooldown_remaining(&w) > Duration::ZERO);
        clear_worker_429(&w);
        assert!(!worker_rate_limited(&w), "clear must drop the 503 cooldown");
    }

    #[test]
    fn http_503_does_not_cool_siblings() {
        let w1 = "s1.acct-d.workers.dev".to_string();
        let w2 = "s2.acct-d.workers.dev".to_string();
        clear_worker_429(&w1);
        clear_worker_429(&w2);
        mark_worker_429_with_peers(&w1, &handshake_err(503), &[w1.clone(), w2.clone()]);
        assert!(worker_rate_limited(&w1));
        assert!(
            !worker_rate_limited(&w2),
            "503 is a per-worker fault, siblings stay usable"
        );
        clear_worker_429(&w1);
    }

    #[test]
    fn other_5xx_marks_nothing() {
        // 502 is the Worker script's "upstream DC connect failed" — specific
        // to the requested dst, not to the Worker; cooling would punish the
        // Worker for one bad target.
        let w = "svc502.example".to_string();
        clear_worker_429(&w);
        mark_worker_429_with_peers(&w, &handshake_err(502), &[w.clone()]);
        mark_worker_429_with_peers(&w, &handshake_err(500), &[w.clone()]);
        assert!(!worker_rate_limited(&w));
    }
}
