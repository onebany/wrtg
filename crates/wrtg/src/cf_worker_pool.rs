//! Pre-established Cloudflare-Worker WebSocket pool per (DC, media).
//! Thin wiring over [`crate::conn_pool::Pool`].

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::time::timeout;

use crate::cf_balancer::worker_domains_for_dc;
use crate::conn_pool::{ConnectFuture, Key, Pool};
use crate::mtproto::{dc_default_ip, ws_target_ip};
use crate::ws::{connect_cf_worker_ws, RawWebSocket};

const DEFAULT_POOL_SIZE: usize = 2;
const MAX_POOL_SIZE: usize = 4;
const DEFAULT_POOL_TTL_SEC: u64 = 120;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const REFILL_INTERVAL: Duration = Duration::from_secs(45);

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

fn enabled() -> bool {
    !worker_domains_for_dc(0).is_empty()
}

/// Warm only the slots this router has actually used.
///
/// Seeding the full `5 DC × media` cross-product opened `size × 10` sockets —
/// 40 at `WRTG_CF_WORKER_POOL_SIZE=4` — and, recycled on the pool TTL, spent
/// roughly a third of the Cloudflare free-plan daily request quota before a
/// single client connected. An unseeded slot is not unreachable: the first
/// connection to it simply pays one cold connect and `schedule_refill` warms it
/// from there, so a fresh install with no learned map degrades to lazy filling.
fn seeds() -> Vec<Key> {
    crate::dc_learn::observed_slots()
}

fn dst_ip(dc: i32, orig_hint: &str) -> String {
    let target = ws_target_ip(dc, orig_hint);
    if !target.is_empty() {
        return target;
    }
    dc_default_ip(dc).unwrap_or("149.154.167.220").to_string()
}

fn connect(dc: i32, _is_media: bool, hint: String) -> ConnectFuture {
    Box::pin(async move {
        let workers = worker_domains_for_dc(dc);
        if workers.is_empty() {
            return None;
        }
        let dst = dst_ip(dc, &hint);
        // Rotate the starting worker each attempt so pooled connections spread
        // across the configured workers instead of all landing on the first.
        static RR: AtomicUsize = AtomicUsize::new(0);
        let start = RR.fetch_add(1, Ordering::Relaxed);
        for i in 0..workers.len() {
            let worker = &workers[(start + i) % workers.len()];
            match timeout(
                CONNECT_TIMEOUT,
                connect_cf_worker_ws(worker, &dst, dc, CONNECT_TIMEOUT),
            )
            .await
            {
                Ok(Ok(ws)) => return Some((ws, worker.clone())),
                Ok(Err(e)) => log::debug!("cf worker pool: DC{dc} {worker} failed: {e}"),
                Err(_) => log::debug!("cf worker pool: DC{dc} {worker} timeout"),
            }
        }
        None
    })
}

// CF Worker pool serves both media and non-media DCs.
static POOL: Pool = Pool::new(
    connect,
    pool_size,
    pool_ttl,
    enabled,
    seeds,
    true, // serves_media
    REFILL_INTERVAL,
    "cf worker pool",
);

pub struct PooledCfWs {
    pub ws: RawWebSocket,
    pub worker: String,
}

pub async fn acquire(dc: i32, is_media: bool, _orig_hint: &str) -> Option<PooledCfWs> {
    POOL.acquire(dc, is_media).await.map(|r| PooledCfWs {
        ws: r.ws,
        worker: r.label,
    })
}

pub fn schedule_refill(dc: i32, is_media: bool, orig_hint: String) {
    POOL.schedule_refill(dc, is_media, orig_hint);
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

    const _: () = assert!(MAX_POOL_SIZE <= 4);

    #[test]
    fn pool_size_within_bounds() {
        assert!((1..=MAX_POOL_SIZE).contains(&pool_size()));
    }

    #[test]
    fn seeds_are_reachable_dc_slots() {
        for (dc, _) in seeds() {
            assert!(crate::mtproto::valid_dc(dc), "DC{dc} is not a valid DC");
            assert!(
                dc_default_ip(dc).is_some(),
                "DC{dc} has no default IP, so the connector cannot reach it"
            );
        }
    }

    #[test]
    fn seeds_are_deduplicated() {
        let s = seeds();
        let mut sorted = s.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(s.len(), sorted.len(), "a slot would be warmed twice");
    }
}
