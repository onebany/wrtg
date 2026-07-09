use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use crate::mtproto::MAX_WS_PAYLOAD;
use crate::sockopt::tune_tcp;
use rand::RngCore;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

type WsStream = tokio_rustls::client::TlsStream<TcpStream>;

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
    let addr = format!("{target_ip}:443");
    let tcp = timeout(connect_timeout, TcpStream::connect(&addr)).await??;
    tune_tcp(&tcp);

    let config = Arc::new(
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
            .with_no_client_auth(),
    );

    let connector = TlsConnector::from(config);
    let server_name = ServerName::try_from(domain.to_string()).map_err(std::io::Error::other)?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp)).await??;

    let ws_key = {
        let mut key = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut key);
        STANDARD.encode(key)
    };

    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {domain}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {ws_key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Protocol: binary\r\n\
         \r\n"
    );

    timeout(connect_timeout, stream.write_all(req.as_bytes())).await??;

    let mut status_line = Vec::new();
    read_line(&mut stream, &mut status_line, connect_timeout).await?;
    loop {
        let mut line = Vec::new();
        read_line(&mut stream, &mut line, connect_timeout).await?;
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }

    let status = String::from_utf8_lossy(&status_line);
    if !status.contains(" 101 ") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("WS handshake failed: {}", status.trim()),
        ));
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
) -> std::io::Result<()> {
    let mut byte = [0u8; 1];
    loop {
        timeout(t, stream.read_exact(&mut byte)).await??;
        buf.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(());
        }
    }
}

pub async fn connect_cf_worker_ws(
    worker_domain: &str,
    dst_ip: &str,
    dc: i32,
    connect_timeout: Duration,
) -> std::io::Result<RawWebSocket> {
    let path = format!("/apiws?dst={dst_ip}&dc={dc}");
    connect_ws(worker_domain, worker_domain, &path, connect_timeout).await
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
    connect_ws(worker_domain, worker_domain, &path, connect_timeout).await
}

pub fn is_ws_redirect(err: &std::io::Error) -> bool {
    let msg = err.to_string();
    [" 302 ", " 301 ", " 303 ", " 307 ", " 308 "]
        .iter()
        .any(|code| msg.contains(code))
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
        loop {
            let (opcode, payload) = read_frame(&mut self.stream).await?;
            match opcode {
                0x8 => return Ok(None),
                0x9 => {
                    let pong = build_ws_frame(0xA, &payload, true);
                    let _ = self.stream.write_all(&pong).await;
                }
                0x1 | 0x2 => return Ok(Some(payload)),
                _ => {}
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
        loop {
            let (opcode, payload) = read_frame(&mut self.read).await?;
            match opcode {
                0x8 => return Ok(None),
                0x9 => {
                    let pong = build_ws_frame(0xA, &payload, true);
                    let _ = pong_tx.send(pong).await;
                }
                0x1 | 0x2 => return Ok(Some(payload)),
                _ => {}
            }
        }
    }
}

async fn read_frame<R: AsyncReadExt + Unpin>(read: &mut R) -> std::io::Result<(u8, Vec<u8>)> {
    let mut hdr = [0u8; 2];
    read.read_exact(&mut hdr).await?;
    let opcode = hdr[0] & 0x0F;
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

    let payload = if hdr[1] & 0x80 != 0 {
        let mut mask_key = [0u8; 4];
        read.read_exact(&mut mask_key).await?;
        let mut payload = vec![0u8; length];
        read.read_exact(&mut payload).await?;
        xor_mask(&mut payload, &mask_key);
        payload
    } else {
        let mut payload = vec![0u8; length];
        read.read_exact(&mut payload).await?;
        payload
    };
    Ok((opcode, payload))
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
    rand::thread_rng().fill_bytes(&mut mask_key);
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

fn xor_mask(data: &mut [u8], mask: &[u8; 4]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= mask[i % 4];
    }
}

fn xor_mask_owned(data: &[u8], mask: &[u8; 4]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect()
}

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
