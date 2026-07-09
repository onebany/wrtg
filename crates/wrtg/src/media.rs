//! Media CDN HTTP front helpers.

use crate::mtproto::{dc_alt_ips, dc_from_orig_dst};

pub fn is_blocked_media_cdn(ip: &str) -> bool {
    dc_alt_ips().get(ip).map(|e| e.is_media).unwrap_or(false)
}

pub fn media_http_host(dc: i32) -> String {
    let dc = if dc == 203 { 2 } else { dc };
    format!("kws{dc}-1.web.telegram.org")
}

pub fn regular_http_host(dc: i32) -> String {
    let dc = if dc == 203 { 2 } else { dc };
    format!("kws{dc}.web.telegram.org")
}

pub fn http_front_host(orig_ip: &str, orig_port: u16) -> String {
    if orig_port != 80 {
        return String::new();
    }
    let (dc, is_media) = match dc_from_orig_dst(orig_ip) {
        Some(v) => v,
        None => return String::new(),
    };
    if is_media {
        media_http_host(dc)
    } else {
        regular_http_host(dc)
    }
}

pub fn tls_front_host(orig_ip: &str, orig_port: u16) -> String {
    if orig_port != 443 {
        return String::new();
    }
    let (dc, is_media) = match dc_from_orig_dst(orig_ip) {
        Some(v) => v,
        None => return String::new(),
    };
    if is_media {
        media_http_host(dc)
    } else {
        regular_http_host(dc)
    }
}

pub fn rewrite_http_front_host(data: &[u8], orig_ip: &str, orig_port: u16) -> Vec<u8> {
    let host = http_front_host(orig_ip, orig_port);
    if host.is_empty() {
        return data.to_vec();
    }
    replace_http_host(data, &host).unwrap_or_else(|| data.to_vec())
}

fn replace_http_host(data: &[u8], new_host: &str) -> Option<Vec<u8>> {
    let lower: Vec<u8> = data.iter().map(|b| b.to_ascii_lowercase()).collect();
    let line_start = if let Some(i) = lower.windows(7).position(|w| w == b"\r\nhost:") {
        i + 2
    } else if let Some(i) = lower.windows(6).position(|w| w == b"\nhost:") {
        i + 1
    } else {
        return None;
    };
    let rest = &data[line_start..];
    let colon = rest.iter().position(|&b| b == b':')?;
    let mut val_start = colon + 1;
    while val_start < rest.len() && (rest[val_start] == b' ' || rest[val_start] == b'\t') {
        val_start += 1;
    }
    let mut line_end = val_start;
    while line_end < rest.len() && rest[line_end] != b'\r' && rest[line_end] != b'\n' {
        line_end += 1;
    }

    let mut out = Vec::new();
    out.extend_from_slice(&data[..line_start]);
    out.extend_from_slice(&rest[..=colon]);
    out.push(b' ');
    out.extend_from_slice(new_host.as_bytes());
    out.extend_from_slice(&rest[line_end..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mtproto::set_front_ip;
    use crate::tls_sni::parse_http_host;

    #[test]
    fn rewrite_http_front_host_media() {
        set_front_ip("149.154.167.220".into());
        let req = b"POST /api HTTP/1.1\r\nHost: 149.154.175.211:80\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nbody";
        let out = rewrite_http_front_host(req, "149.154.175.211", 80);
        assert_ne!(out, req);
        let host = parse_http_host(&out);
        assert_eq!(host.as_deref(), Some("kws1-1.web.telegram.org"));
    }

    #[test]
    fn rewrite_http_front_host_dc5_emoji_cdn() {
        // 91.108.56.155 is the DC5 animated-emoji CDN; :80 Host must become kws5-1.
        let req = b"POST /api HTTP/1.1\r\nHost: 91.108.56.155:80\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nbody";
        let out = rewrite_http_front_host(req, "91.108.56.155", 80);
        assert_ne!(out, req);
        let host = parse_http_host(&out);
        assert_eq!(host.as_deref(), Some("kws5-1.web.telegram.org"));
        assert!(is_blocked_media_cdn("91.108.56.155"));
    }

    #[test]
    fn rewrite_http_front_host_alt_dc() {
        let req = b"POST /api HTTP/1.1\r\nHost: 149.154.175.53:80\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nbody";
        let out = rewrite_http_front_host(req, "149.154.175.53", 80);
        let host = parse_http_host(&out);
        assert_eq!(host.as_deref(), Some("kws1.web.telegram.org"));
    }

    #[test]
    fn passthrough_targets_media_cdn() {
        use crate::tls_sni::passthrough_targets;
        set_front_ip("149.154.167.220".into());
        let targets = passthrough_targets("149.154.175.211", "", 80);
        assert!(!targets.is_empty());
        assert_eq!(targets[0], "149.154.167.220");
        assert!(!targets.iter().any(|ip| ip == "149.154.175.211"));
    }

    #[test]
    fn passthrough_targets_cdn_telesco() {
        use crate::tls_sni::passthrough_targets;
        set_front_ip("149.154.167.220".into());
        let targets = passthrough_targets("149.154.175.204", "cdn1.telesco.pe", 443);
        assert!(!targets.is_empty());
        assert_eq!(targets[0], "149.154.167.220");
    }
}
