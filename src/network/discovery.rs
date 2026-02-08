use anyhow::Result;
use log::{error, info, warn};
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::collections::HashMap;

use super::protocol::{DiscoveryMessage, DISCOVERY_PORT};
use super::socket::create_listener_socket;

/// Server information for discovery
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub address: SocketAddr,
    pub last_seen: Instant,
}

/// Discovery service for auto-connecting clients to servers
pub struct DiscoveryService {
    servers: Arc<Mutex<HashMap<String, ServerInfo>>>,
}

impl DiscoveryService {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    /// Start broadcasting as a server
    pub fn start_server_broadcast(server_name: String) -> Result<()> {
        thread::spawn(move || {
            // Try to bind to any available port for sending broadcasts
            let socket = match UdpSocket::bind("0.0.0.0:0") {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create broadcast socket: {}", e);
                    return;
                }
            };
            
            if let Err(e) = socket.set_broadcast(true) {
                error!("Failed to enable broadcast: {}", e);
                return;
            }
            
            // Also allow address reuse for better compatibility
            if let Err(e) = socket.set_read_timeout(Some(Duration::from_millis(100))) {
                warn!("Could not set socket timeout: {}", e);
            }
            
            let message = DiscoveryMessage {
                server_name: server_name.clone(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            };
            
            info!("Starting discovery broadcast for server: {}", server_name);
            info!("Broadcasting on port {} (make sure firewall allows UDP)", DISCOVERY_PORT);
            
            loop {
                let json = match serde_json::to_string(&message) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize discovery message: {}", e);
                        thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                };
                
                // Try global broadcast
                let broadcast_addr = format!("255.255.255.255:{}", DISCOVERY_PORT);
                if let Err(e) = socket.send_to(json.as_bytes(), &broadcast_addr) {
                    warn!("Failed to send broadcast to {}: {}", broadcast_addr, e);
                } else {
                    log::debug!("Sent discovery broadcast to {}", broadcast_addr);
                }
                
                thread::sleep(Duration::from_secs(2));
            }
        });
        
        Ok(())
    }
    
    /// Start listening for server broadcasts (client mode)
    pub fn start_client_listen(&self) -> Result<()> {
        let servers = self.servers.clone();
        
        thread::spawn(move || {
            // Use SO_REUSEADDR socket to allow quick restarts
            let socket = match create_listener_socket(DISCOVERY_PORT) {
                Ok(s) => {
                    info!("Discovery listener bound to port {}", DISCOVERY_PORT);
                    s
                }
                Err(e) => {
                    error!("Failed to bind discovery socket on port {}: {}", DISCOVERY_PORT, e);
                    error!("Make sure no other instance is running and port is not blocked by firewall");
                    return;
                }
            };
            
            // Set a read timeout so we can periodically check for shutdown
            if let Err(e) = socket.set_read_timeout(Some(Duration::from_secs(1))) {
                warn!("Could not set socket read timeout: {}", e);
            }
            
            info!("Listening for server broadcasts on port {}...", DISCOVERY_PORT);
            
            let mut buf = [0u8; 1024];
            loop {
                match socket.recv_from(&mut buf) {
                    Ok((len, addr)) => {
                        let json = match std::str::from_utf8(&buf[..len]) {
                            Ok(s) => s,
                            Err(e) => {
                                warn!("Invalid UTF-8 in discovery message from {}: {}", addr, e);
                                continue;
                            }
                        };
                        
                        match serde_json::from_str::<DiscoveryMessage>(json) {
                            Ok(msg) => {
                                let server_info = ServerInfo {
                                    name: msg.server_name.clone(),
                                    address: addr,
                                    last_seen: Instant::now(),
                                };
                                
                                let mut servers = servers.lock().unwrap();
                                let is_new = !servers.contains_key(&msg.server_name);
                                servers.insert(msg.server_name.clone(), server_info);
                                
                                if is_new {
                                    info!("🎯 Discovered new server '{}' at {}", msg.server_name, addr);
                                } else {
                                    log::debug!("Updated server '{}' at {}", msg.server_name, addr);
                                }
                            }
                            Err(e) => {
                                warn!("Failed to parse discovery message from {}: {}", addr, e);
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut 
                               || e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Timeout is expected, continue loop
                        continue;
                    }
                    Err(e) => {
                        error!("Failed to receive discovery message: {}", e);
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        });
        
        // Start cleanup thread to remove stale servers
        let servers = self.servers.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(5));
                
                let mut servers = servers.lock().unwrap();
                let before_count = servers.len();
                servers.retain(|_, info| {
                    info.last_seen.elapsed() < Duration::from_secs(10)
                });
                let after_count = servers.len();
                
                if after_count < before_count {
                    info!("Removed {} stale server(s) from discovery list", before_count - after_count);
                }
            }
        });
        
        Ok(())
    }
    
    /// Get list of discovered servers
    pub fn get_servers(&self) -> Vec<ServerInfo> {
        let servers = self.servers.lock().unwrap();
        servers.values().cloned().collect()
    }
}
