//! Transparent listener binding + self-healing accept loop.
//!
//! The listener is owned by the accept loop (no shared lock). A broken listening
//! socket surfaces as repeated `accept()` errors; after a short run of them we
//! rebind a fresh transparent socket instead of busy-looping.

use std::future::Future;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};

/// Backoff after a failed `accept()` (avoids a hot error loop).
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(200);
/// Consecutive accept errors before rebinding the listening socket.
const REBIND_AFTER_ERRORS: u32 = 5;

pub async fn bind_transparent(listen_addr: &str) -> std::io::Result<TcpListener> {
    crate::sockopt::listen_transparent(listen_addr).await
}

/// Accept connections forever, dispatching each to `handler`. Rebinds the
/// transparent socket after a run of consecutive accept failures.
pub async fn serve<H, Fut>(mut listener: TcpListener, listen_addr: String, handler: H)
where
    H: Fn(TcpStream) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut errors: u32 = 0;
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                errors = 0;
                tokio::spawn(handler(stream));
            }
            Err(e) => {
                errors += 1;
                log::warn!("accept: {e} (#{errors})");
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                if errors >= REBIND_AFTER_ERRORS {
                    log::warn!("{errors} consecutive accept errors, rebinding on {listen_addr}");
                    match bind_transparent(&listen_addr).await {
                        Ok(l) => {
                            listener = l;
                            errors = 0;
                            log::info!("listening socket rebound on {listen_addr}");
                        }
                        Err(e) => log::error!("watchdog rebind failed: {e}"),
                    }
                }
            }
        }
    }
}
