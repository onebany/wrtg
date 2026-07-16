//! DNS-over-HTTPS fallback when hostname dial fails (Cloudflare / Google / Quad9 / AdGuard race).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

const DOH_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const DOH_CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_DOH_BODY: usize = 16 * 1024;

static DOH_CACHE: LazyLock<Mutex<HashMap<String, (String, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn doh_cache_ttl() -> Duration {
    static TTL: LazyLock<Duration> = LazyLock::new(|| {
        std::env::var("WRTG_DOH_CACHE_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(300))
    });
    *TTL
}

struct DohEndpoint {
    /// TLS SNI + HTTP Host (the cert is validated against this name).
    host: &'static str,
    path: &'static str,
    /// Well-known anycast IPs, dialed directly so DoH does not depend on the
    /// system resolver it exists to bypass.
    ips: &'static [&'static str],
}

const DOH_ENDPOINTS: &[DohEndpoint] = &[
    DohEndpoint {
        host: "cloudflare-dns.com",
        path: "/dns-query",
        ips: &["1.1.1.1", "1.0.0.1"],
    },
    DohEndpoint {
        host: "dns.google",
        path: "/dns-query",
        ips: &["8.8.8.8", "8.8.4.4"],
    },
    DohEndpoint {
        host: "dns.quad9.net",
        path: "/dns-query",
        ips: &["9.9.9.9", "149.112.112.112"],
    },
    DohEndpoint {
        host: "dns.adguard-dns.com",
        path: "/dns-query",
        ips: &["94.140.14.14", "94.140.15.15"],
    },
];

fn pick_preferred_ip(candidates: &[String]) -> Option<String> {
    let mut fallback_v6 = None;
    for c in candidates {
        let c = c.trim();
        if let Ok(ip) = c.parse::<IpAddr>() {
            match ip {
                IpAddr::V4(v4) => return Some(v4.to_string()),
                IpAddr::V6(v6) if fallback_v6.is_none() => fallback_v6 = Some(v6.to_string()),
                _ => {}
            }
        }
    }
    fallback_v6
}

/// Does this answer record carry DNS type A (value exactly 1)? Guards against
/// `"type":1` being a prefix of `"type":15/16/18/…`, which would otherwise pull
/// non-A record data (CNAME/TXT/…) into the address list.
fn record_is_type_a(rec: &str) -> bool {
    let mut hay = rec;
    while let Some(pos) = hay.find("\"type\":1") {
        let after = &hay[pos + "\"type\":1".len()..];
        if after.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            hay = after; // e.g. "type":15 — keep scanning
        } else {
            return true;
        }
    }
    false
}

fn parse_doh_a_records(body: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(body);
    let mut out = Vec::new();
    // Only look inside the "Answer" array, then walk each `{…}` record. DoH
    // answer records are flat objects, so splitting on `{` isolates them, and
    // the first `]` after the opening `[` closes the array — bound the scan
    // there so type-A records in a later "Authority" / "Additional" (glue)
    // section can't leak an address that was never in the answer.
    let Some((_, after)) = text.split_once("\"Answer\"") else {
        return out;
    };
    let Some((_, array)) = after.split_once('[') else {
        return out;
    };
    let answer = array.split(']').next().unwrap_or(array);
    for rec in answer.split('{').skip(1) {
        if !record_is_type_a(rec) {
            continue;
        }
        if let Some(data) = rec.split("\"data\":\"").nth(1) {
            if let Some(ip) = data.split('"').next() {
                let ip = ip.trim();
                if !ip.is_empty() {
                    out.push(ip.to_string());
                }
            }
        }
    }
    out
}

async fn doh_query(ep: &DohEndpoint, domain: &str) -> Option<String> {
    let path = format!("{}?name={domain}&type=A", ep.path);
    for ip in ep.ips {
        let body = timeout(
            DOH_QUERY_TIMEOUT,
            https_get_json(ip, ep.host, &path, DOH_CONNECT_TIMEOUT),
        )
        .await
        .ok()
        .and_then(|r| r.ok());
        if let Some(body) = body {
            if let Some(resolved) = pick_preferred_ip(&parse_doh_a_records(&body)) {
                return Some(resolved);
            }
        }
    }
    None
}

async fn https_get_json(
    connect_ip: &str,
    host: &str,
    path: &str,
    connect_timeout: Duration,
) -> std::io::Result<Vec<u8>> {
    // Dial the pinned IP directly; TLS SNI + Host stay the hostname, so the
    // certificate is still validated against the real resolver name.
    let addr = format!("{connect_ip}:443");
    let tcp = timeout(connect_timeout, TcpStream::connect(&addr)).await??;
    crate::https::get_over(
        tcp,
        host,
        path,
        &[("Accept", "application/dns-json")],
        MAX_DOH_BODY,
        connect_timeout,
    )
    .await
}

async fn system_lookup(domain: &str) -> Option<String> {
    let host = format!("{domain}:443");
    let addrs = timeout(DOH_CONNECT_TIMEOUT, tokio::net::lookup_host(host))
        .await
        .ok()?
        .ok()?;
    let ips: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
    pick_preferred_ip(&ips)
}

/// Resolve `domain` via DoH race (+ system lookup). Results cached for 5 minutes.
pub async fn resolve_doh(domain: &str) -> Option<String> {
    let domain = domain.trim();
    if domain.is_empty() {
        return None;
    }

    {
        let cache = DOH_CACHE.lock().unwrap();
        if let Some((ip, exp)) = cache.get(domain) {
            if Instant::now() < *exp {
                return Some(ip.clone());
            }
        }
    }

    let (tx, mut rx) = mpsc::channel(DOH_ENDPOINTS.len() + 1);
    let mut tasks = Vec::new();

    for ep in DOH_ENDPOINTS {
        let tx = tx.clone();
        let domain = domain.to_string();
        tasks.push(tokio::spawn(async move {
            let _ = tx.send(doh_query(ep, &domain).await).await;
        }));
    }

    {
        let tx = tx.clone();
        let domain = domain.to_string();
        tasks.push(tokio::spawn(async move {
            if let Some(ip) = system_lookup(&domain).await {
                let _ = tx.send(Some(ip)).await;
            } else {
                let _ = tx.send(None).await;
            }
        }));
    }
    drop(tx);

    let deadline = tokio::time::sleep(DOH_CONNECT_TIMEOUT);
    tokio::pin!(deadline);

    let mut final_ip = None;
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            msg = rx.recv() => {
                match msg {
                    Some(Some(ip)) => {
                        final_ip = Some(ip);
                        break;
                    }
                    Some(None) => {}
                    None => break,
                }
            }
        }
    }

    for t in tasks {
        t.abort();
    }

    if let Some(ref ip) = final_ip {
        let mut cache = DOH_CACHE.lock().unwrap();
        cache.insert(
            domain.to_string(),
            (ip.clone(), Instant::now() + doh_cache_ttl()),
        );
    }

    final_ip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_doh_a_records_extracts_ipv4() {
        let json = br#"{"Status":0,"Answer":[{"name":"x.co.uk","type":1,"TTL":300,"data":"104.21.75.42"}]}"#;
        let ips = parse_doh_a_records(json);
        assert_eq!(ips, vec!["104.21.75.42".to_string()]);
    }

    #[test]
    fn parse_doh_a_records_skips_non_a_records() {
        // A CNAME (type 5) plus a TXT (type 16, whose "type":16 starts with
        // "type":1) precede the real A record; only the A address is returned.
        let json = br#"{"Answer":[
            {"name":"x.co.uk","type":5,"data":"alias.example."},
            {"name":"x.co.uk","type":16,"data":"v=spf1 -all"},
            {"name":"x.co.uk","type":1,"data":"104.21.75.42"}
        ]}"#;
        let ips = parse_doh_a_records(json);
        assert_eq!(ips, vec!["104.21.75.42".to_string()]);
    }

    #[test]
    fn parse_doh_a_records_ignores_non_answer_sections() {
        // A type-A record in the "Additional" (glue) section must not leak into
        // the result: only the address inside the "Answer" array is returned.
        let json = br#"{"Status":0,
            "Answer":[{"name":"x.co.uk","type":1,"data":"104.21.75.42"}],
            "Additional":[{"name":"ns1.example.","type":1,"data":"198.51.100.9"}]}"#;
        let ips = parse_doh_a_records(json);
        assert_eq!(ips, vec!["104.21.75.42".to_string()]);
    }

    #[test]
    fn parse_doh_a_records_multiple_a() {
        let json = br#"{"Answer":[
            {"type":1,"data":"1.1.1.1"},
            {"type":1,"data":"2.2.2.2"}
        ]}"#;
        let ips = parse_doh_a_records(json);
        assert_eq!(ips, vec!["1.1.1.1".to_string(), "2.2.2.2".to_string()]);
    }

    #[test]
    fn pick_preferred_ip_prefers_v4() {
        let ips = vec!["2001:db8::1".into(), "10.0.0.1".into()];
        assert_eq!(pick_preferred_ip(&ips).as_deref(), Some("10.0.0.1"));
    }
}
