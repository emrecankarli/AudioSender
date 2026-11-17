use anyhow::Result;
use log::{error, info, warn};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use std::time::{Duration, Instant};

use super::protocol::{AudioPacket, STREAM_PORT};
use super::socket::create_client_socket;

/// Handshake byte. The server registers the sender on the first one and treats
/// every repeat as a keep-alive that refreshes its liveness timer.
const HANDSHAKE: u8 = 0x01;

/// How often a connected client proves that it is still there.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);

/// Read timeout, i.e. how often keep-alives and timeouts are checked while no
/// audio is arriving.
const READ_POLL: Duration = Duration::from_millis(250);

/// How long a handshake may go unanswered before the attempt fails.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a stream may go quiet before the attempt fails.
const DATA_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause between connection attempts.
const RECONNECT_DELAY: Duration = Duration::from_secs(3);

/// Connection state for the client
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Reconnecting,
}

/// Audio streaming server (sends audio to clients)
pub struct StreamingServer {
    socket: UdpSocket,
    sequence: u32,
}

impl StreamingServer {
    /// Create a new streaming server
    pub fn new() -> Result<Self> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", STREAM_PORT))?;
        socket.set_nonblocking(false)?;
        
        info!("Streaming server bound to port {}", STREAM_PORT);
        
        Ok(Self {
            socket,
            sequence: 0,
        })
    }
    
    /// Start streaming encoded audio packets to a client
    pub fn start_streaming(
        &mut self,
        client_addr: SocketAddr,
        packet_receiver: Receiver<(Vec<u8>, u64)>,
    ) -> Result<()> {
        info!("Starting stream to client: {}", client_addr);
        
        loop {
            match packet_receiver.recv() {
                Ok((payload, timestamp)) => {
                    let packet = AudioPacket::new(self.sequence, timestamp, payload);
                    let bytes = packet.to_bytes();
                    
                    if let Err(e) = self.socket.send_to(&bytes, client_addr) {
                        error!("Failed to send packet to {}: {}", client_addr, e);
                    }
                    
                    self.sequence = self.sequence.wrapping_add(1);
                }
                Err(e) => {
                    error!("Packet receiver error: {}", e);
                    break;
                }
            }
        }
        
        Ok(())
    }
}

/// Audio streaming client (receives audio from server)
pub struct StreamingClient {
    socket: UdpSocket,
    packet_sender: Sender<AudioPacket>,
    connection_state: Arc<AtomicBool>, // true = connected, false = not connected
}

impl StreamingClient {
    /// Create a new streaming client
    pub fn new(packet_sender: Sender<AudioPacket>) -> Result<Self> {
        // One socket for the lifetime of the process, on an ephemeral port.
        //
        // This used to bind a brand new socket inside every connection attempt,
        // i.e. a new source port on every reconnect. The server registers clients
        // by source endpoint, so each retry left the previous port behind - still
        // receiving (and paying for) every audio frame. Keeping the endpoint
        // stable is what makes that table bounded.
        let socket = create_client_socket()?;
        socket.set_read_timeout(Some(READ_POLL))?;

        Ok(Self {
            socket,
            packet_sender,
            connection_state: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get connection state reference (for checking from outside)
    pub fn get_connection_state(&self) -> Arc<AtomicBool> {
        self.connection_state.clone()
    }

    /// Connect to a server with auto-retry logic
    pub fn connect_and_receive(&self, server_addr: SocketAddr) -> Result<()> {
        let mut attempt = 0;

        loop {
            attempt += 1;
            info!("Connection attempt #{} to server at {}...", attempt, server_addr);

            match self.try_connect(server_addr) {
                Ok(()) => {
                    info!("Connection closed gracefully");
                    return Ok(());
                }
                Err(e) => {
                    error!("Connection lost: {}", e);
                    self.connection_state.store(false, Ordering::SeqCst);

                    warn!("Reconnecting in {} seconds...", RECONNECT_DELAY.as_secs());
                    std::thread::sleep(RECONNECT_DELAY);
                    continue; // Retry connection
                }
            }
        }
    }

    /// Try to connect and receive audio (single attempt)
    fn try_connect(&self, server_addr: SocketAddr) -> Result<()> {
        // Reset connection state
        self.connection_state.store(false, Ordering::SeqCst);

        info!("Connecting to server at {}...", server_addr);

        // Re-connecting an already connected UDP socket only retargets the peer,
        // which is exactly what a retry needs: no rebind, so the source port - and
        // therefore the server's registration - stays the same.
        if let Err(e) = self.socket.connect(server_addr) {
            anyhow::bail!("Failed to connect socket to {}: {}", server_addr, e);
        }

        info!("Sending connection handshake to server...");
        if let Err(e) = self.send_handshake() {
            warn!("Failed to send handshake: {} (server may not be ready yet)", e);
        }

        info!("Waiting for audio packets from {}...", server_addr);
        info!("⚠️  If no audio received within 5 seconds, check:");
        info!("   1. Server is running and firewall allows UDP port {}", STREAM_PORT);
        info!("   2. Both computers are on the same network");
        info!("   3. No VPN is blocking the connection");

        let mut buf = vec![0u8; 2048];
        let mut last_sequence = 0u32;
        let mut packets_received = 0u32;
        let mut last_log = Instant::now();
        let mut connected = false;
        let mut last_data_received = Instant::now();
        let mut last_handshake = Instant::now();
        let connect_started = Instant::now();

        loop {
            match self.socket.recv(&mut buf) {
                Ok(len) => {
                    if !connected {
                        info!("✅ First audio packet received! Connection established.");
                        connected = true;
                        // Mark as connected only after first successful packet
                        self.connection_state.store(true, Ordering::SeqCst);
                    }

                    last_data_received = Instant::now();

                    if let Some(packet) = AudioPacket::from_bytes(&buf[..len]) {
                        // Check for packet loss
                        let expected_seq = last_sequence.wrapping_add(1);
                        if last_sequence != 0 && packet.sequence != expected_seq {
                            let lost = packet.sequence.wrapping_sub(expected_seq);
                            warn!("Packet loss detected: {} packets", lost);
                        }
                        last_sequence = packet.sequence;

                        packets_received += 1;

                        // Log stats every 5 seconds
                        if last_log.elapsed().as_secs() >= 5 {
                            info!("Client stats: Received {} packets in last 5s", packets_received);
                            packets_received = 0;
                            last_log = Instant::now();
                        }

                        if self.packet_sender.send(packet).is_err() {
                            error!("Failed to send packet to decoder - receiver disconnected");
                            self.connection_state.store(false, Ordering::SeqCst);
                            break;
                        }
                    } else {
                        warn!("Received invalid packet ({} bytes)", len);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock
                           || e.kind() == std::io::ErrorKind::TimedOut => {
                    // Idle tick of the short read timeout. A connected client sends
                    // no data of its own, so this keep-alive is what keeps the
                    // server's liveness timer fresh; without it the server would
                    // evict a perfectly healthy client.
                    if last_handshake.elapsed() >= KEEPALIVE_INTERVAL {
                        if let Err(e) = self.send_handshake() {
                            warn!("Keep-alive to {} failed: {}", server_addr, e);
                        }
                        last_handshake = Instant::now();
                    }

                    if !connected {
                        if connect_started.elapsed() > CONNECT_TIMEOUT {
                            anyhow::bail!(
                                "Connection timeout: no data received from server within {} seconds",
                                CONNECT_TIMEOUT.as_secs()
                            );
                        }
                        log::debug!("⏳ Waiting for server response...");
                        continue;
                    }

                    // We were connected but now timed out
                    if last_data_received.elapsed() > DATA_TIMEOUT {
                        anyhow::bail!(
                            "Data timeout: No packets received for {} seconds",
                            DATA_TIMEOUT.as_secs()
                        );
                    }
                }
                Err(e) => {
                    error!("Socket error: {}", e);
                    self.connection_state.store(false, Ordering::SeqCst);
                    anyhow::bail!("Connection error: {}", e);
                }
            }
        }

        self.connection_state.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Send the handshake byte. The server treats a repeat of it as a keep-alive.
    fn send_handshake(&self) -> Result<()> {
        match self.socket.send(&[HANDSHAKE]) {
            Ok(_) => Ok(()),
            Err(e) => anyhow::bail!("{}", e),
        }
    }
}
