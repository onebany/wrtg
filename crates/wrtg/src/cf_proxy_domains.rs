//! Auto-fetch shared CF Proxy domain pool from Flowseal/tg-ws-proxy (hourly).

use std::time::Duration;

use rand::Rng;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::cf_balancer::{cf_fallback_disabled, set_proxy_domains};

pub const CFPROXY_DOMAINS_URL: &str =
    "https://raw.githubusercontent.com/Flowseal/tg-ws-proxy/main/.github/cfproxy-domains.txt";

const REFRESH_INTERVAL: Duration = Duration::from_secs(3600);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HTTP_RESPONSE: usize = 1024 * 1024;
const MIN_VALID_DOMAINS: usize = 3;
const DOMAIN_SUFFIX: &str = ".co.uk";

const DEFAULT_ENCODED: &[&str] = &[
    "virkgj.com",
    "vmmzovy.com",
    "mkuosckvso.com",
    "zaewayzmplad.com",
    "twdmbzcm.com",
    "awzwsldi.com",
    "clngqrflngqin.com",
    "tjacxbqtj.com",
    "bxaxtxmrw.com",
    "dmohrsgmohcrwb.com",
    "vwbmtmoi.com",
    "khgrre.com",
    "ulihssf.com",
    "tmhqsdqmfpmk.com",
    "xwuwoqbm.com",
    "orgcnunpj.com",
    "zhkuldz.com",
    "zypoljnslxa.com",
    "efabnxaowuzs.com",
    "zaftuzsftqdq.com",
];

/// Decode obfuscated CF proxy domain (Flowseal Caesar cipher, suffix `.co.uk`).
pub fn decode_cfproxy_domain(s: &str) -> String {
    if !s.ends_with(".com") {
        return s.to_string();
    }
    let prefix = &s[..s.len() - 4];
    let shift = prefix.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let decoded: String = prefix
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                let base = b'a';
                let off = (c as u8 - base) as i32 - shift as i32;
                char::from(base + off.rem_euclid(26) as u8)
            } else if c.is_ascii_uppercase() {
                let base = b'A';
                let off = (c as u8 - base) as i32 - shift as i32;
                char::from(base + off.rem_euclid(26) as u8)
            } else {
                c
            }
        })
        .collect();
    format!("{decoded}{DOMAIN_SUFFIX}")
}

pub fn default_cfproxy_domains() -> Vec<String> {
    DEFAULT_ENCODED
        .iter()
        .map(|d| decode_cfproxy_domain(d))
        .collect()
}

pub fn user_proxy_domains_configured() -> bool {
    proxy_env_set("CF_PROXY_DOMAIN") || proxy_env_set("WRTG_CF_PROXY_DOMAINS")
}

fn proxy_env_set(key: &str) -> bool {
    std::env::var(key)
        .map(|v| !v.trim().trim_matches('"').is_empty())
        .unwrap_or(false)
}

/// Auto-fetch is opt-in because public CF domain pools are untrusted and can
/// introduce long fallback delays when stale.
pub fn cfproxy_auto_enabled() -> bool {
    if cf_fallback_disabled() {
        return false;
    }
    if user_proxy_domains_configured() {
        return false;
    }
    match std::env::var("WRTG_CFPROXY_AUTO") {
        Ok(v) => {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
}

pub fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.len() > 253 {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    for label in &labels {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    let tld = labels[labels.len() - 1];
    tld.len() >= 2 && tld.chars().any(|c| c.is_ascii_alphabetic())
}

pub fn normalize_domain_pool(domains: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for domain in domains {
        let item = domain.trim().to_ascii_lowercase();
        if !is_valid_domain(&item) || seen.contains(&item) {
            continue;
        }
        seen.insert(item.clone());
        out.push(item);
    }
    out
}

pub fn apply_proxy_domains(domains: Vec<String>) {
    set_proxy_domains(domains);
}

pub async fn fetch_cfproxy_domains() -> Vec<String> {
    let cache_bust: String = rand::thread_rng()
        .sample_iter(rand::distributions::Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let path = format!("/Flowseal/tg-ws-proxy/main/.github/cfproxy-domains.txt?{cache_bust}");

    let body = match https_get("raw.githubusercontent.com", &path, FETCH_TIMEOUT).await {
        Ok(b) => b,
        Err(e) => {
            log::warn!("Failed to fetch CF proxy domain list: {e}");
            return Vec::new();
        }
    };

    let text = String::from_utf8_lossy(&body);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(decode_cfproxy_domain)
        .collect()
}

pub async fn refresh_cfproxy_domains() {
    if !cfproxy_auto_enabled() {
        return;
    }

    let fetched = fetch_cfproxy_domains().await;
    let pool = normalize_domain_pool(&fetched);
    if pool.len() >= MIN_VALID_DOMAINS {
        log::info!(
            "CF proxy domain pool updated from GitHub ({} domains)",
            pool.len()
        );
        apply_proxy_domains(pool);
        return;
    }

    if fetched.is_empty() {
        log::warn!("CF proxy domain refresh failed or empty response; keeping current domain pool");
    } else {
        log::warn!(
            "Ignoring fetched CF proxy domains due to low-quality payload \
             (total={}, valid={}, required>={MIN_VALID_DOMAINS}); keeping current domain pool",
            fetched.len(),
            pool.len()
        );
    }
}

pub fn seed_default_cfproxy_domains() {
    let defaults = default_cfproxy_domains();
    log::info!(
        "CF proxy auto-fetch enabled ({} built-in domains until GitHub refresh)",
        defaults.len()
    );
    apply_proxy_domains(defaults);
}

pub fn start_cfproxy_refresh_task() {
    tokio::spawn(async {
        refresh_cfproxy_domains().await;
        loop {
            tokio::time::sleep(REFRESH_INTERVAL).await;
            refresh_cfproxy_domains().await;
        }
    });
}

/// GitHub raw content is fronted by these Fastly anycast IPs. Connecting to them
/// directly (TLS SNI / Host stay `raw.githubusercontent.com`) bypasses ISP DNS
/// poisoning of the hostname, so the domain-list refresh keeps working.
const GITHUB_PINNED_IPS: &[&str] = &[
    "185.199.108.133",
    "185.199.109.133",
    "185.199.110.133",
    "185.199.111.133",
];

fn is_github_host(host: &str) -> bool {
    host == "raw.githubusercontent.com" || host.ends_with(".githubusercontent.com")
}

/// TCP-connect to `host:443`, trying pinned GitHub IPs first (if applicable),
/// then the system resolver as a last resort.
async fn connect_pinned(host: &str, connect_timeout: Duration) -> std::io::Result<TcpStream> {
    let mut candidates: Vec<String> = Vec::new();
    if is_github_host(host) {
        candidates.extend(GITHUB_PINNED_IPS.iter().map(|s| s.to_string()));
    }
    candidates.push(host.to_string());

    let mut last_err: Option<std::io::Error> = None;
    for target in candidates {
        let addr = format!("{target}:443");
        match timeout(connect_timeout, TcpStream::connect(&addr)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => last_err = Some(e),
            Err(_) => {
                last_err = Some(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "connect timeout",
                ))
            }
        }
    }
    Err(last_err
        .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no connect target")))
}

async fn https_get(host: &str, path: &str, connect_timeout: Duration) -> std::io::Result<Vec<u8>> {
    let tcp = connect_pinned(host, connect_timeout).await?;

    let connector = crate::tls::connector();
    let server_name = ServerName::try_from(host.to_string()).map_err(std::io::Error::other)?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp)).await??;

    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: wrtg\r\n\
         Connection: close\r\n\
         \r\n"
    );
    timeout(connect_timeout, stream.write_all(req.as_bytes())).await??;

    let mut buf = Vec::new();
    let mut limited = stream.take((MAX_HTTP_RESPONSE + 1) as u64);
    timeout(connect_timeout, limited.read_to_end(&mut buf)).await??;
    if buf.len() > MAX_HTTP_RESPONSE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP response too large",
        ));
    }

    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no HTTP headers"))?;
    let body = buf[header_end + 4..].to_vec();

    let status = String::from_utf8_lossy(&buf[..header_end.min(32)]);
    if !status.contains(" 200 ") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("HTTP error: {}", status.lines().next().unwrap_or("")),
        ));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_cfproxy_domain_shifts_and_replaces_suffix() {
        assert_eq!(decode_cfproxy_domain("virkgj.com"), "pclead.co.uk");
        assert_eq!(decode_cfproxy_domain("plain.co.uk"), "plain.co.uk");
    }

    #[test]
    fn default_pool_has_twenty_domains() {
        assert_eq!(default_cfproxy_domains().len(), 20);
        assert!(default_cfproxy_domains()
            .iter()
            .all(|d| d.ends_with(DOMAIN_SUFFIX)));
    }

    #[test]
    fn normalize_domain_pool_dedupes_and_validates() {
        let raw = vec![
            "Good.Example.com".into(),
            "good.example.com".into(),
            "bad".into(),
            "x..y.com".into(),
        ];
        let out = normalize_domain_pool(&raw);
        assert_eq!(out, vec!["good.example.com".to_string()]);
    }

    #[test]
    fn github_host_pinning() {
        assert!(is_github_host("raw.githubusercontent.com"));
        assert!(is_github_host("release-assets.githubusercontent.com"));
        assert!(!is_github_host("example.com"));
        assert!(!is_github_host("githubusercontent.com.evil.com"));
        assert_eq!(GITHUB_PINNED_IPS.len(), 4);
    }

    #[test]
    fn cfproxy_auto_defaults_off_without_user_domain() {
        std::env::remove_var("WRTG_NO_CFPROXY");
        std::env::remove_var("WRTG_CFPROXY_AUTO");
        std::env::remove_var("CF_PROXY_DOMAIN");
        std::env::remove_var("WRTG_CF_PROXY_DOMAINS");
        assert!(!cfproxy_auto_enabled());
    }
}
