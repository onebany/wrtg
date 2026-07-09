/// Read buffer for relay loops (client ↔ remote).
pub const RELAY_BUF_SIZE: usize = 512 * 1024;

/// Target SO_RCVBUF / SO_SNDBUF for relay sockets (video-friendly).
pub const TCP_BUF_SIZE: usize = 512 * 1024;

pub fn tune_tcp(stream: &tokio::net::TcpStream) {
    if let Err(e) = stream.set_nodelay(true) {
        log::debug!("set_nodelay: {e}");
    }
    let sock = socket2::SockRef::from(stream);
    let _ = sock.set_recv_buffer_size(TCP_BUF_SIZE);
    let _ = sock.set_send_buffer_size(TCP_BUF_SIZE);
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::{get_original_dst, listen_transparent};

#[cfg(not(target_os = "linux"))]
mod stub;

#[cfg(not(target_os = "linux"))]
pub use stub::{get_original_dst, listen_transparent};
