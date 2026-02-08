use anyhow::Result;
use log::{error, info, warn};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use std::time::{Duration, Instant};

use super::protocol::{AudioPacket, STREAM_PORT};

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
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        
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
        let reconnect_interval = Duration::from_secs(3);
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
                    
                    warn!("Reconnecting in {} seconds...", reconnect_interval.as_secs());
                    std::thread::sleep(reconnect_interval);
                    continue; // Retry connection
                }
            }
        }
    }
    
    /// Try to connect and receive audio (single attempt)
    fn try_connect(&self, server_addr: SocketAddr) -> Result<()> {
        // Reset connection state
        self.connection_state.store(false, Ordering::SeqCst);
        
        // Create a new socket for this connection attempt (to clear any bad state)
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        
        info!("Connecting to server at {}...", server_addr);
        
        // Send initial connection packet
        if let Err(e) = socket.connect(server_addr) {
            anyhow::bail!("Failed to connect socket to {}: {}", server_addr, e);
        }
        
        info!("Sending connection handshake to server...");
        match socket.send(&[0x01]) {
            Ok(n) => info!("Sent handshake ({} bytes)", n),
            Err(e) => warn!("Failed to send handshake: {} (server may not be ready yet)", e),
        }
        
        // Set read timeout for initial connection (shorter for faster detection)
        if let Err(e) = socket.set_read_timeout(Some(Duration::from_secs(3))) {
            warn!("Could not set read timeout: {}", e);
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
        let mut first_packet = true;
        let mut last_data_received = Instant::now();
        let data_timeout = Duration::from_secs(5); // Timeout for data reception
        let connection_timeout = Duration::from_secs(10); // Timeout for initial connection
        let start_time = Instant::now();
        
        loop {
            match socket.recv(&mut buf) {
                Ok(len) => {
                    if first_packet {
                        info!("✅ First audio packet received! Connection established.");
                        first_packet = false;
                        // Mark as connected only after first successful packet
                        self.connection_state.store(true, Ordering::SeqCst);
                        
                        // Switch to longer timeout for normal operation
                        if let Err(e) = socket.set_read_timeout(Some(data_timeout)) {
                            warn!("Could not update read timeout: {}", e);
                        }
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
                    // Check if we haven't received any data yet (initial connection phase)
                    if first_packet {
                        if start_time.elapsed() > connection_timeout {
                            anyhow::bail!("Connection timeout: No data received from server within 10 seconds");
                        }
                        
                        warn!("⏳ Waiting for server response... (attempting handshake retry)");
                        
                        // Retry handshake in case server missed it
                        if let Err(e) = socket.send(&[0x01]) {
                            warn!("Retry handshake failed: {}", e);
                        }
                        continue;
                    }
                    
                    // We were connected but now timed out
                    if last_data_received.elapsed() > data_timeout {
                        anyhow::bail!("Data timeout: No packets received for {} seconds", data_timeout.as_secs());
                    }
                    
                    continue;
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
}
