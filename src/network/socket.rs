use anyhow::Result;
use socket2::{Domain, Socket, Type};
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

/// Create a UDP socket with SO_REUSEADDR enabled to allow quick restarts
pub fn create_reuse_socket(bind_addr: &str) -> Result<UdpSocket> {
    let addr: SocketAddr = bind_addr.parse()?;
    
    // Create socket using socket2 for more control
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, None)?;
    
    // Enable address reuse - THIS IS THE KEY FIX
    // Allows the socket to bind to an address in TIME_WAIT state
    socket.set_reuse_address(true)?;
    
    // Also enable reuse port on supported platforms (helps with multiple instances)
    #[cfg(all(unix, not(target_os = "linux")))]
    socket.set_reuse_port(true)?;
    
    // Bind to the address
    socket.bind(&addr.into())?;
    
    // Convert to standard UdpSocket
    let udp_socket: UdpSocket = socket.into();

    // A UDP stream must survive ICMP port-unreachable from endpoints that went
    // away; on Windows that otherwise shows up as WSAECONNRESET on the next read.
    if let Err(e) = crate::platform::udp::disable_udp_conn_reset(&udp_socket) {
        log::warn!("Could not disable SIO_UDP_CONNRESET: {}", e);
    }

    // Set reasonable timeouts
    udp_socket.set_read_timeout(Some(Duration::from_secs(1)))?;

    Ok(udp_socket)
}

/// Create a UDP socket for broadcasting
pub fn create_broadcast_socket() -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
    
    // Enable broadcast
    socket.set_broadcast(true)?;
    
    // Bind to any available port
    socket.bind(&"0.0.0.0:0".parse::<SocketAddr>()?.into())?;
    
    Ok(socket.into())
}

/// Create a UDP socket for listening on a specific port with reuse
pub fn create_listener_socket(port: u16) -> Result<UdpSocket> {
    let addr = format!("0.0.0.0:{}", port);
    create_reuse_socket(&addr)
}

/// Create the socket a client receives on.
///
/// One socket per process, reused for every reconnection attempt: the server
/// identifies a client by its source endpoint, so keeping the endpoint stable is
/// what stops it from accumulating one stale entry per reconnect.
pub fn create_client_socket() -> Result<UdpSocket> {
    create_reuse_socket("0.0.0.0:0")
}

/// Wait for a port to be released (with timeout)
pub fn wait_for_port_release(port: u16, timeout_ms: u64) -> bool {
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    
    while start.elapsed() < timeout {
        // Try to create a test socket
        match create_listener_socket(port) {
            Ok(socket) => {
                // Success! Port is free
                drop(socket);
                return true;
            }
            Err(_) => {
                // Port still in use, wait a bit
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    
    false
}

/// Force release a port by creating a socket with SO_REUSEADDR
pub fn force_release_port(port: u16) -> Result<()> {
    // Just creating and immediately dropping a socket with SO_REUSEADDR
    // can help release the port from TIME_WAIT in some cases
    let socket = create_listener_socket(port)?;
    drop(socket);
    Ok(())
}
