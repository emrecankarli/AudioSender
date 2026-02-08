/// Audio configuration constants
pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: u16 = 2;
pub const FRAME_SIZE_MS: u32 = 10; // Opus minimum: 10ms (was 5ms - caused garbled audio!)
pub const FRAME_SIZE: usize = (SAMPLE_RATE as usize * FRAME_SIZE_MS as usize) / 1000;

/// Audio format represents the configuration of audio streams
#[derive(Debug, Clone, Copy)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_size: usize,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            frame_size: FRAME_SIZE,
        }
    }
}

/// Audio frame containing raw PCM samples
pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub timestamp: u64,
}

impl AudioFrame {
    pub fn new(samples: Vec<f32>, timestamp: u64) -> Self {
        Self { samples, timestamp }
    }
}
