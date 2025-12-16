use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SampleFormat, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Manages audio device enumeration and selection.
/// Stream management is handled separately to avoid Send/Sync issues.
pub struct AudioManager {
    host: Host,
    input_device_name: Option<String>,
    output_device_name: Option<String>,
}

impl AudioManager {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        Ok(Self {
            host,
            input_device_name: None,
            output_device_name: None,
        })
    }

    pub fn list_input_devices(&self) -> Vec<String> {
        self.host
            .input_devices()
            .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    pub fn list_output_devices(&self) -> Vec<String> {
        self.host
            .output_devices()
            .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    pub fn get_default_input_device_name(&self) -> Option<String> {
        self.host.default_input_device().and_then(|d| d.name().ok())
    }

    pub fn get_default_output_device_name(&self) -> Option<String> {
        self.host.default_output_device().and_then(|d| d.name().ok())
    }

    pub fn set_input_device(&mut self, name: &str) -> Result<()> {
        // Verify device exists
        let _ = self.get_input_device_by_name(name)?;
        self.input_device_name = Some(name.to_string());
        Ok(())
    }

    pub fn set_output_device(&mut self, name: &str) -> Result<()> {
        // Verify device exists
        let _ = self.get_output_device_by_name(name)?;
        self.output_device_name = Some(name.to_string());
        Ok(())
    }

    fn get_input_device_by_name(&self, name: &str) -> Result<Device> {
        self.host
            .input_devices()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| anyhow!("Input device not found: {}", name))
    }

    fn get_output_device_by_name(&self, name: &str) -> Result<Device> {
        self.host
            .output_devices()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| anyhow!("Output device not found: {}", name))
    }

    pub fn get_input_device(&self) -> Result<Device> {
        match &self.input_device_name {
            Some(name) => self.get_input_device_by_name(name),
            None => self
                .host
                .default_input_device()
                .ok_or_else(|| anyhow!("No input device selected")),
        }
    }

    pub fn get_output_device(&self) -> Result<Device> {
        match &self.output_device_name {
            Some(name) => self.get_output_device_by_name(name),
            None => self
                .host
                .default_output_device()
                .ok_or_else(|| anyhow!("No output device selected")),
        }
    }

    pub fn get_input_config(&self) -> Result<(StreamConfig, SampleFormat)> {
        let device = self.get_input_device()?;
        let config = device.default_input_config()?;
        let sample_format = config.sample_format();
        Ok((config.into(), sample_format))
    }

    pub fn get_output_config(&self) -> Result<(StreamConfig, SampleFormat)> {
        let device = self.get_output_device()?;
        let config = device.default_output_config()?;
        let sample_format = config.sample_format();
        Ok((config.into(), sample_format))
    }
}

impl Default for AudioManager {
    fn default() -> Self {
        Self::new().expect("Failed to initialize audio")
    }
}

/// Creates an input stream that sends audio data to the provided callback.
/// Returns the stream which must be kept alive for audio to flow.
pub fn create_input_stream<F>(
    device: &Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    mut on_audio: F,
) -> Result<Stream>
where
    F: FnMut(Vec<f32>) + Send + 'static,
{
    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                on_audio(data.to_vec());
            },
            |err| log::error!("Audio input error: {}", err),
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let float_data: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                on_audio(float_data);
            },
            |err| log::error!("Audio input error: {}", err),
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                let float_data: Vec<f32> = data
                    .iter()
                    .map(|&s| (s as f32 - 32768.0) / 32768.0)
                    .collect();
                on_audio(float_data);
            },
            |err| log::error!("Audio input error: {}", err),
            None,
        )?,
        _ => return Err(anyhow!("Unsupported sample format")),
    };

    stream.play()?;
    Ok(stream)
}

/// Creates an output stream that pulls audio from a shared buffer.
/// The buffer contains mono samples which are duplicated to all output channels.
pub fn create_output_stream(
    device: &Device,
    config: &StreamConfig,
    audio_buffer: Arc<Mutex<VecDeque<f32>>>,
) -> Result<Stream> {
    let channels = config.channels as usize;
    log::info!("Creating output stream with {} channels", channels);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let mut buffer = audio_buffer.lock().unwrap();
            // Process frame by frame (each frame has `channels` samples)
            for frame in data.chunks_mut(channels) {
                // Get one mono sample and duplicate to all channels
                let sample = buffer.pop_front().unwrap_or(0.0);
                for channel_sample in frame.iter_mut() {
                    *channel_sample = sample;
                }
            }
        },
        |err| log::error!("Audio output error: {}", err),
        None,
    )?;

    stream.play()?;
    Ok(stream)
}
