//! TLS SNI / HTTP Host parsing for passthrough routing.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::OnceLock;

use crate::media::{http_front_host, is_blocked_media_cdn, tls_front_host};
use crate::mtproto::front_ip;

fn web_blocked_ips() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        HashSet::from([
            "149.154.167.99",
            "149.154.175.211",
            "149.154.171.255",
            "149.154.162.123",
            "149.154.175.204",
            "149.154.175.205",
        ])
    })
}

pub fn is_web_telegram_host(host: &str) -> bool {
    let host = host.trim().to_lowercase();
    if host.is_empty() {
        return false;
    }
    let host = host.split(':').next().unwrap_or(&host);
    host == "web.telegram.org" || host.ends_with(".web.telegram.org")
}

pub fn is_cdn_telegram_host(host: &str) -> bool {
    let host = host.trim().to_lowercase();
    if host.is_empty() {
        return false;
    }
    let host = host.split(':').next().unwrap_or(&host);
    if host == "telesco.pe" {
        return true;
    }
    host.ends_with(".telesco.pe") || (host.starts_with("cdn") && host.ends_with(".telegram.org"))
}

pub fn parse_tls_client_hello_sni(data: &[u8]) -> Option<String> {
    let mut off = 0usize;
    while off + 5 <= data.len() {
        if data[off] != 0x16 {
            off += 1;
            continue;
        }
        if data[off + 1] != 0x03 {
            off += 1;
            continue;
        }
        let rec_len = u16::from_be_bytes([data[off + 3], data[off + 4]]) as usize;
        if rec_len < 4 || off + 5 + rec_len > data.len() {
            break;
        }
        let payload = &data[off + 5..off + 5 + rec_len];
        if let Some(host) = sni_from_client_hello(payload) {
            return Some(host);
        }
        off += 5 + rec_len;
    }
    None
}

fn sni_from_client_hello(payload: &[u8]) -> Option<String> {
    if payload.len() < 4 || payload[0] != 0x01 {
        return None;
    }
    let hs_len = ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | payload[3] as usize;
    if hs_len + 4 > payload.len() {
        return None;
    }
    let body = &payload[4..4 + hs_len];
    if body.len() < 34 {
        return None;
    }
    let mut pos = 34usize; // version(2) + random(32)
    let sid_len = body[pos] as usize;
    pos += 1;
    if pos + sid_len > body.len() {
        return None;
    }
    pos += sid_len;
    if pos + 2 > body.len() {
        return None;
    }
    let cs_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;
    if pos + cs_len > body.len() {
        return None;
    }
    pos += cs_len;
    if pos >= body.len() {
        return None;
    }
    let comp_len = body[pos] as usize;
    pos += 1;
    if pos + comp_len > body.len() {
        return None;
    }
    pos += comp_len;
    if pos + 2 > body.len() {
        return None;
    }
    let ext_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;
    if pos + ext_len > body.len() {
        return None;
    }
    let ext_end = pos + ext_len;
    while pos + 4 <= ext_end {
        let ext_type = u16::from_be_bytes([body[pos], body[pos + 1]]);
        let ext_data_len = u16::from_be_bytes([body[pos + 2], body[pos + 3]]) as usize;
        pos += 4;
        if pos + ext_data_len > ext_end {
            return None;
        }
        if ext_type == 0x0000 {
            if let Some(host) = parse_sni_extension(&body[pos..pos + ext_data_len]) {
                return Some(host);
            }
        }
        pos += ext_data_len;
    }
    None
}

fn parse_sni_extension(data: &[u8]) -> Option<String> {
    if data.len() < 5 {
        return None;
    }
    let list_len = u16::from_be_bytes([data[0], data[1]]) as usize;
    if list_len + 2 > data.len() {
        return None;
    }
    let mut pos = 2usize;
    while pos + 3 <= 2 + list_len {
        let name_type = data[pos];
        let name_len = u16::from_be_bytes([data[pos + 1], data[pos + 2]]) as usize;
        pos += 3;
        if pos + name_len > data.len() {
            return None;
        }
        if name_type == 0 {
            return String::from_utf8(data[pos..pos + name_len].to_vec()).ok();
        }
        pos += name_len;
    }
    None
}

pub fn parse_http_host(data: &[u8]) -> Option<String> {
    const MAX: usize = 4096;
    let data = if data.len() > MAX { &data[..MAX] } else { data };
    let lower = String::from_utf8_lossy(data).to_lowercase();
    let i = lower.find("\r\nhost:")?;
    let rest = &data[i + 7..];
    let mut j = 0usize;
    while j < rest.len() && (rest[j] == b' ' || rest[j] == b'\t') {
        j += 1;
    }
    let start = j;
    while j < rest.len() && rest[j] != b'\r' && rest[j] != b'\n' {
        j += 1;
    }
    if j == start {
        return None;
    }
    let mut host = String::from_utf8_lossy(&rest[start..j]).trim().to_string();
    // Match Go net.SplitHostPort: strip :port before domain/IP checks.
    if let Some((h, port)) = host.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            host = h.to_string();
        }
    }
    if host.parse::<IpAddr>().is_ok() {
        return None;
    }
    Some(host)
}

pub fn passthrough_host(initial: &[u8]) -> String {
    if let Some(h) = parse_tls_client_hello_sni(initial) {
        return h;
    }
    parse_http_host(initial).unwrap_or_default()
}

pub fn passthrough_targets(orig_ip: &str, host: &str, orig_port: u16) -> Vec<String> {
    let front = front_ip();
    let mut targets = Vec::new();
    let mut add = |ip: &str| {
        if ip.is_empty() || targets.iter().any(|t| t == ip) {
            return;
        }
        targets.push(ip.to_string());
    };

    let web_host = is_web_telegram_host(host);
    let cdn_host = is_cdn_telegram_host(host);
    let blocked_orig = web_blocked_ips().contains(orig_ip);
    let media_cdn = is_blocked_media_cdn(orig_ip);
    let http_front = orig_port == 80 && !http_front_host(orig_ip, orig_port).is_empty();
    let tls_front = orig_port == 443 && !tls_front_host(orig_ip, orig_port).is_empty();

    if web_host || cdn_host || blocked_orig || media_cdn || http_front || tls_front {
        add(&front);
        if !orig_ip.is_empty() && !blocked_orig && !media_cdn && !http_front && !tls_front {
            add(orig_ip);
        }
    } else {
        if !orig_ip.is_empty() {
            add(orig_ip);
        }
        add(&front);
    }

    if !media_cdn && !http_front && !tls_front {
        for ip in [
            "149.154.167.51",
            "149.154.175.50",
            "149.154.175.100",
            "149.154.167.91",
            "149.154.171.5",
            "91.105.192.100",
        ] {
            add(ip);
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mtproto::set_front_ip;

    fn build_tls_client_hello_sni(sni: &str) -> Vec<u8> {
        let name_entry = {
            let mut e = vec![0x00, 0x00, sni.len() as u8];
            e.extend_from_slice(sni.as_bytes());
            e
        };
        let sni_ext = {
            let mut e = vec![(name_entry.len() >> 8) as u8, name_entry.len() as u8];
            e.extend_from_slice(&name_entry);
            e
        };

        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend(vec![0u8; 32]);
        body.push(0x00); // session id len
        body.extend_from_slice(&[0x00, 0x02, 0x00, 0x2f]); // cipher suites
        body.extend_from_slice(&[0x01, 0x00]); // compression
        let mut ext_block = vec![0x00, 0x00, (sni_ext.len() >> 8) as u8, sni_ext.len() as u8];
        ext_block.extend_from_slice(&sni_ext);
        body.push((ext_block.len() >> 8) as u8);
        body.push(ext_block.len() as u8);
        body.extend_from_slice(&ext_block);

        let mut hs = vec![0x01];
        hs.push((body.len() >> 16) as u8);
        hs.push((body.len() >> 8) as u8);
        hs.push(body.len() as u8);
        hs.extend_from_slice(&body);

        let mut rec = Vec::new();
        rec.extend_from_slice(&[0x16, 0x03, 0x01]);
        rec.push((hs.len() >> 8) as u8);
        rec.push(hs.len() as u8);
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn parse_tls_client_hello_sni_web() {
        let rec = build_tls_client_hello_sni("web.telegram.org");
        let host = parse_tls_client_hello_sni(&rec);
        assert_eq!(host.as_deref(), Some("web.telegram.org"));
    }

    #[test]
    fn is_web_telegram_host_cases() {
        assert!(is_web_telegram_host("web.telegram.org"));
        assert!(is_web_telegram_host("pluto.web.telegram.org"));
        assert!(is_web_telegram_host("kws1.web.telegram.org"));
        assert!(!is_web_telegram_host("telegram.org"));
        assert!(!is_web_telegram_host("evil.web.telegram.org.attacker.com"));
        assert!(!is_web_telegram_host(""));
    }

    #[test]
    fn passthrough_targets_web() {
        set_front_ip("149.154.167.220".into());
        let targets = passthrough_targets("149.154.167.99", "web.telegram.org", 443);
        assert!(!targets.is_empty());
        assert_eq!(targets[0], "149.154.167.220");
        assert!(!targets.iter().any(|ip| ip == "149.154.167.99"));
    }

    #[test]
    fn parse_http_host_get() {
        let req = b"GET / HTTP/1.1\r\nHost: pluto.web.telegram.org\r\n\r\n";
        let host = parse_http_host(req);
        assert_eq!(host.as_deref(), Some("pluto.web.telegram.org"));
    }

    #[test]
    fn parse_http_host_rejects_dc_ip() {
        let req = b"POST /api HTTP/1.1\r\nHost: 149.154.175.53:80\r\n\r\n";
        assert!(parse_http_host(req).is_none());
    }

    #[test]
    fn is_cdn_telegram_host_cases() {
        assert!(is_cdn_telegram_host("cdn1.telesco.pe"));
        assert!(is_cdn_telegram_host("cdn2.telegram.org"));
        assert!(is_cdn_telegram_host("telesco.pe"));
        assert!(!is_cdn_telegram_host("kws1.web.telegram.org"));
    }
}
