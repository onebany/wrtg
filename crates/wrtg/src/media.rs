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

/// Payload for local `FRONT_IP:80` passthrough. Regular DC MTProto-over-HTTP
/// keeps the client's `Host: <dc-ip>:80` — the front routes on that header.
/// Rewriting to `kws{N}.web.telegram.org` makes the front answer HTTP 302
/// (Location to core.telegram.org) and the emoji picker stays empty. Only
/// blocked media CDN IPs need `kws{N}-1` Host rewrite.
pub fn http_front_passthrough_payload(data: &[u8], orig_ip: &str, orig_port: u16) -> Vec<u8> {
    if orig_port != 80 || !needs_http_host_rewrite(orig_ip) {
        return data.to_vec();
    }
    rewrite_http_front_host(data, orig_ip, orig_port)
}

pub fn needs_http_host_rewrite(orig_ip: &str) -> bool {
    if is_blocked_media_cdn(orig_ip) {
        return true;
    }
    dc_from_orig_dst(orig_ip).is_some_and(|(_, is_media)| is_media)
}

/// Whether blind-relay should try CF Worker passthrough for this flow.
///
/// All MTProto-over-HTTP (`:80`) must use local `FRONT_IP` — the front routes on
/// `Host: <dc-ip>:80` for regular DCs and on `Host: kws{N}-1.web.telegram.org` for
/// media CDN. Tunnelling HTTP through the Worker to the real DC IP returns HTTP
/// 404; the session closes before front fallback runs (same failure as regular
/// DC before 0.5.14). Worker passthrough remains for TLS (:443) to blocked CDN IPs.
pub fn should_try_worker_passthrough(orig_port: u16) -> bool {
    orig_port != 80
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
    fn http_front_passthrough_keeps_dc_host_for_regular_dc() {
        set_front_ip("149.154.167.220".into());
        let req = b"POST /api HTTP/1.1\r\nHost: 149.154.167.51:80\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nbody";
        let out = http_front_passthrough_payload(req, "149.154.167.51", 80);
        // Payload is passed through untouched: the wire keeps `Host: <dc-ip>:80`.
        // parse_http_host intentionally rejects DC-IP hosts (front routing), so it
        // returns None here — the bytes, not that helper, carry the host.
        assert_eq!(out, req);
        assert_eq!(parse_http_host(&out), None);
    }

    #[test]
    fn http_front_passthrough_rewrites_media_cdn() {
        let req = b"POST /api HTTP/1.1\r\nHost: 91.108.56.155:80\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nbody";
        let out = http_front_passthrough_payload(req, "91.108.56.155", 80);
        assert_ne!(out, req);
        let host = parse_http_host(&out);
        assert_eq!(host.as_deref(), Some("kws5-1.web.telegram.org"));
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
    fn should_try_worker_passthrough_skips_http() {
        // MTProto-over-HTTP (:80) always goes to the local FRONT_IP.
        assert!(!should_try_worker_passthrough(80));
    }

    #[test]
    fn should_try_worker_passthrough_allows_tls() {
        // TLS (:443) may tunnel to blocked CDN IPs through the Worker.
        assert!(should_try_worker_passthrough(443));
        assert!(should_try_worker_passthrough(5222));
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
