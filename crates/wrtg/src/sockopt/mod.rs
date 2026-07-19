/// Read buffer for relay loops (client ↔ remote). 128 KiB is ample for video
/// chunking while keeping per-connection RAM bounded on the router.
pub const RELAY_BUF_SIZE: usize = 128 * 1024;

/// Target SO_RCVBUF / SO_SNDBUF for relay sockets (video-friendly).
pub const TCP_BUF_SIZE: usize = 512 * 1024;

fn tcp_keepalive_time() -> std::time::Duration {
    static D: std::sync::LazyLock<std::time::Duration> = std::sync::LazyLock::new(|| {
        std::env::var("WRTG_TCP_KEEPALIVE_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(std::time::Duration::from_secs)
            .unwrap_or(std::time::Duration::from_secs(30))
    });
    *D
}

pub fn tune_tcp(stream: &tokio::net::TcpStream) {
    if let Err(e) = stream.set_nodelay(true) {
        log::debug!("set_nodelay: {e}");
    }
    let sock = socket2::SockRef::from(stream);
    let _ = sock.set_recv_buffer_size(TCP_BUF_SIZE);
    let _ = sock.set_send_buffer_size(TCP_BUF_SIZE);
    let ka = socket2::TcpKeepalive::new().with_time(tcp_keepalive_time());
    let _ = sock.set_tcp_keepalive(&ka);
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{get_original_dst, listen_transparent};

#[cfg(not(target_os = "linux"))]
mod stub;

#[cfg(not(target_os = "linux"))]
pub use stub::{get_original_dst, listen_transparent};
