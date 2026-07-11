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
pub async fn serve<H, Fut>(mut listener: TcpListener, listen_addr: String, handler: H)
where
    H: Fn(TcpStream) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let sem = Arc::new(Semaphore::new(max_conns()));
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
                let fut = handler(stream);
                tokio::spawn(async move {
                    let _permit = permit; // released when the connection ends
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
