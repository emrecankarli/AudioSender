use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleRate, Stream, StreamConfig};
use log::{error, info, warn};
use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::types::{AudioFrame, CHANNELS, SAMPLE_RATE};

// Start safely, but allow auto-tuning down or up
const INITIAL_BUFFER_TARGET_MS: usize = 60; 
const MIN_BUFFER_MS: usize = 20;
const MAX_BUFFER_MS: usize = 500;

pub struct AudioPlayback {
    device: Device,
    config: StreamConfig,
}

impl AudioPlayback {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No output device available"))?;

        info!("Using playback device: {}", device.name()?);

        let default_config = device.default_output_config()?;
        let native_sample_rate = default_config.sample_rate().0;
        
        let mut config: StreamConfig = default_config.into();
        config.sample_rate = SampleRate(SAMPLE_RATE);
        config.channels = CHANNELS;

        // Verify native format to hint at potential hardware dissatisfaction
        if native_sample_rate != SAMPLE_RATE {
            info!("Note: Resampling from {} Hz to {} Hz", SAMPLE_RATE, native_sample_rate);
        }

        Ok(Self { device, config })
    }

    pub fn start(&self, frame_receiver: Receiver<AudioFrame>) -> Result<Stream> {
        // Pre-allocate decent size
        let mut sample_buffer = VecDeque::with_capacity(48000); 
        let mut buffering = true;
        let mut started = false;
        
        // Dynamic Buffer Target
        let mut current_target_ms = INITIAL_BUFFER_TARGET_MS;
        
        // Calculate sample counts
        let samples_per_ms = (SAMPLE_RATE as usize * CHANNELS as usize) / 1000;
        
        // Stats
        let mut underrun_count = 0;
        let mut last_stat_log = Instant::now();
        let mut last_adjustment = Instant::now();

        info!("Starting Auto-Tuning Engine (Initial Target: {}ms)", current_target_ms);

        let stream = self.device.build_output_stream(
            &self.config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                // 1. Ingest
                while let Ok(frame) = frame_receiver.try_recv() {
                    sample_buffer.extend(frame.samples);
                }

                // 2. Auto-Tuning Logic (Drift & Underrun Management)
                let target_samples = current_target_ms * samples_per_ms;
                let max_samples = MAX_BUFFER_MS * samples_per_ms;
                
                // If buffer gets huge (latency spike), trim it
                if sample_buffer.len() > max_samples {
                     let drop = sample_buffer.len() - target_samples;
                     sample_buffer.drain(..drop);
                }

                // 3. Playback
                if buffering {
                    if sample_buffer.len() >= target_samples {
                        buffering = false;
                        started = true;
                        info!("Buffer stabilized at {}ms. Playing.", current_target_ms);
                    } else {
                        data.fill(0.0);
                        return;
                    }
                }

                let samples_needed = data.len();
                
                if sample_buffer.len() >= samples_needed {
                    // Normal playback
                    for i in 0..samples_needed {
                        data[i] = sample_buffer.pop_front().unwrap_or(0.0);
                    }
                } else {
                    // Underrun
                    let available = sample_buffer.len();
                    for i in 0..available {
                        data[i] = sample_buffer.pop_front().unwrap_or(0.0);
                    }
                    data[available..].fill(0.0); // Soft silence
                    
                    underrun_count += 1;
                    
                    // SMART FIX: If underruns happen too often, increase buffer size!
                    if started && last_adjustment.elapsed().as_secs() > 1 {
                         if current_target_ms < MAX_BUFFER_MS {
                             current_target_ms += 20; // Add 20ms safety
                             last_adjustment = Instant::now();
                             warn!("Underrun detected! Increasing safety buffer to {}ms", current_target_ms);
                         }
                    }
                }
                
                if last_stat_log.elapsed() >= Duration::from_secs(5) {
                    if underrun_count > 0 {
                        info!("Stats: {} underruns in last 5s (Current Buffer Target: {}ms)", underrun_count, current_target_ms);
                    }
                    underrun_count = 0;
                    last_stat_log = Instant::now();
                }
            },
            |err| {
                error!("Playback stream error: {}", err);
            },
            None,
        )?;

        stream.play()?;
        Ok(stream)
    }
}
