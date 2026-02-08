use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared application state between GUI and background threads
#[derive(Clone)]
pub struct SharedState {
    /// Current audio level (0.0 - 1.0) for visualization
    pub audio_level: Arc<Mutex<f32>>,
    
    /// Connection status message
    pub status_message: Arc<Mutex<String>>,
    
    /// Whether currently connected/streaming
    pub is_connected: Arc<AtomicBool>,
    
    /// Current latency in milliseconds
    pub latency_ms: Arc<Mutex<u32>>,
    
    /// Packets per second
    pub packets_per_sec: Arc<Mutex<u32>>,
    
    /// Bytes transferred (for stats)
    pub bytes_transferred: Arc<Mutex<u64>>,
    
    /// Error message (if any)
    pub error_message: Arc<Mutex<Option<String>>>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            audio_level: Arc::new(Mutex::new(0.0)),
            status_message: Arc::new(Mutex::new("Ready".to_string())),
            is_connected: Arc::new(AtomicBool::new(false)),
            latency_ms: Arc::new(Mutex::new(0)),
            packets_per_sec: Arc::new(Mutex::new(0)),
            bytes_transferred: Arc::new(Mutex::new(0)),
            error_message: Arc::new(Mutex::new(None)),
        }
    }
}

impl SharedState {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn set_audio_level(&self, level: f32) {
        if let Ok(mut guard) = self.audio_level.lock() {
            *guard = level.clamp(0.0, 1.0);
        }
    }
    
    pub fn get_audio_level(&self) -> f32 {
        self.audio_level.lock().map(|g| *g).unwrap_or(0.0)
    }
    
    pub fn set_status(&self, status: &str) {
        if let Ok(mut guard) = self.status_message.lock() {
            *guard = status.to_string();
        }
    }
    
    pub fn get_status(&self) -> String {
        self.status_message.lock().map(|g| g.clone()).unwrap_or_default()
    }
    
    pub fn set_connected(&self, connected: bool) {
        self.is_connected.store(connected, Ordering::SeqCst);
    }
    
    pub fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst)
    }
    
    pub fn set_latency(&self, ms: u32) {
        if let Ok(mut guard) = self.latency_ms.lock() {
            *guard = ms;
        }
    }
    
    pub fn get_latency(&self) -> u32 {
        self.latency_ms.lock().map(|g| *g).unwrap_or(0)
    }
    
    pub fn set_packets_per_sec(&self, pps: u32) {
        if let Ok(mut guard) = self.packets_per_sec.lock() {
            *guard = pps;
        }
    }
    
    pub fn get_packets_per_sec(&self) -> u32 {
        self.packets_per_sec.lock().map(|g| *g).unwrap_or(0)
    }
    
    pub fn add_bytes(&self, bytes: u64) {
        if let Ok(mut guard) = self.bytes_transferred.lock() {
            *guard += bytes;
        }
    }
    
    pub fn get_bytes_transferred(&self) -> u64 {
        self.bytes_transferred.lock().map(|g| *g).unwrap_or(0)
    }
    
    pub fn set_error(&self, error: Option<String>) {
        if let Ok(mut guard) = self.error_message.lock() {
            *guard = error;
        }
    }
    
    pub fn get_error(&self) -> Option<String> {
        self.error_message.lock().map(|g| g.clone()).unwrap_or(None)
    }
    
    pub fn clear_error(&self) {
        self.set_error(None);
    }
}
