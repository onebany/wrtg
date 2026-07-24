//! Transparent listener binding + self-healing accept loop.
//!
//! The listener is owned by the accept loop (no shared lock). A broken listening
//! socket surfaces as repeated `accept()` errors; after a short run of them we
//! rebind a fresh transparent socket instead of busy-looping.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

/// Backoff after a failed `accept()` (avoids a hot error loop).
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(200);
/// Consecutive accept errors before rebinding the listening socket.
const REBIND_AFTER_ERRORS: u32 = 5;
/// Ceiling for the exponential backoff between failed rebind attempts.
const REBIND_MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Default cap on simultaneously-served connections.
const DEFAULT_MAX_CONNS: usize = 1024;

/// Maximum in-flight connections. Bounds task/buffer growth so a connection
/// flood can't exhaust memory. Tunable via `WRTG_MAX_CONNS` (0/unset → default).
fn max_conns() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("WRTG_MAX_CONNS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_MAX_CONNS)
    })
}

pub async fn bind_transparent(listen_addr: &str) -> std::io::Result<TcpListener> {
    crate::sockopt::listen_transparent(listen_addr).await
}

/// Accept connections forever, dispatching each to `handler`. Rebinds the
/// transparent socket after a run of consecutive accept failures, and bounds the
/// number of connections served at once so a flood can't spawn unbounded tasks.
pub async fn serve<H, Fut>(listener: TcpListener, listen_addr: String, handler: H)
where
    H: Fn(TcpStream) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    serve_with_cap(listener, listen_addr, handler, max_conns()).await
}

/// [`serve`] with an explicit connection cap, so the backpressure behaviour can
/// be exercised without reaching for a process-wide env var.
pub async fn serve_with_cap<H, Fut>(
    mut listener: TcpListener,
    listen_addr: String,
    handler: H,
    cap: usize,
) where
    H: Fn(TcpStream) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    crate::stats::set_capacity(cap);
    let sem = Arc::new(Semaphore::new(cap));
    let mut errors: u32 = 0;
    let mut rebind_backoff = ACCEPT_ERROR_BACKOFF;
    loop {
        // Reserve a slot before accepting. At the cap this awaits until an
        // in-flight connection finishes, applying backpressure (the kernel
        // accept backlog absorbs the wait) instead of spawning without bound.
        let Ok(permit) = sem.clone().acquire_owned().await else {
            return; // semaphore closed — nothing more to serve
        };
        match listener.accept().await {
            Ok((stream, _)) => {
                errors = 0;
                crate::stats::inc(crate::stats::Stat::Accepted);
                let fut = handler(stream);
                tokio::spawn(async move {
                    let _permit = permit; // released when the connection ends
                    let _active = crate::stats::enter_active();
                    fut.await;
                });
            }
            Err(e) => {
                drop(permit); // don't hold a slot while backing off
                errors += 1;
                log::warn!("accept: {e} (#{errors})");
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                if errors >= REBIND_AFTER_ERRORS {
                    log::warn!("{errors} consecutive accept errors, rebinding on {listen_addr}");
                    match bind_transparent(&listen_addr).await {
                        Ok(l) => {
                            listener = l;
                            errors = 0;
                            rebind_backoff = ACCEPT_ERROR_BACKOFF;
                            log::info!("listening socket rebound on {listen_addr}");
                        }
                        Err(e) => {
                            // Back off exponentially (capped) so a persistently
                            // unbindable socket doesn't spin a ~200ms error loop.
                            log::error!(
                                "watchdog rebind failed: {e} (retry in {rebind_backoff:?})"
                            );
                            tokio::time::sleep(rebind_backoff).await;
                            rebind_backoff = (rebind_backoff * 2).min(REBIND_MAX_BACKOFF);
                            errors = REBIND_AFTER_ERRORS; // stay armed to retry
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream as ClientStream;
    use tokio::sync::oneshot;

    const _: () = assert!(REBIND_AFTER_ERRORS > 0);
    const _: () = assert!(DEFAULT_MAX_CONNS > 0);

    #[test]
    fn rebind_backoff_is_capped_above_the_accept_backoff() {
        // The exponential retry must start below its ceiling, or the very first
        // failed rebind would already wait the maximum.
        assert!(ACCEPT_ERROR_BACKOFF < REBIND_MAX_BACKOFF);
    }

    #[test]
    fn max_conns_is_positive() {
        assert!(max_conns() > 0, "a zero cap would deadlock the accept loop");
    }

    /// Bind a listener on an ephemeral loopback port.
    async fn listener() -> (TcpListener, String) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        (l, addr)
    }

    #[tokio::test]
    async fn serve_dispatches_each_connection_to_the_handler() {
        let (l, addr) = listener().await;
        let seen = Arc::new(AtomicUsize::new(0));
        let seen_h = seen.clone();

        let server = tokio::spawn(async move {
            serve_with_cap(
                l,
                "test".to_string(),
                move |_stream| {
                    let seen = seen_h.clone();
                    async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                    }
                },
                8,
            )
            .await;
        });

        for _ in 0..3 {
            let mut c = ClientStream::connect(&addr).await.unwrap();
            let _ = c.shutdown().await;
        }

        // Poll rather than sleep a fixed span, so the test is not timing-fragile.
        for _ in 0..200 {
            if seen.load(Ordering::SeqCst) == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(seen.load(Ordering::SeqCst), 3);
        server.abort();
    }

    #[tokio::test]
    async fn in_flight_connections_are_capped() {
        const CAP: usize = 2;
        let (l, addr) = listener().await;
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let release = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));

        let (inf, pk) = (in_flight.clone(), peak.clone());
        let server = tokio::spawn(async move {
            serve_with_cap(
                l,
                "test".to_string(),
                move |_stream| {
                    let (inf, pk, release) = (inf.clone(), pk.clone(), release.clone());
                    async move {
                        let n = inf.fetch_add(1, Ordering::SeqCst) + 1;
                        pk.fetch_max(n, Ordering::SeqCst);
                        // Hold the slot until the test releases it. Only the
                        // first handler owns the receiver; the rest just park.
                        if let Some(rx) = release.lock().await.take() {
                            let _ = rx.await;
                        } else {
                            tokio::time::sleep(Duration::from_secs(30)).await;
                        }
                        inf.fetch_sub(1, Ordering::SeqCst);
                    }
                },
                CAP,
            )
            .await;
        });

        // Open well past the cap; the extra connections must sit in the kernel
        // backlog rather than spawn handlers.
        let mut clients = Vec::new();
        for _ in 0..CAP + 4 {
            clients.push(ClientStream::connect(&addr).await.unwrap());
        }

        for _ in 0..100 {
            if in_flight.load(Ordering::SeqCst) >= CAP {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Give any over-admission a chance to show up before asserting.
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(
            peak.load(Ordering::SeqCst),
            CAP,
            "the semaphore admitted more than its cap"
        );

        let _ = release_tx.send(());
        server.abort();
    }

    #[tokio::test]
    async fn accepting_a_connection_bumps_the_accepted_counter() {
        let (l, addr) = listener().await;
        let before = crate::stats::get(crate::stats::Stat::Accepted);
        let server = tokio::spawn(async move {
            serve_with_cap(l, "test".to_string(), |_s| async {}, 4).await;
        });

        let mut c = ClientStream::connect(&addr).await.unwrap();
        let _ = c.shutdown().await;

        // Only a lower bound: this counter is process-wide and other tests in
        // the binary accept connections too.
        for _ in 0..200 {
            if crate::stats::get(crate::stats::Stat::Accepted) > before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(crate::stats::get(crate::stats::Stat::Accepted) > before);
        server.abort();
    }
}
