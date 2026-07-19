//! Cloudflare-proxied WebSocket fallback (wss://kws{N}.<cf-domain>/apiws).

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::cf_proxy_cooldown::{
    cf_proxy_cooldown_remaining, clear_cf_proxy_429_cooldown, mark_cf_proxy_429_cooldown,
};
use crate::cf_proxy_doh::resolve_doh;
use crate::ws::{connect_ws_with_headers, is_ws_http_status, RawWebSocket, WsConnectError};

pub fn cf_proxy_ws_domain(cf_domain: &str, dc: i32, is_media: bool) -> String {
    let dc = if dc == 203 { 2 } else { dc };
    if is_media {
        format!("kws{dc}-1.{cf_domain}")
    } else {
        format!("kws{dc}.{cf_domain}")
    }
}

fn cf_proxy_parallel_limit() -> usize {
    static LIMIT: LazyLock<usize> = LazyLock::new(|| {
        std::env::var("WRTG_CFPROXY_PARALLEL")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(2)
    });
    *LIMIT
}

static CFPROXY_SEM: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(cf_proxy_parallel_limit())));

/// Connect via a Cloudflare-proxied domain (TLS to CF, CF forwards to Telegram).
/// On a transport failure (DNS/connect/timeout), resolves the base domain via
/// DoH and retries with IP + SNI.
pub async fn connect_cf_proxy_ws(
    cf_domain: &str,
    dc: i32,
    is_media: bool,
    connect_timeout: Duration,
) -> Result<RawWebSocket, WsConnectError> {
    let host = cf_proxy_ws_domain(cf_domain, dc, is_media);
    cf_connect_domain(cf_domain, &host, connect_timeout, &[]).await
}

async fn cf_connect_domain(
    dial_host: &str,
    sni_host: &str,
    connect_timeout: Duration,
    extra_headers: &[(&str, &str)],
) -> Result<RawWebSocket, WsConnectError> {
    match connect_ws_with_headers(
        dial_host,
        sni_host,
        "/apiws",
        connect_timeout,
        extra_headers,
    )
    .await
    {
        Ok(ws) => Ok(ws),
        Err(e) if is_ws_http_status(&e, 429) => Err(e),
        Err(host_err) => {
            // DoH retry only helps transport failures (DNS/connect/timeout).
            // A TLS failure or an HTTP response status won't change with a
            // different dial IP — don't retry those.
            if !matches!(host_err, WsConnectError::Io(_) | WsConnectError::Timeout) {
                return Err(host_err);
            }
            let resolved_ip = resolve_doh(dial_host).await.unwrap_or_default();
            if resolved_ip.is_empty() {
                log::debug!("CF proxy DoH {dial_host}: no result");
                return Err(host_err);
            }
            log::debug!("CF proxy DoH {dial_host} -> {resolved_ip}");
            connect_ws_with_headers(
                &resolved_ip,
                sni_host,
                "/apiws",
                connect_timeout,
                extra_headers,
            )
            .await
        }
    }
}

/// Try one CF proxy base domain (429 cooldown + parallel slot + DoH fallback).
pub async fn try_cf_proxy_domain(
    cf_domain: &str,
    dc: i32,
    is_media: bool,
    connect_timeout: Duration,
) -> Result<RawWebSocket, WsConnectError> {
    let remaining = cf_proxy_cooldown_remaining(cf_domain);
    if remaining > Duration::ZERO {
        log::debug!(
            "CF proxy skip {cf_domain}: 429 cooldown {:.0}s",
            remaining.as_secs_f64().ceil()
        );
        return Err(WsConnectError::Io(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "CF proxy domain in 429 cooldown",
        )));
    }

    let _permit = CFPROXY_SEM.clone().acquire_owned().await.map_err(|_| {
        WsConnectError::Io(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "CF proxy parallel slot unavailable",
        ))
    })?;

    match connect_cf_proxy_ws(cf_domain, dc, is_media, connect_timeout).await {
        Ok(ws) => {
            clear_cf_proxy_429_cooldown(cf_domain);
            Ok(ws)
        }
        Err(e) if is_ws_http_status(&e, 429) => {
            mark_cf_proxy_429_cooldown(cf_domain, &e);
            Err(e)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_proxy_ws_domain_media_suffix() {
        assert_eq!(cf_proxy_ws_domain("x.co.uk", 2, false), "kws2.x.co.uk");
        assert_eq!(cf_proxy_ws_domain("x.co.uk", 2, true), "kws2-1.x.co.uk");
    }
}
