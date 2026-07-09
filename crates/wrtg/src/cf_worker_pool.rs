//! Pre-established Cloudflare Worker WebSocket pool per DC.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio::time::{interval, timeout};

use crate::cf_balancer::worker_domains_for_dc;
use crate::mtproto::{cf_worker_domain, dc_default_ip, ws_target_ip};
use crate::ws::{connect_cf_worker_ws, RawWebSocket};

type PoolKey = (i32, bool, String);

struct PooledConn {
    ws: RawWebSocket,
    created: Instant,
    worker: String,
}

static POOLS: LazyLock<Mutex<HashMap<PoolKey, Vec<PooledConn>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const DEFAULT_POOL_SIZE: usize = 2;
const MAX_POOL_SIZE: usize = 4;
const DEFAULT_POOL_TTL_SEC: u64 = 120;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const REFILL_INTERVAL: Duration = Duration::from_secs(45);

pub struct PooledCfWs {
    pub ws: RawWebSocket,
    pub worker: String,
}

fn pool_size() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        std::env::var("WRTG_CF_WORKER_POOL_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_POOL_SIZE)
            .clamp(1, MAX_POOL_SIZE)
    })
}

fn pool_ttl() -> Duration {
    static TTL: OnceLock<Duration> = OnceLock::new();
    *TTL.get_or_init(|| {
        let secs = std::env::var("WRTG_CF_WORKER_POOL_TTL_SEC")
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
    conns.retain(|c| !is_expired(c.created));
}

fn dst_ip(dc: i32, orig_hint: &str) -> String {
    let target = ws_target_ip(dc, orig_hint);
    if !target.is_empty() {
        return target;
    }
    dc_default_ip(dc).unwrap_or("149.154.167.220").to_string()
}

async fn connect_one(dc: i32, worker: &str, dst: &str) -> Option<PooledConn> {
    match timeout(
        CONNECT_TIMEOUT,
        connect_cf_worker_ws(worker, dst, dc, CONNECT_TIMEOUT),
    )
    .await
    {
        Ok(Ok(ws)) => Some(PooledConn {
            ws,
            created: Instant::now(),
            worker: worker.to_string(),
        }),
        Ok(Err(e)) => {
            log::debug!("cf worker pool: DC{dc} {worker} failed: {e}");
            None
        }
        Err(_) => {
            log::debug!("cf worker pool: DC{dc} {worker} timeout");
            None
        }
    }
}

async fn fill_pool(key: PoolKey, dc: i32, dst: &str, count: usize) {
    let (dc_k, is_media, _) = key;
    for worker in worker_domains_for_dc(dc) {
        for _ in 0..count {
            if let Some(conn) = connect_one(dc, &worker, dst).await {
                let mut pools = POOLS.lock().await;
                let entry = pools.entry((dc_k, is_media, worker.clone())).or_default();
                prune_expired(entry);
                if entry.len() >= pool_size() {
                    return;
                }
                entry.push(conn);
            }
        }
    }
}

pub async fn acquire(dc: i32, is_media: bool, _orig_hint: &str) -> Option<PooledCfWs> {
    for worker in worker_domains_for_dc(dc) {
        let key = (dc, is_media, worker.clone());
        let mut pools = POOLS.lock().await;
        if let Some(entry) = pools.get_mut(&key) {
            prune_expired(entry);
            if let Some(conn) = entry.pop() {
                log::debug!(
                    "cf worker pool: acquired DC{dc}{} via {} ({} left)",
                    if is_media { "m" } else { "" },
                    conn.worker,
                    entry.len()
                );
                return Some(PooledCfWs {
                    ws: conn.ws,
                    worker: conn.worker,
                });
            }
        }
    }
    None
}

pub fn schedule_refill(dc: i32, is_media: bool, orig_hint: String) {
    tokio::spawn(async move {
        let dst = dst_ip(dc, &orig_hint);
        for worker in worker_domains_for_dc(dc) {
            let key = (dc, is_media, worker);
            let pools = POOLS.lock().await;
            let need = pool_size().saturating_sub(
                pools
                    .get(&key)
                    .map(|v| v.len())
                    .unwrap_or(0),
            );
            drop(pools);
            if need > 0 {
                fill_pool(key, dc, &dst, need).await;
            }
        }
    });
}

pub async fn reset_pools() {
    POOLS.lock().await.clear();
}

pub fn start_refill_task() {
    tokio::spawn(async move {
        let mut tick = interval(REFILL_INTERVAL);
        tick.tick().await;
        loop {
            tick.tick().await;
            let workers = worker_domains_for_dc(0);
            if workers.is_empty() && cf_worker_domain().is_empty() {
                continue;
            }
            for dc in 1..=5 {
                for is_media in [false, true] {
                    schedule_refill(dc, is_media, String::new());
                }
            }
        }
    });
}

pub fn warmup_pools() {
    let workers = worker_domains_for_dc(1);
    if workers.is_empty() {
        log::debug!("cf worker pool: skip warmup (no CF_WORKER_DOMAIN)");
        return;
    }
    log::info!(
        "cf worker pool: warming up DC1-5 (size={}, ttl={}s)",
        pool_size(),
        pool_ttl().as_secs()
    );
    tokio::spawn(async move {
        for dc in 1..=5 {
            for is_media in [false, true] {
                let dst = dst_ip(dc, "");
                for worker in worker_domains_for_dc(dc) {
                    fill_pool((dc, is_media, worker), dc, &dst, pool_size()).await;
                }
            }
        }
        let pools = POOLS.lock().await;
        let total: usize = pools.values().map(|v| v.len()).sum();
        log::info!("cf worker pool: warmup done ({total} connection(s) ready)");
    });
}
