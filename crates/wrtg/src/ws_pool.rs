//! Pre-established WebSocket pool per (DC, media) for faster bridge setup.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio::time::{interval, timeout};

use crate::mtproto::{dc_front_ip, ws_domains, ws_target_ip};
use crate::ws::{connect_ws, RawWebSocket};

type PoolKey = (i32, bool);

struct PooledConn {
    ws: RawWebSocket,
    created: Instant,
    domain: String,
}

static POOLS: LazyLock<Mutex<HashMap<PoolKey, Vec<PooledConn>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const DEFAULT_POOL_SIZE: usize = 2;
const MAX_POOL_SIZE: usize = 8;
const DEFAULT_POOL_TTL_SEC: u64 = 120;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REFILL_INTERVAL: Duration = Duration::from_secs(30);

pub struct PooledWs {
    pub ws: RawWebSocket,
    pub domain: String,
}

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

fn is_expired(created: Instant) -> bool {
    created.elapsed() > pool_ttl()
}

fn prune_expired(conns: &mut Vec<PooledConn>) {
    let before = conns.len();
    conns.retain(|c| !is_expired(c.created));
    let dropped = before.saturating_sub(conns.len());
    if dropped > 0 {
        log::debug!("ws pool: dropped {dropped} expired connection(s)");
    }
}

async fn connect_one(dc: i32, is_media: bool, target_ip: &str) -> Option<PooledConn> {
    for domain in ws_domains(dc, is_media) {
        match timeout(
            CONNECT_TIMEOUT,
            connect_ws(target_ip, &domain, "/apiws", CONNECT_TIMEOUT),
        )
        .await
        {
            Ok(Ok(ws)) => {
                return Some(PooledConn {
                    ws,
                    created: Instant::now(),
                    domain,
                });
            }
            Ok(Err(e)) => {
                log::debug!("ws pool: DC{dc} {domain} failed: {e}");
            }
            Err(_) => {
                log::debug!("ws pool: DC{dc} {domain} timeout");
            }
        }
    }
    None
}

async fn fill_pool(key: PoolKey, target_ip: &str, count: usize) {
    let (dc, is_media) = key;
    for _ in 0..count {
        if let Some(conn) = connect_one(dc, is_media, target_ip).await {
            let domain = conn.domain.clone();
            let mut pools = POOLS.lock().await;
            let entry = pools.entry(key).or_default();
            prune_expired(entry);
            if entry.len() >= pool_size() {
                return;
            }
            log::debug!(
                "ws pool: filled DC{dc}{} via {domain}",
                if is_media { "m" } else { "" }
            );
            entry.push(conn);
        }
    }
}

/// Take a ready connection from the pool, if any.
pub async fn acquire(dc: i32, is_media: bool) -> Option<PooledWs> {
    if is_media {
        return None;
    }
    let key = (dc, is_media);
    let mut pools = POOLS.lock().await;
    let entry = pools.get_mut(&key)?;
    prune_expired(entry);
    let conn = entry.pop()?;
    log::debug!(
        "ws pool: acquired DC{dc}{} via {} ({} left)",
        if is_media { "m" } else { "" },
        conn.domain,
        entry.len()
    );
    Some(PooledWs {
        ws: conn.ws,
        domain: conn.domain,
    })
}

/// Schedule background refill for one (DC, media) slot.
pub fn schedule_refill(dc: i32, is_media: bool, target_ip: String) {
    if is_media {
        return;
    }
    tokio::spawn(async move {
        refill_one((dc, is_media), &target_ip).await;
    });
}

async fn refill_one(key: PoolKey, target_ip: &str) {
    let pools = POOLS.lock().await;
    let need = pool_size().saturating_sub(pools.get(&key).map(|v| v.len()).unwrap_or(0));
    drop(pools);
    if need > 0 {
        fill_pool(key, target_ip, need).await;
    }
}

async fn refill_all() {
    for dc in 1..=5 {
        if dc_front_ip(dc).is_empty() {
            continue;
        }
        let target = ws_target_ip(dc, "");
        if target.is_empty() {
            continue;
        }
        refill_one((dc, false), &target).await;
    }
}

/// Background task: periodically top up pools.
pub fn start_refill_task() {
    tokio::spawn(async move {
        let mut tick = interval(REFILL_INTERVAL);
        tick.tick().await;
        loop {
            tick.tick().await;
            refill_all().await;
        }
    });
}

/// Pre-warm non-media pools for DCs that have a usable front target.
pub fn warmup_pools() {
    log::info!(
        "ws pool: warming non-media fronted DCs (size={}, ttl={}s)",
        pool_size(),
        pool_ttl().as_secs()
    );
    tokio::spawn(async move {
        for dc in 1..=5 {
            if dc_front_ip(dc).is_empty() {
                continue;
            }
            let target = ws_target_ip(dc, "");
            if target.is_empty() {
                continue;
            }
            fill_pool((dc, false), &target, pool_size()).await;
        }
        let pools = POOLS.lock().await;
        let total: usize = pools.values().map(|v| v.len()).sum();
        log::info!("ws pool: warmup done ({total} connection(s) ready)");
    });
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
}
