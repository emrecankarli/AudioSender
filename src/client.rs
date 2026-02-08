use anyhow::Result;
use log::{info, warn};
use std::net::SocketAddr;
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::{codec::OpusDecoder, playback::AudioPlayback};
use crate::config::Config;
use crate::network::discovery::DiscoveryService;
use crate::network::streaming::StreamingClient;

/// Run the client (audio receiver)
pub fn run_client(config: Config, connect_to: Option<String>) -> Result<()> {
    info!("Initializing client");
    
    let server_addr = if let Some(ip) = connect_to {
        // Direct connection to specified IP
        let addr_str = format!("{}:59200", ip);
        let addr: SocketAddr = match addr_str.parse() {
            Ok(a) => a,
            Err(e) => anyhow::bail!("Invalid server address '{}': {}", addr_str, e),
        };
        info!("Connecting directly to {}", addr);
        info!("Make sure the server is running at {} and firewall allows UDP port 59200", ip);
        addr
    } else {
        // Start discovery
        let discovery = DiscoveryService::new();
        discovery.start_client_listen()?;
        
        info!("Discovering servers on the local network...");
        info!("Make sure the server is running on the same network");
        
        // Wait for discovery with progress indication
        let start = Instant::now();
        let server = loop {
            thread::sleep(Duration::from_millis(500));
            
            let elapsed = start.elapsed().as_secs();
            if elapsed % 3 == 0 && elapsed > 0 {
                info!("Still searching... ({}s)", elapsed);
            }
            
            let servers = discovery.get_servers();
            if !servers.is_empty() {
                break servers.into_iter().next().unwrap();
            }
            
            if elapsed > 30 {
                anyhow::bail!("No servers found after 30 seconds. Use --connect <IP> to connect directly.");
            }
        };
        
        info!("Found server: '{}' at {}", server.name, server.address);
        
        // Use server address but with stream port
        let mut addr = server.address;
        addr.set_port(59200);
        info!("Will connect to streaming port: {}", addr);
        addr
    };
    
    // Create audio playback
    let playback = AudioPlayback::new()?;
    
    // Create Opus decoder
    let mut decoder = OpusDecoder::new()?;
    
    // Channels for audio pipeline
    let (packet_tx, packet_rx) = channel::<crate::network::protocol::AudioPacket>();
    let (frame_tx, frame_rx) = channel::<crate::audio::types::AudioFrame>();
    
    // Start playback
    let _playback_stream = playback.start(frame_rx)?;
    info!("Audio playback started");
    
    // Decoder thread with statistics
    let decoder_thread = thread::spawn(move || {
        let mut frames_decoded = 0;
        let mut decode_errors = 0;
        let mut last_log = Instant::now();
        
        while let Ok(packet) = packet_rx.recv() {
            match decoder.decode(&packet.payload, packet.timestamp) {
                Ok(frame) => {
                    frames_decoded += 1;
                    
                    // Log stats every 5 seconds
                    if last_log.elapsed().as_secs() >= 5 {
                        if decode_errors > 0 {
                            info!("Decoder stats: {} frames decoded, {} errors in last 5s", 
                                  frames_decoded, decode_errors);
                        } else {
                            info!("✓ Decoder stats: {} frames decoded successfully in last 5s", 
                                  frames_decoded);
                        }
                        frames_decoded = 0;
                        decode_errors = 0;
                        last_log = Instant::now();
                    }
                    
                    if frame_tx.send(frame).is_err() {
                        warn!("Playback channel closed, stopping decoder");
                        break;
                    }
                }
                Err(e) => {
                    decode_errors += 1;
                    if decode_errors <= 5 || decode_errors % 100 == 0 {
                        log::error!("Decoding error: {}", e);
                    }
                }
            }
        }
        
        info!("Decoder thread ending. Total decoded: {}, errors: {}", frames_decoded, decode_errors);
    });
    
    // Streaming client
    let client = StreamingClient::new(packet_tx)?;
    
    info!("Starting audio stream...");
    match client.connect_and_receive(server_addr) {
        Ok(()) => info!("Streaming ended normally"),
        Err(e) => warn!("Streaming error: {}", e),
    }
    
    // Wait a bit for graceful shutdown
    thread::sleep(Duration::from_millis(100));
    
    // Clean up decoder thread
    drop(client);
    
    Ok(())
}
