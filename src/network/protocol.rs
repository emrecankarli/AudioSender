use serde::{Deserialize, Serialize};

pub const DISCOVERY_PORT: u16 = 59100;
pub const STREAM_PORT: u16 = 59200;
pub const MAX_PACKET_SIZE: usize = 1400; // MTU-safe

/// Discovery message sent by servers
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DiscoveryMessage {
    pub server_name: String,
    pub version: String,
}

/// Audio packet format for UDP streaming
#[repr(C)]
#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub sequence: u32,
    pub timestamp: u64,
    pub payload_size: u16,
    pub payload: Vec<u8>,
}

impl AudioPacket {
    pub fn new(sequence: u32, timestamp: u64, payload: Vec<u8>) -> Self {
        let payload_size = payload.len() as u16;
        Self {
            sequence,
            timestamp,
            payload_size,
            payload,
        }
    }
    
    /// Serialize packet to bytes for transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(14 + self.payload.len());
        bytes.extend(&self.sequence.to_le_bytes());
        bytes.extend(&self.timestamp.to_le_bytes());
        bytes.extend(&self.payload_size.to_le_bytes());
        bytes.extend(&self.payload);
        bytes
    }
    
    /// Deserialize packet from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 14 {
            return None;
        }
        
        let sequence = u32::from_le_bytes(data[0..4].try_into().ok()?);
        let timestamp = u64::from_le_bytes(data[4..12].try_into().ok()?);
        let payload_size = u16::from_le_bytes(data[12..14].try_into().ok()?);
        
        if data.len() < 14 + payload_size as usize {
            return None;
        }
        
        let payload = data[14..14 + payload_size as usize].to_vec();
        
        Some(Self {
            sequence,
            timestamp,
            payload_size,
            payload,
        })
    }
}
