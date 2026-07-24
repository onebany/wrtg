//! Generic pre-established WebSocket pool keyed by `(dc, is_media)`.
//!
//! Both the direct-WS pool (`ws_pool`) and the Cloudflare-Worker pool
//! (`cf_worker_pool`) are just this `Pool` wired to a different connector and a
//! bit of config — they used to be two ~90%-identical copies. A `Pool` lives in
//! a `static` (const `new`, lazily-built map) and is parameterized by plain
//! function pointers, so no generics/trait objects are needed.
//!
//! **Slots are demand-driven.** A pool warms only the `(dc, is_media)` slots its
//! `seeds` function nominates, and the background refill tops up only slots
//! acquired within `SLOT_IDLE_AFTER`. Warming the whole `5 DC × media`
//! cross-product meant a CF-Worker pool of size 4 held 40 open WebSockets and —
//! with a 120 s TTL recycled every 45 s — burned ~29 k Worker requests a day on
//! a completely idle router, against a 100 k/day free-plan quota.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio::time::{interval, MissedTickBehavior};

use crate::ws::RawWebSocket;

/// A pool slot: `(dc, is_media)`.
pub type Key = (i32, bool);

/// Stop refilling a slot that has not been acquired for this long. An idle
/// router then lets its pooled connections expire and stops reconnecting
/// altogether; the next real connection re-arms the slot via `schedule_refill`.
const SLOT_IDLE_AFTER: Duration = Duration::from_secs(600);

/// A connector attempt: establish one connection for `(dc, is_media)` toward an
/// optional `hint` (an orig-IP / target hint; empty = derive the default), and
/// return the socket plus a label (domain or worker) for diagnostics.
pub type ConnectFuture = Pin<Box<dyn Future<Output = Option<(RawWebSocket, String)>> + Send>>;
pub type ConnectFn = fn(dc: i32, is_media: bool, hint: String) -> ConnectFuture;
/// Which `(dc, is_media)` slots to pre-warm. Evaluated after config load, so it
/// can consult the front-DC scope / learned DC map instead of guessing.
pub type SeedFn = fn() -> Vec<Key>;

struct Pooled {
    ws: RawWebSocket,
    created: Instant,
    label: String,
}

/// One `(dc, is_media)` slot: ready connections plus when it was last acquired.
struct Slot {
    conns: Vec<Pooled>,
    last_used: Instant,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            conns: Vec::new(),
            last_used: Instant::now(),
        }
    }
}

/// Split out entries older than `ttl`, leaving the fresh ones in `v`.
///
/// Separate from `Pool` (and generic over the item) so the expiry rule is
/// testable without standing up a live TLS socket.
fn take_expired<T>(v: &mut Vec<T>, ttl: Duration, stamp: fn(&T) -> Instant) -> Vec<T> {
    let (fresh, expired) = std::mem::take(v)
        .into_iter()
        .partition(|e| stamp(e).elapsed() <= ttl);
    *v = fresh;
    expired
}

fn created_at(p: &Pooled) -> Instant {
    p.created
}

/// A ready connection handed back from the pool.
pub struct Ready {
    pub ws: RawWebSocket,
    pub label: String,
}

pub struct Pool {
    slots: LazyLock<Mutex<HashMap<Key, Slot>>>,
    connect: ConnectFn,
    size: fn() -> usize,
    ttl: fn() -> Duration,
    /// Whether the pool currently has any backend configured (gates warmup/refill).
    enabled: fn() -> bool,
    /// Slots worth pre-warming at startup.
    seeds: SeedFn,
    /// Whether this pool serves media slots at all.
    serves_media: bool,
    refill_interval: Duration,
    name: &'static str,
}

fn new_map() -> Mutex<HashMap<Key, Slot>> {
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
        seeds: SeedFn,
        serves_media: bool,
        refill_interval: Duration,
        name: &'static str,
    ) -> Self {
        Self {
            slots: LazyLock::new(new_map),
            connect,
            size,
            ttl,
            enabled,
            seeds,
            serves_media,
            refill_interval,
            name,
        }
    }

    /// Close expired connections so the peer sees a WS close frame instead of a
    /// bare TCP drop (a Cloudflare Worker isolate otherwise lingers to its own
    /// timeout, holding quota).
    async fn close_expired(&self, expired: Vec<Pooled>) {
        if expired.is_empty() {
            return;
        }
        log::debug!(
            "{}: dropped {} expired connection(s)",
            self.name,
            expired.len()
        );
        for mut c in expired {
            c.ws.close().await;
        }
    }

    /// Take a ready connection, if any. Marks the slot in-use so the refill task
    /// keeps it warm.
    pub async fn acquire(&self, dc: i32, is_media: bool) -> Option<Ready> {
        if is_media && !self.serves_media {
            return None;
        }
        let ttl = (self.ttl)();
        let (conn, left, expired) = {
            let mut slots = self.slots.lock().await;
            let slot = slots.get_mut(&(dc, is_media))?;
            slot.last_used = Instant::now();
            let expired = take_expired(&mut slot.conns, ttl, created_at);
            let conn = slot.conns.pop();
            (conn, slot.conns.len(), expired)
        };
        self.close_expired(expired).await;
        let conn = conn?;
        log::debug!(
            "{}: acquired DC{dc}{} via {} ({left} left)",
            self.name,
            media_tag(is_media),
            conn.label
        );
        Some(Ready {
            ws: conn.ws,
            label: conn.label,
        })
    }

    async fn fill(&self, dc: i32, is_media: bool, hint: &str, count: usize) {
        let size = (self.size)();
        let ttl = (self.ttl)();
        for _ in 0..count {
            let Some((ws, label)) = (self.connect)(dc, is_media, hint.to_string()).await else {
                continue;
            };
            let (spare, expired) = {
                let mut slots = self.slots.lock().await;
                let slot = slots.entry((dc, is_media)).or_default();
                let expired = take_expired(&mut slot.conns, ttl, created_at);
                if slot.conns.len() >= size {
                    (Some(ws), expired)
                } else {
                    slot.conns.push(Pooled {
                        ws,
                        created: Instant::now(),
                        label: label.clone(),
                    });
                    (None, expired)
                }
            };
            self.close_expired(expired).await;
            if let Some(mut ws) = spare {
                // Raced past capacity — close the extra rather than leak it, and
                // stop: further attempts would race the same way.
                ws.close().await;
                return;
            }
            log::debug!(
                "{}: filled DC{dc}{} via {label}",
                self.name,
                media_tag(is_media)
            );
        }
    }

    /// Register a slot as in-use and top it up to `size`.
    async fn refill_one(&self, dc: i32, is_media: bool, hint: &str) {
        let have = {
            let mut slots = self.slots.lock().await;
            let slot = slots.entry((dc, is_media)).or_default();
            slot.last_used = Instant::now();
            slot.conns.len()
        };
        let need = (self.size)().saturating_sub(have);
        if need > 0 {
            self.fill(dc, is_media, hint, need).await;
        }
    }

    /// Schedule a one-off background top-up for one slot.
    pub fn schedule_refill(&'static self, dc: i32, is_media: bool, hint: String) {
        if is_media && !self.serves_media {
            return;
        }
        tokio::spawn(async move {
            self.refill_one(dc, is_media, &hint).await;
        });
    }

    /// Slots the refill task should keep warm: those acquired within
    /// `SLOT_IDLE_AFTER`. Expired connections everywhere are reaped on the way.
    async fn active_slots(&self) -> Vec<Key> {
        let ttl = (self.ttl)();
        let (active, expired) = {
            let mut slots = self.slots.lock().await;
            let mut expired = Vec::new();
            let mut active = Vec::new();
            for (key, slot) in slots.iter_mut() {
                expired.extend(take_expired(&mut slot.conns, ttl, created_at));
                if slot.last_used.elapsed() <= SLOT_IDLE_AFTER {
                    active.push(*key);
                }
            }
            (active, expired)
        };
        self.close_expired(expired).await;
        active
    }

    /// Background task: periodically top up every slot still in use.
    pub fn start_refill_task(&'static self) {
        tokio::spawn(async move {
            let mut tick = interval(self.refill_interval);
            // A sweep can outrun the interval on a degraded network; the default
            // `Burst` behaviour then fires the missed ticks back-to-back and
            // turns a slow sweep into a reconnect storm exactly when the network
            // is already struggling.
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                if !(self.enabled)() {
                    continue;
                }
                for (dc, is_media) in self.active_slots().await {
                    self.refill_one(dc, is_media, "").await;
                }
            }
        });
    }

    /// Pre-warm the seed slots.
    pub fn warmup(&'static self) {
        if !(self.enabled)() {
            log::debug!("{}: skip warmup (no backend configured)", self.name);
            return;
        }
        let seeds = (self.seeds)();
        if seeds.is_empty() {
            log::debug!("{}: skip warmup (no slots to seed)", self.name);
            return;
        }
        log::info!(
            "{}: warming up {} slot(s) (size={}, ttl={}s)",
            self.name,
            seeds.len(),
            (self.size)(),
            (self.ttl)().as_secs()
        );
        tokio::spawn(async move {
            for (dc, is_media) in seeds {
                self.refill_one(dc, is_media, "").await;
            }
            let slots = self.slots.lock().await;
            let total: usize = slots.values().map(|s| s.conns.len()).sum();
            log::info!("{}: warmup done ({total} connection(s) ready)", self.name);
        });
    }

    /// `(dc, is_media, depth)` snapshot for `--stats`, slots sorted.
    pub async fn depths(&self) -> Vec<(i32, bool, usize)> {
        let slots = self.slots.lock().await;
        let mut out: Vec<(i32, bool, usize)> = slots
            .iter()
            .map(|((dc, m), s)| (*dc, *m, s.conns.len()))
            .collect();
        out.sort_unstable();
        out
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(i: &Instant) -> Instant {
        *i
    }

    fn ago(secs: u64) -> Instant {
        Instant::now() - Duration::from_secs(secs)
    }

    #[test]
    fn take_expired_keeps_fresh_entries() {
        let mut v = vec![Instant::now(), Instant::now()];
        let expired = take_expired(&mut v, Duration::from_secs(60), stamp);
        assert!(expired.is_empty());
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn take_expired_returns_stale_entries() {
        let mut v = vec![ago(300), Instant::now(), ago(300)];
        let expired = take_expired(&mut v, Duration::from_secs(60), stamp);
        assert_eq!(
            expired.len(),
            2,
            "stale entries are handed back for close()"
        );
        assert_eq!(v.len(), 1, "the fresh entry stays pooled");
    }

    #[test]
    fn take_expired_preserves_order_of_survivors() {
        let fresh_a = Instant::now();
        let fresh_b = Instant::now();
        let mut v = vec![fresh_a, ago(300), fresh_b];
        take_expired(&mut v, Duration::from_secs(60), stamp);
        assert_eq!(v, vec![fresh_a, fresh_b]);
    }

    #[test]
    fn take_expired_on_empty_is_noop() {
        let mut v: Vec<Instant> = Vec::new();
        assert!(take_expired(&mut v, Duration::from_secs(1), stamp).is_empty());
    }

    #[test]
    fn take_expired_can_empty_the_slot() {
        let mut v = vec![ago(300), ago(400)];
        assert_eq!(
            take_expired(&mut v, Duration::from_secs(60), stamp).len(),
            2
        );
        assert!(v.is_empty());
    }

    #[test]
    fn fresh_slot_counts_as_active() {
        assert!(Slot::default().last_used.elapsed() <= SLOT_IDLE_AFTER);
    }

    #[test]
    fn slot_idle_past_threshold_is_retired() {
        let slot = Slot {
            conns: Vec::new(),
            last_used: ago(SLOT_IDLE_AFTER.as_secs() + 1),
        };
        assert!(slot.last_used.elapsed() > SLOT_IDLE_AFTER);
    }

    #[test]
    fn idle_threshold_exceeds_typical_ttl() {
        // The refill task must not retire a slot faster than its connections
        // expire, or a busy slot would flap between warm and cold.
        assert!(SLOT_IDLE_AFTER >= Duration::from_secs(120));
    }
}
