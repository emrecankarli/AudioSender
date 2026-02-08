use anyhow::Result;
use opus::{Application, Channels, Decoder, Encoder};

use super::types::{AudioFrame, SAMPLE_RATE, CHANNELS, FRAME_SIZE};

const BITRATE: i32 = 160000; // 160 kbps (Higher quality)
const GAIN: f32 = 0.5;         // User set to 1.0

/// Opus encoder for audio compression
pub struct OpusEncoder {
    encoder: Encoder,
}

impl OpusEncoder {
    /// Create a new Opus encoder
    pub fn new() -> Result<Self> {
        let mut encoder = Encoder::new(
            SAMPLE_RATE,
            Channels::Stereo,
            Application::Audio, // Optimized for music/high fidelity
        )?;
        
        encoder.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
        
        Ok(Self { encoder })
    }
    
    /// Encode an audio frame to Opus format
    pub fn encode(&mut self, frame: &AudioFrame) -> Result<Vec<u8>> {
        // Convert f32 samples to i16 with soft limiting
        let samples_i16: Vec<i16> = frame
            .samples
            .iter()
            .map(|&s| (s * GAIN * 32768.0).clamp(-32768.0, 32767.0) as i16)
            .collect();
        
        let mut output = vec![0u8; 4000]; // Max Opus frame size
        let len = self.encoder.encode(&samples_i16, &mut output)?;
        output.truncate(len);
        
        Ok(output)
    }
}

/// Opus decoder for audio decompression
pub struct OpusDecoder {
    decoder: Decoder,
}

impl OpusDecoder {
    /// Create a new Opus decoder
    pub fn new() -> Result<Self> {
        let decoder = Decoder::new(SAMPLE_RATE, Channels::Stereo)?;
        
        Ok(Self { decoder })
    }
    
    /// Decode Opus data to an audio frame
    pub fn decode(&mut self, data: &[u8], timestamp: u64) -> Result<AudioFrame> {
        let mut output = vec![0i16; FRAME_SIZE * CHANNELS as usize];
        let len = self.decoder.decode(data, &mut output, false)?;
        
        // CRITICAL FIX: len is samples PER CHANNEL. Total is len * CHANNELS.
        let total_samples = len * CHANNELS as usize;
        
        // Convert i16 samples to f32 (proper conversion: divide by 32768.0)
        let samples: Vec<f32> = output[..total_samples]
            .iter()
            .map(|&s| s as f32 / 32768.0)
            .collect();
        
        Ok(AudioFrame::new(samples, timestamp))
    }
}
