use std::collections::HashMap;
use std::time::Duration;

use crate::mtproto::MAX_WS_PAYLOAD;
use crate::sockopt::tune_tcp;
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::Rng;
use rustls::pki_types::ServerName;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

type WsStream = tokio_rustls::client::TlsStream<TcpStream>;

#[derive(Debug, Clone)]
pub struct WsHandshakeError {
    pub status_code: u16,
    pub status_line: String,
    pub headers: HashMap<String, String>,
}

#[derive(Debug)]
pub enum WsConnectError {
    Io(std::io::Error),
    Handshake(WsHandshakeError),
    Timeout,
}

impl WsConnectError {
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Handshake(h) => Some(h.status_code),
            _ => None,
        }
    }

    pub fn into_io(self) -> std::io::Error {
        match self {
            Self::Io(e) => e,
            Self::Timeout => std::io::Error::new(std::io::ErrorKind::TimedOut, "WebSocket timeout"),
            Self::Handshake(h) => std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("WS handshake failed: {}", h.status_line.trim()),
            ),
        }
    }
}

impl From<std::io::Error> for WsConnectError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub fn is_ws_http_status(err: &WsConnectError, code: u16) -> bool {
    err.http_status() == Some(code)
}

pub fn retry_after_from_err(err: &WsConnectError) -> Duration {
    let WsConnectError::Handshake(h) = err else {
        return Duration::ZERO;
    };
    let retry_after = h.headers.get("retry-after").map(|s| s.trim()).unwrap_or("");
    if retry_after.is_empty() {
        return Duration::ZERO;
    }
    if let Ok(seconds) = retry_after.parse::<u64>() {
        if seconds > 0 {
            return Duration::from_secs(seconds);
        }
    }
    Duration::ZERO
}

const MAX_HTTP_LINE: usize = 8 * 1024;
const MAX_HTTP_HEADERS: usize = 32 * 1024;
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub struct RawWebSocket {
    stream: WsStream,
}

pub struct WsReadHalf {
    read: tokio::io::ReadHalf<WsStream>,
}

pub struct WsWriteHalf {
    write: tokio::io::WriteHalf<WsStream>,
}

pub async fn connect_ws(
    target_ip: &str,
    domain: &str,
    path: &str,
    connect_timeout: Duration,
) -> std::io::Result<RawWebSocket> {
    connect_ws_with_headers(target_ip, domain, path, connect_timeout, &[])
        .await
        .map_err(WsConnectError::into_io)
}

/// TLS fronting: TCP to `target_ip`, HTTP Host `host`, TLS SNI `sni`.
pub async fn connect_ws_fronted(
    target_ip: &str,
    host: &str,
    sni: &str,
    path: &str,
    connect_timeout: Duration,
) -> Result<RawWebSocket, WsConnectError> {
    connect_ws_inner(target_ip, host, Some(sni), path, connect_timeout, &[]).await
}

pub async fn connect_ws_with_headers(
    target_ip: &str,
    domain: &str,
    path: &str,
    connect_timeout: Duration,
    extra_headers: &[(&str, &str)],
) -> Result<RawWebSocket, WsConnectError> {
    connect_ws_inner(
        target_ip,
        domain,
        None,
        path,
        connect_timeout,
        extra_headers,
    )
    .await
}

async fn connect_ws_inner(
    target_ip: &str,
    host: &str,
    sni: Option<&str>,
    path: &str,
    connect_timeout: Duration,
    extra_headers: &[(&str, &str)],
) -> Result<RawWebSocket, WsConnectError> {
    let addr = format!("{target_ip}:443");
    let tcp = match timeout(connect_timeout, TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(WsConnectError::Io(e)),
        Err(_) => return Err(WsConnectError::Timeout),
    };
    tune_tcp(&tcp);

    let connector = crate::tls::connector();
    let tls_name = sni.unwrap_or(host);
    let server_name = ServerName::try_from(tls_name.to_string())
        .map_err(|e| WsConnectError::Io(std::io::Error::other(e)))?;
    let mut stream = match timeout(connect_timeout, connector.connect(server_name, tcp)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(WsConnectError::Io(e)),
        Err(_) => return Err(WsConnectError::Timeout),
    };

    let ws_key = {
        let mut key = [0u8; 16];
        rand::rng().fill_bytes(&mut key);
        STANDARD.encode(key)
    };

    let mut req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {ws_key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: binary\r\n"
    );
    for (name, value) in extra_headers {
        if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
            return Err(WsConnectError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid WebSocket header",
            )));
        }
        req.push_str(name);
        req.push_str(": ");
        req.push_str(value);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");

    match timeout(connect_timeout, stream.write_all(req.as_bytes())).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(WsConnectError::Io(e)),
        Err(_) => return Err(WsConnectError::Timeout),
    }

    let mut status_line = Vec::new();
    if let Err(e) = read_line(
        &mut stream,
        &mut status_line,
        connect_timeout,
        MAX_HTTP_LINE,
    )
    .await
    {
        return Err(map_read_err(e));
    }
    let mut headers = HashMap::<String, String>::new();
    let mut headers_len = status_line.len();
    loop {
        let mut line = Vec::new();
        if let Err(e) = read_line(&mut stream, &mut line, connect_timeout, MAX_HTTP_LINE).await {
            return Err(map_read_err(e));
        }
        headers_len += line.len();
        if headers_len > MAX_HTTP_HEADERS {
            return Err(WsConnectError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WebSocket response headers too large",
            )));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let line = String::from_utf8_lossy(&line);
        let Some((name, value)) = line.trim_end().split_once(':') else {
            return Err(WsConnectError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "malformed WebSocket response header",
            )));
        };
        headers
            .entry(name.trim().to_ascii_lowercase())
            .and_modify(|old| {
                old.push(',');
                old.push_str(value.trim());
            })
            .or_insert_with(|| value.trim().to_string());
    }

    let status = String::from_utf8_lossy(&status_line);
    let status_code = status
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    if status_code != 101 {
        return Err(WsConnectError::Handshake(WsHandshakeError {
            status_code,
            status_line: status.trim().to_string(),
            headers,
        }));
    }
    let upgrade_ok = headers
        .get("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let connection_ok = headers.get("connection").is_some_and(|v| {
        v.split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    });
    let expected_accept = websocket_accept(&ws_key);
    let accept_ok = headers
        .get("sec-websocket-accept")
        .is_some_and(|v| v == &expected_accept);
    if !upgrade_ok || !connection_ok || !accept_ok {
        return Err(WsConnectError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid WebSocket upgrade response",
        )));
    }

    if let Err(e) = stream.get_ref().0.set_nodelay(true) {
        log::debug!("ws set_nodelay: {e}");
    }

    Ok(RawWebSocket { stream })
}

async fn read_line(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    buf: &mut Vec<u8>,
    t: Duration,
    max_len: usize,
) -> std::io::Result<()> {
    let mut byte = [0u8; 1];
    loop {
        match timeout(t, stream.read_exact(&mut byte)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "WebSocket read timeout",
                ))
            }
        }
        buf.push(byte[0]);
        if buf.len() > max_len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP response line too long",
            ));
        }
        if byte[0] == b'\n' {
            return Ok(());
        }
    }
}

fn map_read_err(e: std::io::Error) -> WsConnectError {
    if e.kind() == std::io::ErrorKind::TimedOut {
        WsConnectError::Timeout
    } else {
        WsConnectError::Io(e)
    }
}

pub fn ws_ping_frame() -> Vec<u8> {
    build_ws_frame(0x9, &[], true)
}

fn websocket_accept(key: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(key.as_bytes());
    sha1.update(WS_GUID.as_bytes());
    STANDARD.encode(sha1.finalize())
}

pub async fn connect_cf_worker_ws(
    worker_domain: &str,
    dst_ip: &str,
    dc: i32,
    connect_timeout: Duration,
) -> std::io::Result<RawWebSocket> {
    let path = format!("/apiws?dst={dst_ip}&dc={dc}");
    connect_cf_worker(worker_domain, &path, connect_timeout).await
}

/// Open a raw TCP tunnel to `dst_ip:dst_port` through the CF Worker (for
/// passthrough of TLS / MTProto-over-HTTP media traffic to blocked DCs).
pub async fn connect_cf_worker_tcp(
    worker_domain: &str,
    dst_ip: &str,
    dst_port: u16,
    connect_timeout: Duration,
) -> std::io::Result<RawWebSocket> {
    let path = format!("/apiws?dst={dst_ip}&dc=0&port={dst_port}");
    connect_cf_worker(worker_domain, &path, connect_timeout).await
}

async fn connect_cf_worker(
    worker_domain: &str,
    path: &str,
    connect_timeout: Duration,
) -> std::io::Result<RawWebSocket> {
    let token = std::env::var("WRTG_CF_WORKER_TOKEN")
        .unwrap_or_default()
        .trim()
        .to_string();
    let headers = if token.is_empty() {
        Vec::new()
    } else {
        vec![("X-WRTG-Token", token.as_str())]
    };
    connect_ws_with_headers(
        worker_domain,
        worker_domain,
        path,
        connect_timeout,
        &headers,
    )
    .await
    .map_err(WsConnectError::into_io)
}

pub fn is_ws_redirect(err: &std::io::Error) -> bool {
    let msg = err.to_string();
    [" 302 ", " 301 ", " 303 ", " 307 ", " 308 "]
        .iter()
        .any(|code| msg.contains(code))
}

pub fn is_ws_redirect_err(err: &WsConnectError) -> bool {
    match err {
        WsConnectError::Handshake(h) => matches!(h.status_code, 301 | 302 | 303 | 307 | 308),
        WsConnectError::Io(e) => is_ws_redirect(e),
        _ => false,
    }
}

impl RawWebSocket {
    pub fn into_halves(self) -> (WsReadHalf, WsWriteHalf) {
        let (read, write) = tokio::io::split(self.stream);
        (WsReadHalf { read }, WsWriteHalf { write })
    }

    pub async fn send(&mut self, data: &[u8]) -> std::io::Result<()> {
        let frame = build_ws_frame(0x2, data, true);
        self.stream.write_all(&frame).await
    }

    pub async fn send_batch(&mut self, parts: &[Vec<u8>]) -> std::io::Result<()> {
        for p in parts {
            self.send(p).await?;
        }
        Ok(())
    }

    pub async fn recv(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        let mut fragmented = None;
        loop {
            let (fin, opcode, payload) = read_frame(&mut self.stream).await?;
            match opcode {
                0x8 => return Ok(None),
                0x9 => {
                    let pong = build_ws_frame(0xA, &payload, true);
                    self.stream.write_all(&pong).await?;
                }
                0xA => {}
                0x0..=0x2 => {
                    if let Some(message) =
                        assemble_data_message(fin, opcode, payload, &mut fragmented)?
                    {
                        return Ok(Some(message));
                    }
                }
                _ => return Err(ws_protocol_error("unsupported WebSocket opcode")),
            }
        }
    }

    pub async fn close(&mut self) {
        let frame = build_ws_frame(0x8, &[], true);
        let _ = self.stream.write_all(&frame).await;
        let _ = self.stream.shutdown().await;
    }
}

impl WsWriteHalf {
    pub async fn send_binary(&mut self, data: &[u8]) -> std::io::Result<()> {
        let frame = build_ws_frame(0x2, data, true);
        self.write.write_all(&frame).await
    }

    pub async fn send_batch(&mut self, parts: &[Vec<u8>]) -> std::io::Result<()> {
        for p in parts {
            let frame = build_ws_frame(0x2, p, true);
            self.write.write_all(&frame).await?;
        }
        Ok(())
    }

    pub async fn send_raw(&mut self, frame: &[u8]) -> std::io::Result<()> {
        self.write.write_all(frame).await
    }

    pub async fn close(&mut self) {
        let frame = build_ws_frame(0x8, &[], true);
        let _ = self.write.write_all(&frame).await;
        let _ = self.write.shutdown().await;
    }
}

impl WsReadHalf {
    pub async fn recv_binary(
        &mut self,
        pong_tx: &tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> std::io::Result<Option<Vec<u8>>> {
        let mut fragmented = None;
        loop {
            let (fin, opcode, payload) = read_frame(&mut self.read).await?;
            match opcode {
                0x8 => return Ok(None),
                0x9 => {
                    let pong = build_ws_frame(0xA, &payload, true);
                    pong_tx.send(pong).await.map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "WebSocket writer closed",
                        )
                    })?;
                }
                0xA => {}
                0x0..=0x2 => {
                    if let Some(message) =
                        assemble_data_message(fin, opcode, payload, &mut fragmented)?
                    {
                        return Ok(Some(message));
                    }
                }
                _ => return Err(ws_protocol_error("unsupported WebSocket opcode")),
            }
        }
    }
}

async fn read_frame<R: AsyncReadExt + Unpin>(read: &mut R) -> std::io::Result<(bool, u8, Vec<u8>)> {
    let mut hdr = [0u8; 2];
    read.read_exact(&mut hdr).await?;
    if hdr[0] & 0x70 != 0 {
        return Err(ws_protocol_error("WebSocket RSV bits are set"));
    }
    let fin = hdr[0] & 0x80 != 0;
    let opcode = hdr[0] & 0x0F;
    let masked = hdr[1] & 0x80 != 0;
    if masked {
        return Err(ws_protocol_error("masked server WebSocket frame"));
    }
    let mut length = (hdr[1] & 0x7F) as usize;
    match length {
        126 => {
            let mut ext = [0u8; 2];
            read.read_exact(&mut ext).await?;
            length = u16::from_be_bytes(ext) as usize;
        }
        127 => {
            let mut ext = [0u8; 8];
            read.read_exact(&mut ext).await?;
            let len64 = u64::from_be_bytes(ext);
            if len64 > MAX_WS_PAYLOAD as u64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("WS frame too large: {len64}"),
                ));
            }
            length = len64 as usize;
        }
        _ => {}
    }

    if length > MAX_WS_PAYLOAD {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("WS frame too large: {length}"),
        ));
    }
    if opcode >= 0x8 && (!fin || length > 125) {
        return Err(ws_protocol_error("invalid WebSocket control frame"));
    }

    let mut payload = vec![0u8; length];
    read.read_exact(&mut payload).await?;
    Ok((fin, opcode, payload))
}

fn assemble_data_message(
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
    fragmented: &mut Option<Vec<u8>>,
) -> std::io::Result<Option<Vec<u8>>> {
    match opcode {
        0x1 | 0x2 => {
            if fragmented.is_some() {
                return Err(ws_protocol_error(
                    "new WebSocket data frame during fragmented message",
                ));
            }
            if fin {
                return Ok(Some(payload));
            }
            *fragmented = Some(payload);
            Ok(None)
        }
        0x0 => {
            let Some(message) = fragmented.as_mut() else {
                return Err(ws_protocol_error(
                    "WebSocket continuation without initial frame",
                ));
            };
            if message.len().saturating_add(payload.len()) > MAX_WS_PAYLOAD {
                return Err(ws_protocol_error("fragmented WebSocket message too large"));
            }
            message.extend_from_slice(&payload);
            if fin {
                return Ok(fragmented.take());
            }
            Ok(None)
        }
        _ => Err(ws_protocol_error("not a WebSocket data frame")),
    }
}

fn ws_protocol_error(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

fn build_ws_frame(opcode: u8, data: &[u8], mask: bool) -> Vec<u8> {
    let fb = 0x80 | opcode;
    let length = data.len();
    if !mask {
        let mut hdr = ws_header_unmasked(fb, length);
        hdr.extend_from_slice(data);
        return hdr;
    }
    let mut mask_key = [0u8; 4];
    rand::rng().fill_bytes(&mut mask_key);
    let masked = xor_mask_owned(data, &mask_key);
    let mut hdr = ws_header_masked(fb, length, &mask_key);
    hdr.extend_from_slice(&masked);
    hdr
}

fn ws_header_unmasked(fb: u8, length: usize) -> Vec<u8> {
    match length {
        l if l < 126 => vec![fb, l as u8],
        l if l < 65536 => {
            let mut b = vec![fb, 126];
            b.extend_from_slice(&(l as u16).to_be_bytes());
            b
        }
        l => {
            let mut b = vec![fb, 127];
            b.extend_from_slice(&(l as u64).to_be_bytes());
            b
        }
    }
}

fn ws_header_masked(fb: u8, length: usize, mask_key: &[u8; 4]) -> Vec<u8> {
    match length {
        l if l < 126 => {
            let mut b = vec![fb, 0x80 | l as u8];
            b.extend_from_slice(mask_key);
            b
        }
        l if l < 65536 => {
            let mut b = vec![fb, 0x80 | 126];
            b.extend_from_slice(&(l as u16).to_be_bytes());
            b.extend_from_slice(mask_key);
            b
        }
        l => {
            let mut b = vec![fb, 0x80 | 127];
            b.extend_from_slice(&(l as u64).to_be_bytes());
            b.extend_from_slice(mask_key);
            b
        }
    }
}

fn xor_mask_owned(data: &[u8], mask: &[u8; 4]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_accept_matches_rfc6455() {
        assert_eq!(
            websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn fragmented_message_is_reassembled() {
        let mut fragmented = None;
        assert!(
            assemble_data_message(false, 0x2, b"tele".to_vec(), &mut fragmented)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            assemble_data_message(true, 0x0, b"gram".to_vec(), &mut fragmented).unwrap(),
            Some(b"telegram".to_vec())
        );
    }

    #[test]
    fn unexpected_continuation_is_rejected() {
        let mut fragmented = None;
        assert!(assemble_data_message(true, 0x0, Vec::new(), &mut fragmented).is_err());
    }
}
