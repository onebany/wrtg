use std::io;
use std::net::SocketAddr;
use std::os::fd::AsRawFd;

use socket2::{Domain, Socket, Type};
use tokio::net::{TcpListener, TcpStream};

const IP_TRANSPARENT: libc::c_int = 19;
const SO_ORIGINAL_DST: libc::c_int = 80;

pub async fn listen_transparent(addr: &str) -> io::Result<TcpListener> {
    let addr: SocketAddr = addr.parse().map_err(io::Error::other)?;
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };

    let socket = Socket::new(domain, Type::STREAM, None)?;
    socket.set_reuse_address(true)?;

    let fd = socket.as_raw_fd();
    let one: libc::c_int = 1;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IP,
            IP_TRANSPARENT,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of_val(&one) as libc::socklen_t,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    socket.bind(&addr.into())?;
    socket.listen(128)?;
    let listener: std::net::TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    TcpListener::from_std(listener)
}

#[repr(C)]
struct sockaddr_in {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

pub fn get_original_dst(stream: &TcpStream) -> io::Result<(String, u16)> {
    let sock_fd = stream.as_raw_fd();

    let mut addr = sockaddr_in {
        sin_family: 0,
        sin_port: 0,
        sin_addr: [0; 4],
        sin_zero: [0; 8],
    };
    let mut addr_len = std::mem::size_of::<sockaddr_in>() as libc::socklen_t;

    let ret = unsafe {
        libc::getsockopt(
            sock_fd,
            libc::IPPROTO_IP,
            SO_ORIGINAL_DST,
            &mut addr as *mut _ as *mut libc::c_void,
            &mut addr_len,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }

    let port = u16::from_be(addr.sin_port);
    let ip = format!(
        "{}.{}.{}.{}",
        addr.sin_addr[0], addr.sin_addr[1], addr.sin_addr[2], addr.sin_addr[3]
    );
    Ok((ip, port))
}
