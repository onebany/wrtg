//! Minimal HTTP/1.1-over-TLS GET, shared by the DoH resolver and the CF-proxy
//! domain-list fetch. Both dial a pinned IP themselves (their strategies differ)
//! and then run the same TLS handshake + request + bounded read + status check.

use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// GET `path` over TLS on an already-connected `tcp`, validating the certificate
/// against `sni` (also used as the `Host` header). Adds `extra_headers`, caps the
/// body at `max_body` bytes, and returns the body (headers stripped) on a 200.
pub async fn get_over(
    tcp: TcpStream,
    sni: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    max_body: usize,
    connect_timeout: Duration,
) -> std::io::Result<Vec<u8>> {
    crate::sockopt::tune_tcp(&tcp);

    let connector = crate::tls::connector();
    let server_name = ServerName::try_from(sni.to_string()).map_err(std::io::Error::other)?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp)).await??;

    let mut req = format!("GET {path} HTTP/1.1\r\nHost: {sni}\r\n");
    for (name, value) in extra_headers {
        req.push_str(name);
        req.push_str(": ");
        req.push_str(value);
        req.push_str("\r\n");
    }
    req.push_str("User-Agent: wrtg\r\nConnection: close\r\n\r\n");
    timeout(connect_timeout, stream.write_all(req.as_bytes())).await??;

    let mut buf = Vec::new();
    let mut limited = stream.take((max_body + 1) as u64);
    timeout(connect_timeout, limited.read_to_end(&mut buf)).await??;
    if buf.len() > max_body {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "HTTP response too large",
        ));
    }

    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no HTTP headers"))?;
    let status = String::from_utf8_lossy(&buf[..header_end.min(32)]);
    if !status.contains(" 200 ") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("HTTP error: {}", status.lines().next().unwrap_or("")),
        ));
    }
    Ok(buf[header_end + 4..].to_vec())
}
