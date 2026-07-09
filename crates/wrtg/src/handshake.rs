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
                return Err((stream, slice.to_vec(), "http passthrough".into()));
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
}
