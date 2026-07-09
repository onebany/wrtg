//! Cloudflare-proxied WebSocket fallback (wss://kws{N}.<cf-domain>/apiws).

use std::time::Duration;

use crate::mtproto::ws_domains;
use crate::ws::{connect_ws, RawWebSocket};

pub fn cf_proxy_ws_domain(cf_domain: &str, dc: i32, is_media: bool) -> String {
    let dc = if dc == 203 { 2 } else { dc };
    if is_media {
        format!("kws{dc}-1.{cf_domain}")
    } else {
        format!("kws{dc}.{cf_domain}")
    }
}

/// Connect via a Cloudflare-proxied domain (TLS to CF, CF forwards to Telegram).
pub async fn connect_cf_proxy_ws(
    cf_domain: &str,
    dc: i32,
    is_media: bool,
    connect_timeout: Duration,
) -> std::io::Result<RawWebSocket> {
    let host = cf_proxy_ws_domain(cf_domain, dc, is_media);
    connect_ws(cf_domain, &host, "/apiws", connect_timeout).await
}

pub fn cf_proxy_domains_for_dc(dc: i32, is_media: bool, cf_domain: &str) -> Vec<String> {
    if !cf_domain.is_empty() {
        return vec![cf_proxy_ws_domain(cf_domain, dc, is_media)];
    }
    ws_domains(dc, is_media)
}
