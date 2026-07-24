//! Pre-established direct-WebSocket pool per (DC, media) for faster bridge setup.
//! Thin wiring over [`crate::conn_pool::Pool`].

use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::timeout;

use crate::conn_pool::{ConnectFuture, Key, Pool};
use crate::mtproto::{dc_front_ip, ws_domains, ws_target_ip};
use crate::ws::{connect_ws, RawWebSocket};

const DEFAULT_POOL_SIZE: usize = 2;
const MAX_POOL_SIZE: usize = 8;
const DEFAULT_POOL_TTL_SEC: u64 = 120;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REFILL_INTERVAL: Duration = Duration::from_secs(30);

fn pool_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        std::env::var("WRTG_WS_POOL_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_POOL_SIZE)
            .clamp(1, MAX_POOL_SIZE)
    })
}

fn pool_ttl() -> Duration {
    static TTL: OnceLock<Duration> = OnceLock::new();
    *TTL.get_or_init(|| {
        let secs = std::env::var("WRTG_WS_POOL_TTL_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_POOL_TTL_SEC);
        Duration::from_secs(secs)
    })
}

/// Non-media direct WS only makes sense for DCs that have a front target.
fn enabled() -> bool {
    (1..=5).any(|dc| !dc_front_ip(dc).is_empty())
}

/// Warm only the DCs that actually have a front target. Seeding all of 1..=5
/// meant that with the default `WRTG_FRONT_DCS=2,4` the DC1/3/5 attempts each
/// burned the full connect timeout — a 60 s warmup to end up with 4 usable
/// connections.
fn seeds() -> Vec<Key> {
    (1..=5)
        .filter(|&dc| !dc_front_ip(dc).is_empty())
        .map(|dc| (dc, false))
        .collect()
}

fn connect(dc: i32, is_media: bool, hint: String) -> ConnectFuture {
    Box::pin(async move {
        let target = if hint.is_empty() {
            ws_target_ip(dc, "")
        } else {
            hint
        };
        if target.is_empty() {
            return None;
        }
        for domain in ws_domains(dc, is_media) {
            match timeout(
                CONNECT_TIMEOUT,
                connect_ws(&target, &domain, "/apiws", CONNECT_TIMEOUT),
            )
            .await
            {
                Ok(Ok(ws)) => return Some((ws, domain)),
                Ok(Err(e)) => log::debug!("ws pool: DC{dc} {domain} failed: {e}"),
                Err(_) => log::debug!("ws pool: DC{dc} {domain} timeout"),
            }
        }
        None
    })
}

// Direct WS is served non-media only (media goes straight to the fallback ladder).
static POOL: Pool = Pool::new(
    connect,
    pool_size,
    pool_ttl,
    enabled,
    seeds,
    false, // serves_media
    REFILL_INTERVAL,
    "ws pool",
);

pub struct PooledWs {
    pub ws: RawWebSocket,
    pub domain: String,
}

pub async fn acquire(dc: i32, is_media: bool) -> Option<PooledWs> {
    POOL.acquire(dc, is_media).await.map(|r| PooledWs {
        ws: r.ws,
        domain: r.label,
    })
}

pub fn schedule_refill(dc: i32, is_media: bool, target_ip: String) {
    POOL.schedule_refill(dc, is_media, target_ip);
}

pub fn start_refill_task() {
    POOL.start_refill_task();
}

pub fn warmup_pools() {
    POOL.warmup();
}

pub async fn depths() -> Vec<(i32, bool, usize)> {
    POOL.depths().await
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = assert!(MAX_POOL_SIZE <= 8);
    const _: () = assert!(DEFAULT_POOL_SIZE >= 1);

    #[test]
    fn pool_size_within_bounds() {
        let sz = pool_size();
        assert!((1..=MAX_POOL_SIZE).contains(&sz));
    }

    #[test]
    fn seeds_are_non_media_and_have_a_front() {
        for (dc, is_media) in seeds() {
            assert!(!is_media, "direct WS pool never serves media slots");
            assert!(
                !dc_front_ip(dc).is_empty(),
                "DC{dc} was seeded without a front target"
            );
        }
    }

    #[test]
    fn seeds_never_exceed_the_dc_range() {
        assert!(seeds().len() <= 5);
    }
}
