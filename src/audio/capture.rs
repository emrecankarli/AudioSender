use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig, SampleRate};
use log::{error, info, warn};
use std::sync::mpsc::Sender;
use std::time::Instant;

use super::types::{AudioFrame, AudioFormat, SAMPLE_RATE, CHANNELS};

pub struct AudioCapture {
    device: Device,
    config: StreamConfig,
    format: AudioFormat,
}

impl AudioCapture {
    /// List available output devices (for loopback capture)
    pub fn list_devices() -> Result<Vec<String>> {
        let host = cpal::default_host();
        let devices = host.output_devices()?;
        let mut names = Vec::new();
        for dev in devices {
            if let Ok(name) = dev.name() {
                names.push(name);
            }
        }
        Ok(names)
    }

    /// Create a new audio capture instance, optionally selecting a specific device
    pub fn new(device_name: Option<String>) -> Result<Self> {
        let host = cpal::default_host();
        
        // Find requested device or use default
        let device = if let Some(ref name) = device_name {
            info!("Searching for audio device containing: '{}'", name);
            host.output_devices()?
                .find(|d| d.name().map(|n| n.to_lowercase().contains(&name.to_lowercase())).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("Device not found matching: {}", name))?
        } else {
            host.default_output_device()
                .ok_or_else(|| anyhow::anyhow!("No default output device available"))?
        };
        
        info!("Using audio device: {}", device.name()?);
        
        // Get the default output config
        let default_config = device.default_output_config()?;
        let native_sample_rate = default_config.sample_rate().0;
        let native_channels = default_config.channels();
        
        info!("Device native format: {} Hz, {} channels", native_sample_rate, native_channels);
        
        // CRITICAL FIX: Force 48kHz stereo to match Opus encoder
        // This makes cpal do automatic resampling for us
        let mut stream_config: StreamConfig = default_config.into();
        stream_config.sample_rate = SampleRate(SAMPLE_RATE);
        stream_config.channels = CHANNELS;
        
        if native_sample_rate != SAMPLE_RATE {
            warn!("⚠️ Resampling from {} Hz to {} Hz (may affect quality slightly)", 
                  native_sample_rate, SAMPLE_RATE);
        }
        
        if native_channels != CHANNELS {
            warn!("⚠️ Converting from {} to {} channels", native_channels, CHANNELS);
        }
        
        info!("Stream configured at: {} Hz, {} channels (Opus compatible)", 
              SAMPLE_RATE, CHANNELS);
        
        let format = AudioFormat::default();
        
        Ok(Self {
            device,
            config: stream_config,
            format,
        })
    }
    
    /// Start capturing audio and send frames to the provided channel
    pub fn start(&self, frame_sender: Sender<AudioFrame>) -> Result<Stream> {
        let frame_size = self.format.frame_size * self.format.channels as usize;
        let mut buffer = Vec::with_capacity(frame_size);
        let mut timestamp = 0u64;
        let mut last_log = Instant::now();
        
        let stream = self.device.build_input_stream(
            &self.config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                buffer.extend_from_slice(data);
                
                // Audio level monitoring (RMS)
                // Log every 5 seconds
                if last_log.elapsed().as_secs() >= 5 {
                    let mut sum_sq = 0.0;
                    for &s in data {
                        sum_sq += s * s;
                    }
                    let rms = (sum_sq / data.len() as f32).sqrt();
                    
                    if rms < 0.0001 {
                        warn!("⚠️ Silence detected! (RMS: {:.6}) - Is audio playing on this device?", rms);
                    } else {
                        info!("✓ Audio signal detected (RMS: {:.4})", rms);
                    }
                    last_log = Instant::now();
                }
                
                // Send complete frames
                while buffer.len() >= frame_size {
                    let frame_data = buffer.drain(..frame_size).collect();
                    let frame = AudioFrame::new(frame_data, timestamp);
                    
                    if frame_sender.send(frame).is_err() {
                        // Receiver disconnected
                        return;
                    }
                    
                    timestamp += 1;
                }
            },
            |err| {
                error!("Audio stream error: {}", err);
            },
            None,
        )?;
        
        stream.play()?;
        info!("Audio capture started");
        
        Ok(stream)
    }
}
