use std::io;
use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};

pub async fn listen_transparent(addr: &str) -> io::Result<TcpListener> {
    let addr: SocketAddr = addr.parse().map_err(io::Error::other)?;
    TcpListener::bind(addr).await
}

pub fn get_original_dst(_stream: &TcpStream) -> io::Result<(String, u16)> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "SO_ORIGINAL_DST is only supported on Linux",
    ))
}
