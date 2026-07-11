use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::mtproto::{parse_direct_handshake, HandshakeInfo, HANDSHAKE_LEN};

const INIT_READ_MAX: usize = 4096;

pub struct ParsedInit {
    pub info: HandshakeInfo,
    pub stream: PrefixedStream,
}

pub struct PrefixedStream {
    inner: TcpStream,
    prefix: Vec<u8>,
    off: usize,
}

impl PrefixedStream {
    pub fn into_inner(self) -> TcpStream {
        self.inner
    }

    /// Returns the TCP stream plus any bytes already read past the handshake.
    pub fn into_parts(self) -> (TcpStream, Vec<u8>) {
        let extra = if self.off < self.prefix.len() {
            self.prefix[self.off..].to_vec()
        } else {
            Vec::new()
        };
        (self.inner, extra)
    }
}

impl AsyncWrite for PrefixedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
}

impl AsyncRead for PrefixedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.off < self.prefix.len() {
            let n = std::cmp::min(buf.remaining(), self.prefix.len() - self.off);
            buf.put_slice(&self.prefix[self.off..self.off + n]);
            self.off += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

pub async fn read_client_init(
    stream: TcpStream,
) -> Result<Option<ParsedInit>, (TcpStream, Vec<u8>, String)> {
    read_init_buffer(stream).await
}

async fn read_init_buffer(
    mut stream: TcpStream,
) -> Result<Option<ParsedInit>, (TcpStream, Vec<u8>, String)> {
    let mut buf = vec![0u8; INIT_READ_MAX];
    let mut n = 0usize;

    while n < buf.len() {
        let chunk = timeout(Duration::from_millis(750), stream.read(&mut buf[n..])).await;
        let nn = match chunk {
            Err(_) => break,
            Ok(Ok(0)) => break,
            Ok(Ok(k)) => k,
            Ok(Err(e)) => return Err((stream, buf[..n].to_vec(), e.to_string())),
        };
        n += nn;

        if n > 0 {
            let slice = &buf[..n];
            if is_http_transport(slice) && has_complete_http_headers(slice) {
                return Err(read_full_http_request(stream, slice.to_vec()).await);
            }
            if looks_like_tls_stream(slice) {
                return Err((stream, slice.to_vec(), "tls passthrough".into()));
            }
            if let Some(off) = find_handshake_offset(slice) {
                let rem = buf[off + HANDSHAKE_LEN..n].to_vec();
                let info = parse_direct_handshake(&buf[off..off + HANDSHAKE_LEN]).unwrap();
                return Ok(Some(ParsedInit {
                    info,
                    stream: PrefixedStream {
                        inner: stream,
                        prefix: rem,
                        off: 0,
                    },
                }));
            }
        }
    }

    if n == 0 {
        return Err((stream, Vec::new(), "EOF".into()));
    }
    if n < HANDSHAKE_LEN {
        return Err((stream, buf[..n].to_vec(), format!("short read {n}")));
    }
    Err((stream, buf[..n].to_vec(), "unrecognized handshake".into()))
}

/// MTProto-over-HTTP POST /api sends headers then a body. We must not hand
/// blind_relay a header-only buffer or the upstream waits for Content-Length
/// bytes and the client sees hung emoji/media API calls.
async fn read_full_http_request(
    mut stream: TcpStream,
    mut buf: Vec<u8>,
) -> (TcpStream, Vec<u8>, String) {
    let header_end = match buf.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => i + 4,
        None => {
            return (stream, buf, "http passthrough".into());
        }
    };
    if let Some(content_len) = parse_http_content_length(&buf[..header_end]) {
        let total = header_end.saturating_add(content_len);
        while buf.len() < total && buf.len() < INIT_READ_MAX {
            let need = (total - buf.len()).min(INIT_READ_MAX - buf.len());
            let mut tmp = vec![0u8; need];
            let chunk = timeout(Duration::from_millis(750), stream.read(&mut tmp)).await;
            match chunk {
                Err(_) => break,
                Ok(Ok(0)) => break,
                Ok(Ok(k)) => buf.extend_from_slice(&tmp[..k]),
                Ok(Err(e)) => return (stream, buf, e.to_string()),
            }
        }
    }
    (stream, buf, "http passthrough".into())
}

fn parse_http_content_length(headers: &[u8]) -> Option<usize> {
    let lower: Vec<u8> = headers.iter().map(|b| b.to_ascii_lowercase()).collect();
    for pat in [b"\r\ncontent-length:" as &[u8], b"\ncontent-length:"] {
        let mut search = 0usize;
        while search + pat.len() <= lower.len() {
            let Some(rel) = lower[search..].windows(pat.len()).position(|w| w == pat) else {
                break;
            };
            let start = search + rel + pat.len();
            let mut i = start;
            while i < lower.len() && (lower[i] == b' ' || lower[i] == b'\t') {
                i += 1;
            }
            let val_start = i;
            while i < lower.len() && lower[i].is_ascii_digit() {
                i += 1;
            }
            if i > val_start {
                return std::str::from_utf8(&headers[val_start..i])
                    .ok()?
                    .parse()
                    .ok();
            }
            search = start;
        }
    }
    None
}

fn find_handshake_offset(buf: &[u8]) -> Option<usize> {
    if buf.len() < HANDSHAKE_LEN {
        return None;
    }
    (0..=buf.len() - HANDSHAKE_LEN)
        .find(|&off| parse_direct_handshake(&buf[off..off + HANDSHAKE_LEN]).is_ok())
}

fn has_complete_http_headers(data: &[u8]) -> bool {
    data.windows(4).any(|w| w == b"\r\n\r\n")
}

fn is_http_transport(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    &data[..4] == b"POST"
        || data.len() >= 3 && &data[..3] == b"GET"
        || &data[..4] == b"HEAD"
        || data.len() >= 7 && &data[..7] == b"OPTIONS"
}

pub fn looks_like_tls_stream(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    data[1] == 0x03 && (data[0] == 0x16 || data[0] == 0x17)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_tls_stream_only_at_start() {
        let mut mtproto = vec![0u8; 128];
        mtproto[0] = 0xaf;
        mtproto[50] = 0x16;
        mtproto[51] = 0x03;
        assert!(!looks_like_tls_stream(&mtproto));

        let tls = [0x16, 0x03, 0x01, 0x00, 0x05];
        assert!(looks_like_tls_stream(&tls));
    }

    #[test]
    fn parse_http_content_length_value() {
        let req = b"POST /api HTTP/1.1\r\nHost: 149.154.171.255:80\r\nContent-Length: 176\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\n";
        assert_eq!(parse_http_content_length(req), Some(176));
    }

    #[test]
    fn read_full_http_request_needs_body() {
        let headers =
            b"POST /api HTTP/1.1\r\nHost: 149.154.171.255:80\r\nContent-Length: 8\r\n\r\n";
        let body = b"deadbeef";
        assert_eq!(parse_http_content_length(headers), Some(8));
        let mut full = headers.to_vec();
        full.extend_from_slice(body);
        let header_end = full.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(full.len(), header_end + body.len());
    }
}
