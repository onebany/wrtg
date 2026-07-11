//! Generic pre-established WebSocket pool keyed by `(dc, is_media)`.
//!
//! Both the direct-WS pool (`ws_pool`) and the Cloudflare-Worker pool
//! (`cf_worker_pool`) are just this `Pool` wired to a different connector and a
//! bit of config — they used to be two ~90%-identical copies. A `Pool` lives in
//! a `static` (const `new`, lazily-built map) and is parameterized by plain
//! function pointers, so no generics/trait objects are needed.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio::time::interval;

use crate::ws::RawWebSocket;

type Key = (i32, bool);

/// A connector attempt: establish one connection for `(dc, is_media)` toward an
/// optional `hint` (an orig-IP / target hint; empty = derive the default), and
/// return the socket plus a label (domain or worker) for diagnostics.
pub type ConnectFuture = Pin<Box<dyn Future<Output = Option<(RawWebSocket, String)>> + Send>>;
pub type ConnectFn = fn(dc: i32, is_media: bool, hint: String) -> ConnectFuture;

struct Pooled {
    ws: RawWebSocket,
    created: Instant,
    label: String,
}

/// A ready connection handed back from the pool.
pub struct Ready {
    pub ws: RawWebSocket,
    pub label: String,
}

pub struct Pool {
    conns: LazyLock<Mutex<HashMap<Key, Vec<Pooled>>>>,
    connect: ConnectFn,
    size: fn() -> usize,
    ttl: fn() -> Duration,
    /// Whether the pool currently has any backend configured (gates warmup/refill).
    enabled: fn() -> bool,
    /// Which `is_media` variants this pool warms/serves (`[false]` or `[false, true]`).
    media_variants: &'static [bool],
    refill_interval: Duration,
    name: &'static str,
}

fn new_map() -> Mutex<HashMap<Key, Vec<Pooled>>> {
    Mutex::new(HashMap::new())
}

fn media_tag(is_media: bool) -> &'static str {
    if is_media {
        "m"
    } else {
        ""
    }
}

impl Pool {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        connect: ConnectFn,
        size: fn() -> usize,
        ttl: fn() -> Duration,
        enabled: fn() -> bool,
        media_variants: &'static [bool],
        refill_interval: Duration,
        name: &'static str,
    ) -> Self {
        Self {
            conns: LazyLock::new(new_map),
            connect,
            size,
            ttl,
            enabled,
            media_variants,
            refill_interval,
            name,
        }
    }

    fn serves_media(&self) -> bool {
        self.media_variants.contains(&true)
    }

    fn prune(&self, conns: &mut Vec<Pooled>) {
        let ttl = (self.ttl)();
        let before = conns.len();
        conns.retain(|c| c.created.elapsed() <= ttl);
        let dropped = before - conns.len();
        if dropped > 0 {
            log::debug!("{}: dropped {dropped} expired connection(s)", self.name);
        }
    }

    /// Take a ready connection, if any.
    pub async fn acquire(&self, dc: i32, is_media: bool) -> Option<Ready> {
        if is_media && !self.serves_media() {
            return None;
        }
        let mut pools = self.conns.lock().await;
        let entry = pools.get_mut(&(dc, is_media))?;
        self.prune(entry);
        let conn = entry.pop()?;
        log::debug!(
            "{}: acquired DC{dc}{} via {} ({} left)",
            self.name,
            media_tag(is_media),
            conn.label,
            entry.len()
        );
        Some(Ready {
            ws: conn.ws,
            label: conn.label,
        })
    }

    async fn fill(&self, dc: i32, is_media: bool, hint: &str, count: usize) {
        let size = (self.size)();
        for _ in 0..count {
            let Some((ws, label)) = (self.connect)(dc, is_media, hint.to_string()).await else {
                continue;
            };
            let mut pools = self.conns.lock().await;
            let entry = pools.entry((dc, is_media)).or_default();
            self.prune(entry);
            if entry.len() >= size {
                // Raced past capacity — close the extra rather than leaking it.
                drop(pools);
                let mut ws = ws;
                ws.close().await;
                return;
            }
            log::debug!(
                "{}: filled DC{dc}{} via {label}",
                self.name,
                media_tag(is_media)
            );
            entry.push(Pooled {
                ws,
                created: Instant::now(),
                label,
            });
        }
    }

    async fn refill_one(&self, dc: i32, is_media: bool, hint: &str) {
        let have = self
            .conns
            .lock()
            .await
            .get(&(dc, is_media))
            .map_or(0, Vec::len);
        let need = (self.size)().saturating_sub(have);
        if need > 0 {
            self.fill(dc, is_media, hint, need).await;
        }
    }

    /// Schedule a one-off background top-up for one slot.
    pub fn schedule_refill(&'static self, dc: i32, is_media: bool, hint: String) {
        if is_media && !self.serves_media() {
            return;
        }
        tokio::spawn(async move {
            self.refill_one(dc, is_media, &hint).await;
        });
    }

    /// Background task: periodically top up every served slot.
    pub fn start_refill_task(&'static self) {
        tokio::spawn(async move {
            let mut tick = interval(self.refill_interval);
            tick.tick().await;
            loop {
                tick.tick().await;
                if !(self.enabled)() {
                    continue;
                }
                for dc in 1..=5 {
                    for &is_media in self.media_variants {
                        self.refill_one(dc, is_media, "").await;
                    }
                }
            }
        });
    }

    /// Pre-warm all served slots.
    pub fn warmup(&'static self) {
        if !(self.enabled)() {
            log::debug!("{}: skip warmup (no backend configured)", self.name);
            return;
        }
        log::info!(
            "{}: warming up (size={}, ttl={}s)",
            self.name,
            (self.size)(),
            (self.ttl)().as_secs()
        );
        tokio::spawn(async move {
            for dc in 1..=5 {
                for &is_media in self.media_variants {
                    self.fill(dc, is_media, "", (self.size)()).await;
                }
            }
            let pools = self.conns.lock().await;
            let total: usize = pools.values().map(Vec::len).sum();
            log::info!("{}: warmup done ({total} connection(s) ready)", self.name);
        });
    }
}
