//! Runtime counters and the `wrtg --stats` snapshot socket.
//!
//! The daemon used to expose nothing about itself, so answering "is the relay
//! healthy?" meant scraping `logread` and sampling `/proc/<pid>` from cron.
//! Everything worth knowing — which fallback rung traffic is landing on, how
//! full the connection semaphore is, how deep each pool slot is — was already
//! in the process; it just had no way out.
//!
//! Counters are a flat array of atomics behind a `Stat` enum: adding one is a
//! line in each of two lists, and recording one is a relaxed `fetch_add` on a
//! path that already does TLS.

use std::sync::atomic::{AtomicU64, Ordering};

/// A monotonic counter. Keep in the same order as [`NAMES`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stat {
    Accepted,
    SelfConnectDropped,
    WsPoolHit,
    WsDirect,
    Fronting,
    CfWorker,
    CfProxy,
    TcpFallback,
    WorkerPassthrough,
    BlindRelay,
    AllPathsFailed,
    IdleReaped,
    PassthroughNoData,
}

/// Display names, index-aligned with [`Stat`].
const NAMES: [&str; Stat::COUNT] = [
    "accepted",
    "self_connect_dropped",
    "ws_pool_hit",
    "ws_direct",
    "fronting",
    "cf_worker",
    "cf_proxy",
    "tcp_fallback",
    "worker_passthrough",
    "blind_relay",
    "all_paths_failed",
    "idle_reaped",
    "passthrough_no_data",
];

impl Stat {
    pub const COUNT: usize = 13;

    pub fn name(self) -> &'static str {
        NAMES[self as usize]
    }
}

static COUNTERS: [AtomicU64; Stat::COUNT] = [const { AtomicU64::new(0) }; Stat::COUNT];
/// Connections currently being served (gauge, not a counter).
static ACTIVE: Gauge = Gauge::new();
/// Connection-semaphore capacity, published once at startup so a snapshot can
/// show `active/capacity` — the ratio that predicted the 0.5.28 wedge.
static CAPACITY: AtomicU64 = AtomicU64::new(0);

/// An up/down counter whose guard cannot leak.
///
/// A standalone type rather than a bare `static` so its balance can be tested on
/// a private instance: the process-wide gauge is shared by every test in the
/// binary, and asserting on it races whatever else is mid-connection.
pub struct Gauge(AtomicU64);

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

impl Gauge {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// Enter the gauge; the returned guard leaves it on drop.
    pub fn enter(&'static self) -> GaugeGuard {
        self.0.fetch_add(1, Ordering::Relaxed);
        GaugeGuard(self)
    }
}

/// Increments a [`Gauge`] and decrements it on drop, so a connection task cannot
/// leak the count on any early return, abort or panic.
pub struct GaugeGuard(&'static Gauge);

impl Drop for GaugeGuard {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn inc(s: Stat) {
    COUNTERS[s as usize].fetch_add(1, Ordering::Relaxed);
}

pub fn get(s: Stat) -> u64 {
    COUNTERS[s as usize].load(Ordering::Relaxed)
}

pub fn set_capacity(n: usize) {
    CAPACITY.store(n as u64, Ordering::Relaxed);
}

pub fn active() -> u64 {
    ACTIVE.get()
}

/// Mark a connection as in-flight for as long as the guard lives.
pub fn enter_active() -> GaugeGuard {
    ACTIVE.enter()
}

/// Render the human-readable snapshot served on the stats socket.
pub async fn snapshot() -> String {
    let mut out = String::with_capacity(768);
    out.push_str(&format!("wrtg {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!(
        "connections active={} capacity={}\n",
        active(),
        CAPACITY.load(Ordering::Relaxed)
    ));

    out.push_str("counters\n");
    for (i, name) in NAMES.iter().enumerate() {
        out.push_str(&format!(
            "  {name} {}\n",
            COUNTERS[i].load(Ordering::Relaxed)
        ));
    }

    for (label, depths) in [
        ("ws pool", crate::ws_pool::depths().await),
        ("cf worker pool", crate::cf_worker_pool::depths().await),
    ] {
        out.push_str(&format!("{label}\n"));
        if depths.is_empty() {
            out.push_str("  (no slots)\n");
        }
        for (dc, media, depth) in depths {
            let tag = if media { "m" } else { "" };
            out.push_str(&format!("  DC{dc}{tag} {depth}\n"));
        }
    }
    out
}

/// Default path of the snapshot socket. Under `/var/run` (tmpfs on OpenWrt), so
/// a stale socket never survives a reboot onto flash.
pub const DEFAULT_SOCKET: &str = "/var/run/wrtg.sock";

pub fn socket_path() -> String {
    std::env::var("WRTG_STATS_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string())
}

#[cfg(unix)]
mod imp {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};

    /// Serve one snapshot per connection on the stats socket.
    ///
    /// Best-effort: a router where the socket cannot be bound still relays
    /// traffic, so a failure here is a warning, never a startup abort.
    pub fn serve(path: String) {
        tokio::spawn(async move {
            // A previous run that was SIGKILLed leaves the node behind and bind
            // would fail with EADDRINUSE; the socket is per-daemon, so removing
            // it is safe. Blocking `std::fs` here rather than pulling in tokio's
            // `fs` feature: this runs once, before the listener exists.
            let _ = std::fs::remove_file(&path);
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    log::warn!("stats socket {path}: {e}");
                    return;
                }
            };
            log::info!("stats socket on {path}");
            loop {
                match listener.accept().await {
                    Ok((mut s, _)) => {
                        let body = super::snapshot().await;
                        tokio::spawn(async move {
                            let _ = s.write_all(body.as_bytes()).await;
                            let _ = s.shutdown().await;
                        });
                    }
                    Err(e) => {
                        log::warn!("stats accept: {e}");
                        return;
                    }
                }
            }
        });
    }

    /// `--stats`: read the running daemon's snapshot. Returns an exit code.
    pub async fn print(path: &str) -> i32 {
        let mut stream = match UnixStream::connect(path).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("wrtg --stats: cannot reach daemon on {path}: {e}");
                eprintln!("(is wrtg running? the socket appears once the daemon has started)");
                return 1;
            }
        };
        let mut buf = String::new();
        if let Err(e) = stream.read_to_string(&mut buf).await {
            eprintln!("wrtg --stats: read failed: {e}");
            return 1;
        }
        print!("{buf}");
        0
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn serve(_path: String) {}

    pub async fn print(_path: &str) -> i32 {
        eprintln!("wrtg --stats: unix sockets only");
        1
    }
}

pub use imp::{print, serve};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cover_every_stat() {
        assert_eq!(NAMES.len(), Stat::COUNT);
    }

    #[test]
    fn names_are_index_aligned_with_the_enum() {
        // A counter renamed without reordering NAMES would silently report the
        // wrong number, which is worse than not reporting it at all.
        assert_eq!(Stat::Accepted.name(), "accepted");
        assert_eq!(Stat::PassthroughNoData.name(), "passthrough_no_data");
        assert_eq!(Stat::AllPathsFailed.name(), "all_paths_failed");
    }

    #[test]
    fn names_are_unique() {
        let mut seen = NAMES.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), NAMES.len());
    }

    #[test]
    fn inc_is_observable() {
        let before = get(Stat::BlindRelay);
        inc(Stat::BlindRelay);
        assert_eq!(get(Stat::BlindRelay), before + 1);
    }

    // Private gauges, so these assertions are immune to whatever the rest of
    // the test binary is doing to the process-wide one.
    static G_BALANCE: Gauge = Gauge::new();
    static G_PANIC: Gauge = Gauge::new();
    static G_NESTED: Gauge = Gauge::new();

    #[test]
    fn gauge_guard_balances_out() {
        assert_eq!(G_BALANCE.get(), 0);
        {
            let _g = G_BALANCE.enter();
            assert_eq!(G_BALANCE.get(), 1);
        }
        assert_eq!(G_BALANCE.get(), 0, "the gauge must not leak on drop");
    }

    #[test]
    fn gauge_guard_nests() {
        let a = G_NESTED.enter();
        let b = G_NESTED.enter();
        assert_eq!(G_NESTED.get(), 2);
        drop(b);
        assert_eq!(G_NESTED.get(), 1);
        drop(a);
        assert_eq!(G_NESTED.get(), 0);
    }

    #[test]
    fn gauge_guard_unwinds_on_panic() {
        let r = std::panic::catch_unwind(|| {
            let _g = G_PANIC.enter();
            assert_eq!(G_PANIC.get(), 1);
            panic!("boom");
        });
        assert!(r.is_err());
        assert_eq!(
            G_PANIC.get(),
            0,
            "a panicking session must release its slot"
        );
    }

    #[tokio::test]
    async fn snapshot_reports_every_counter_and_both_pools() {
        let out = snapshot().await;
        assert!(out.starts_with("wrtg "), "snapshot must name the version");
        assert!(out.contains("connections active="));
        assert!(out.contains("capacity="));
        for name in NAMES {
            assert!(out.contains(name), "snapshot omitted counter {name}");
        }
        assert!(out.contains("ws pool"));
        assert!(out.contains("cf worker pool"));
    }

    #[test]
    fn socket_path_has_a_default() {
        // Only meaningful when the env var is unset, which is the norm.
        if std::env::var("WRTG_STATS_SOCKET").is_err() {
            assert_eq!(socket_path(), DEFAULT_SOCKET);
        }
    }
}
