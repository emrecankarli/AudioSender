use anyhow::Result;
use log::{info, warn};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::{capture::AudioCapture, codec::OpusEncoder};
use crate::config::Config;
use crate::network::clients::{ClientRegistry, Touch, MAX_CLIENTS};
use crate::network::discovery::DiscoveryService;
use crate::network::protocol::{AudioPacket, STREAM_PORT};
use crate::network::socket::create_listener_socket;

/// A client that stops sending keep-alives for this long is dropped.
///
/// Clients re-send their handshake every second, so this tolerates four lost
/// keep-alives before the endpoint is considered gone.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

/// Run the server (audio source)
pub fn run_server(config: Config, device_name: Option<String>) -> Result<()> {
    info!("Initializing server: {}", config.server_name);
    
    // Start discovery broadcast
    DiscoveryService::start_server_broadcast(config.server_name.clone())?;
    
    // Create audio capture
    let capture = AudioCapture::new(device_name)?;
    
    // Create Opus encoder
    let mut encoder = OpusEncoder::new()?;
    
    // Channels for audio pipeline
    let (frame_tx, frame_rx) = channel();
    let (packet_tx, packet_rx) = channel();
    
    // Start audio capture
    let _capture_stream = capture.start(frame_tx)?;
    
    // Encoder thread
    thread::spawn(move || {
        while let Ok(frame) = frame_rx.recv() {
            match encoder.encode(&frame) {
                Ok(encoded) => {
                    if packet_tx.send((encoded, frame.timestamp)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log::error!("Encoding error: {}", e);
                }
            }
        }
    });
    
    // Create UDP socket with SO_REUSEADDR for quick restarts
    let socket = match create_listener_socket(STREAM_PORT) {
        Ok(s) => {
            info!("Streaming server bound to port {} (with SO_REUSEADDR)", STREAM_PORT);
            Arc::new(s)
        }
        Err(e) => {
            anyhow::bail!("Failed to bind streaming socket on port {}: {}. \
                Try waiting a few seconds before restarting, or restart the computer.", 
                STREAM_PORT, e);
        }
    };
    
    socket.set_nonblocking(true)?;
    
    // Track connected clients. Keyed by host, with a liveness timer: see
    // `network::clients` for why an unbounded set here is a bandwidth bug.
    let clients: Arc<Mutex<ClientRegistry>> =
        Arc::new(Mutex::new(ClientRegistry::new(CLIENT_TIMEOUT)));
    let clients_clone = clients.clone();
    let socket_clone = socket.clone();
    
    // Client connection listener thread
    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        loop {
            match socket_clone.recv_from(&mut buf) {
                Ok((len, addr)) => {
                    // Clients send this byte on connect and then once a second as a
                    // keep-alive, so it doubles as the liveness signal.
                    if len == 1 && buf[0] == 0x01 {
                        let mut clients = clients_clone.lock().unwrap();
                        match clients.touch(addr, Instant::now()) {
                            Touch::New => info!(
                                "New client connected: {} ({} active)",
                                addr,
                                clients.len()
                            ),
                            Touch::Moved { from } => info!(
                                "Client {} reconnected on {} ({} active)",
                                from.ip(),
                                addr,
                                clients.len()
                            ),
                            Touch::Refreshed => log::debug!("Keep-alive from {}", addr),
                            Touch::Rejected => warn!(
                                "Rejected client {}: already at the {} client limit",
                                addr, MAX_CLIENTS
                            ),
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    // A dead peer's ICMP port-unreachable can surface here; it says
                    // nothing about the health of this socket, so keep serving.
                    warn!("Error receiving connection: {}", e);
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    });
    
    // Streaming loop - send packets to all connected clients using THE SAME SOCKET
    let mut sequence = 0u32;
    
    info!("Server is running. Press Ctrl+C to stop.");
    info!("Waiting for client connections on port {}...", STREAM_PORT);
    
    // Statistics
    let mut last_log = Instant::now();
    let mut packets_sent = 0;
    
    while let Ok((payload, timestamp)) = packet_rx.recv() {
        let packet = AudioPacket::new(sequence, timestamp, payload);
        let bytes = packet.to_bytes();
        
        // Drop endpoints that stopped sending keep-alives, then snapshot the list
        // so the lock is not held while sending.
        let clients_list = {
            let mut clients = clients.lock().unwrap();
            let removed = clients.prune(Instant::now());
            if removed > 0 {
                info!(
                    "Removed {} stale client(s), {} still active",
                    removed,
                    clients.len()
                );
            }
            clients.active()
        };
        
        // Log stats every 5 seconds
        if last_log.elapsed().as_secs() >= 5 {
             let client_count = clients_list.len();
             if client_count > 0 {
                 info!("Stats: Sent {} packets to {} client(s) in last 5s", packets_sent, client_count);
             } else if packets_sent > 0 {
                 info!("Stats: Generated {} packets (no clients connected)", packets_sent);
             }
             packets_sent = 0;
             last_log = Instant::now();
        }

        for client_addr in clients_list.iter() {
            // CRITICAL FIX: Use the SAME socket (port 59200) to send data
            // so the client receives from the expected source port
            if let Err(e) = socket.send_to(&bytes, client_addr) {
                warn!("Failed to send to {}: {}", client_addr, e);
            }
        }
        
        packets_sent += 1;
        sequence = sequence.wrapping_add(1);
    }
    
    Ok(())
}
