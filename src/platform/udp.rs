//! WinSock-level socket tweaks that `std::net` does not expose.

use std::io;

/// Turn off `SIO_UDP_CONNRESET` (which Windows enables by default).
///
/// On Windows, sending a datagram to a port nobody listens on makes the peer's
/// ICMP port-unreachable surface as `WSAECONNRESET` (10054) on the *next*
/// `recv`/`send` on the same socket. For a UDP stream that is pure noise: one
/// dead client endpoint can tear down the receive loop of an otherwise healthy
/// stream. Disabling the behaviour is the documented fix and costs one ioctl.
///
/// Failure is not fatal — the caller keeps the socket as-is and only warns — but
/// on Windows it should never fail for a plain UDP socket.
#[cfg(windows)]
pub fn disable_udp_conn_reset(socket: &std::net::UdpSocket) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{WSAIoctl, SOCKET};

    /// `_WSAIOW(IOC_VENDOR, 12)` — not exposed as a constant by the windows crate.
    const SIO_UDP_CONNRESET: u32 = 0x9800_000C;

    let disable: u32 = 0;
    let mut bytes_returned: u32 = 0;

    let result = unsafe {
        WSAIoctl(
            SOCKET(socket.as_raw_socket() as usize),
            SIO_UDP_CONNRESET,
            Some(&disable as *const u32 as *const core::ffi::c_void),
            std::mem::size_of::<u32>() as u32,
            None,
            0,
            &mut bytes_returned,
            None,
            None,
        )
    };

    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Other platforms have no equivalent socket state, so nothing to do.
#[cfg(not(windows))]
pub fn disable_udp_conn_reset(_socket: &std::net::UdpSocket) -> io::Result<()> {
    Ok(())
}
