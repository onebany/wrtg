//! Round-robin selection across multiple CF Worker / Proxy domains.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

static WORKER_DOMAINS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static PROXY_DOMAINS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static WORKER_RR: AtomicUsize = AtomicUsize::new(0);
static PROXY_RR: AtomicUsize = AtomicUsize::new(0);
static DC_WORKER_IDX: LazyLock<Mutex<HashMap<i32, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static DC_PROXY_IDX: LazyLock<Mutex<HashMap<i32, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static DC_PROXY_STICKY: LazyLock<Mutex<HashMap<i32, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_worker_domains(domains: Vec<String>) {
    *WORKER_DOMAINS.lock().unwrap() = domains;
    WORKER_RR.store(0, Ordering::Relaxed);
    DC_WORKER_IDX.lock().unwrap().clear();
}

pub fn set_proxy_domains(domains: Vec<String>) {
    *PROXY_DOMAINS.lock().unwrap() = domains;
    PROXY_RR.store(0, Ordering::Relaxed);
    DC_PROXY_IDX.lock().unwrap().clear();
    DC_PROXY_STICKY.lock().unwrap().clear();
}

pub fn worker_domains() -> Vec<String> {
    WORKER_DOMAINS.lock().unwrap().clone()
}

pub fn proxy_domains() -> Vec<String> {
    PROXY_DOMAINS.lock().unwrap().clone()
}

pub fn cf_fallback_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("WRTG_NO_CFPROXY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// Route blind-relay (TLS / MTProto-over-HTTP media) through the CF Worker to the
/// real DC instead of the front. On by default when a Worker is configured;
/// disable with `WRTG_NO_WORKER_PASSTHROUGH=1`.
pub fn worker_passthrough_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("WRTG_NO_WORKER_PASSTHROUGH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

fn ordered_domains(
    domains: &[String],
    dc: i32,
    dc_map: &Mutex<HashMap<i32, usize>>,
    rr: &AtomicUsize,
) -> Vec<String> {
    if domains.is_empty() {
        return Vec::new();
    }
    if domains.len() == 1 {
        return domains.to_vec();
    }
    let start = {
        let mut map = dc_map.lock().unwrap();
        let idx = *map.entry(dc).or_insert_with(|| {
            let n = rr.fetch_add(1, Ordering::Relaxed);
            n % domains.len()
        });
        idx % domains.len()
    };
    let mut out = Vec::with_capacity(domains.len());
    for i in 0..domains.len() {
        out.push(domains[(start + i) % domains.len()].clone());
    }
    out
}

/// Worker domains for a DC in round-robin order (primary first, rest as fallback).
pub fn worker_domains_for_dc(dc: i32) -> Vec<String> {
    let domains = worker_domains();
    ordered_domains(&domains, dc, &DC_WORKER_IDX, &WORKER_RR)
}

/// Proxy domains for a DC in round-robin order, sticky successful domain first.
pub fn proxy_domains_for_dc(dc: i32) -> Vec<String> {
    let domains = proxy_domains();
    let sticky = DC_PROXY_STICKY.lock().unwrap().get(&dc).cloned();
    let ordered = ordered_domains(&domains, dc, &DC_PROXY_IDX, &PROXY_RR);
    if let Some(sticky_domain) = sticky {
        if domains.iter().any(|d| d == &sticky_domain) {
            let mut out = vec![sticky_domain];
            for d in ordered {
                if !out.iter().any(|x| x == &d) {
                    out.push(d);
                }
            }
            return out;
        }
    }
    ordered
}

/// Remember which CF proxy base domain worked for a DC (sticky preference).
pub fn update_proxy_domain_for_dc(dc: i32, domain: &str) {
    let domain = domain.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return;
    }
    let mut map = DC_PROXY_STICKY.lock().unwrap();
    if map.get(&dc).map(String::as_str) == Some(domain.as_str()) {
        return;
    }
    map.insert(dc, domain);
}

pub fn parse_domain_list(raw: &str) -> Vec<String> {
    raw.split([',', ';', ' '])
        .map(|s| s.trim().trim_matches('"').trim_matches('\r'))
        .filter(|s| !s.is_empty())
        .map(sanitize_domain)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Reduce a pasted value to a bare host for TLS SNI: `https://x.dev/apiws` → `x.dev`.
/// Users routinely paste a Worker URL with scheme and a trailing slash; feeding
/// that to `connect_ws` fails ("Name does not resolve") or gets silently dropped
/// by domain validation. Strip scheme, path, query, and trailing dots.
fn sanitize_domain(s: &str) -> String {
    let s = s.trim();
    let s = s.split_once("://").map_or(s, |(_, rest)| rest);
    let host = s.split(['/', '?', '#']).next().unwrap_or(s);
    host.trim().trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_domain_list_splits() {
        let v = parse_domain_list("a.example.com, b.example.com;c.example.com");
        assert_eq!(
            v,
            vec![
                "a.example.com".to_string(),
                "b.example.com".to_string(),
                "c.example.com".to_string()
            ]
        );
    }

    #[test]
    fn parse_domain_list_strips_scheme_and_path() {
        // The exact shape users paste from the Cloudflare dashboard.
        let v = parse_domain_list(
            "https://w1.workers.dev/, w2.workers.dev , wss://w3.workers.dev/apiws",
        );
        assert_eq!(
            v,
            vec![
                "w1.workers.dev".to_string(),
                "w2.workers.dev".to_string(),
                "w3.workers.dev".to_string(),
            ]
        );
    }

    #[test]
    fn round_robin_worker_domains() {
        set_worker_domains(vec!["w1.dev".into(), "w2.dev".into()]);
        let a = worker_domains_for_dc(1);
        let b = worker_domains_for_dc(1);
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 2);
        // DC1 sticky — same order within same DC
        assert_eq!(a, b);
        // Different DC may start at different offset
        let c = worker_domains_for_dc(2);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn sticky_proxy_domain_first() {
        set_proxy_domains(vec!["a.co.uk".into(), "b.co.uk".into()]);
        update_proxy_domain_for_dc(1, "b.co.uk");
        let domains = proxy_domains_for_dc(1);
        assert_eq!(domains.first().map(String::as_str), Some("b.co.uk"));
        assert_eq!(domains.len(), 2);
    }
}
