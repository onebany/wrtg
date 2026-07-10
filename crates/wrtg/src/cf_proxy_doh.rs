//! DNS-over-HTTPS fallback when hostname dial fails (Cloudflare / Google / Quad9 / AdGuard race).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

const DOH_ENDPOINTS: &[(&str, &str)] = &[
    ("cloudflare-dns.com", "/dns-query"),
    ("dns.google", "/dns-query"),
    ("dns.quad9.net", "/dns-query"),
    ("dns.adguard-dns.com", "/dns-query"),
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

fn parse_doh_a_records(body: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(body);
    let mut out = Vec::new();
    for chunk in text.split("\"Answer\"").skip(1) {
        for part in chunk.split("\"type\":1") {
            if let Some(data) = part.split("\"data\":\"").nth(1) {
                if let Some(ip) = data.split('"').next() {
                    if !ip.is_empty() {
                        out.push(ip.to_string());
                    }
                }
            }
        }
    }
    out
}

async fn doh_query(host: &str, path: &str, domain: &str) -> Option<String> {
    let path = format!("{path}?name={domain}&type=A");
    let body = timeout(
        DOH_QUERY_TIMEOUT,
        https_get_json(host, &path, DOH_CONNECT_TIMEOUT),
    )
    .await
    .ok()
    .and_then(|r| r.ok())?;
    let ips = parse_doh_a_records(&body);
    pick_preferred_ip(&ips)
}

async fn https_get_json(
    host: &str,
    path: &str,
    connect_timeout: Duration,
) -> std::io::Result<Vec<u8>> {
    let addr = format!("{host}:443");
    let tcp = timeout(connect_timeout, TcpStream::connect(&addr)).await??;
    crate::sockopt::tune_tcp(&tcp);

    let connector = crate::tls::connector();
    let server_name = ServerName::try_from(host.to_string()).map_err(std::io::Error::other)?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp)).await??;

    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Accept: application/dns-json\r\n\
         User-Agent: wrtg\r\n\
         Connection: close\r\n\
         \r\n"
    );
    timeout(connect_timeout, stream.write_all(req.as_bytes())).await??;

    let mut buf = Vec::new();
    let mut limited = stream.take((MAX_DOH_BODY + 1) as u64);
    timeout(connect_timeout, limited.read_to_end(&mut buf)).await??;
    if buf.len() > MAX_DOH_BODY {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "DoH response too large",
        ));
    }

    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no HTTP headers"))?;
    let status = String::from_utf8_lossy(&buf[..header_end.min(32)]);
    if !status.contains(" 200 ") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("DoH HTTP error: {}", status.lines().next().unwrap_or("")),
        ));
    }
    Ok(buf[header_end + 4..].to_vec())
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

    for (host, path) in DOH_ENDPOINTS {
        let tx = tx.clone();
        let domain = domain.to_string();
        let host = host.to_string();
        let path = path.to_string();
        tasks.push(tokio::spawn(async move {
            if let Some(ip) = doh_query(&host, &path, &domain).await {
                let _ = tx.send(Some(ip)).await;
            } else {
                let _ = tx.send(None).await;
            }
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
    fn pick_preferred_ip_prefers_v4() {
        let ips = vec!["2001:db8::1".into(), "10.0.0.1".into()];
        assert_eq!(pick_preferred_ip(&ips).as_deref(), Some("10.0.0.1"));
    }
}
