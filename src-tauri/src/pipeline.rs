use crate::audio::{create_input_stream, create_output_stream, AudioManager};
use crate::stt::SpeechToText;
use crate::tts::TextToSpeech;
use anyhow::Result;
use cpal::traits::DeviceTrait;
use rubato::{Resampler, SincFixedIn, SincInterpolationType, SincInterpolationParameters, WindowFunction};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const SILENCE_THRESHOLD: f32 = 0.01;
const MIN_SPEECH_DURATION_MS: u64 = 300;   // Process quickly - even short words like "hey"
const DEFAULT_SILENCE_DURATION_MS: u64 = 700;  // Default pause detection time
const DEBUG_AUDIO_INTERVAL_MS: u64 = 1000; // Log audio levels every second
const PRE_ROLL_MS: u64 = 250;  // Capture audio from before speech is detected
const POST_ROLL_MS: u64 = 200; // Keep recording after speech ends to capture word endings

/// Thread-safe state that can be shared with Tauri
pub struct PipelineState {
    pub audio_manager: Mutex<AudioManager>,
    pub stt: Mutex<SpeechToText>,
    pub tts: Mutex<TextToSpeech>,
    is_running: AtomicBool,
    // Channel to signal stop
    stop_signal: Mutex<Option<Arc<AtomicBool>>>,
    // Configurable silence duration (ms)
    silence_duration_ms: std::sync::atomic::AtomicU64,
}

impl PipelineState {
    pub fn new() -> Result<Self> {
        Ok(Self {
            audio_manager: Mutex::new(AudioManager::new()?),
            stt: Mutex::new(SpeechToText::new()),
            tts: Mutex::new(TextToSpeech::new()),
            is_running: AtomicBool::new(false),
            stop_signal: Mutex::new(None),
            silence_duration_ms: AtomicU64::new(DEFAULT_SILENCE_DURATION_MS),
        })
    }

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    pub fn get_silence_duration_ms(&self) -> u64 {
        self.silence_duration_ms.load(Ordering::SeqCst)
    }

    pub fn set_silence_duration_ms(&self, ms: u64) {
        self.silence_duration_ms.store(ms, Ordering::SeqCst);
    }
}

unsafe impl Send for PipelineState {}
unsafe impl Sync for PipelineState {}

/// Resamples audio from one sample rate to another
fn resample_audio(input: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(input.to_vec());
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let ratio = to_rate as f64 / from_rate as f64;
    let mut resampler = SincFixedIn::<f32>::new(
        ratio,
        2.0, // max relative ratio
        params,
        input.len(),
        1, // mono
    )?;

    let waves_in = vec![input.to_vec()];
    let waves_out = resampler.process(&waves_in, None)?;

    Ok(waves_out.into_iter().next().unwrap_or_default())
}

/// Helper to emit status events
fn emit_status(app: &AppHandle, status: &str) {
    let _ = app.emit("pipeline-status", status);
}

/// Runs the audio pipeline. This function blocks and should be run in a separate thread.
/// The streams are kept alive within this function to avoid Send/Sync issues.
pub fn run_pipeline(state: Arc<PipelineState>, app: AppHandle) -> Result<()> {
    // Set up stop signal
    let stop_signal = Arc::new(AtomicBool::new(false));
    {
        let mut signal = state.stop_signal.lock().unwrap();
        *signal = Some(Arc::clone(&stop_signal));
    }
    state.is_running.store(true, Ordering::SeqCst);
    emit_status(&app, "listening");

    // Get device info while holding locks briefly
    let (input_device, input_config, sample_format, input_sample_rate, input_channels) = {
        let manager = state.audio_manager.lock().unwrap();
        let device = manager.get_input_device()?;
        let (config, format) = manager.get_input_config()?;
        let rate = config.sample_rate.0;
        let channels = config.channels;
        log::info!("Using input device: {:?}", device.name());
        (device, config, format, rate, channels)
    };

    let (output_device, output_config, output_sample_rate) = {
        let manager = state.audio_manager.lock().unwrap();
        let device = manager.get_output_device()?;
        let (config, _) = manager.get_output_config()?;
        let rate = config.sample_rate.0;
        log::info!("Using output device: {:?}", device.name());
        (device, config, rate)
    };

    log::info!(
        "Starting pipeline: input {} Hz {} ch, output {} Hz",
        input_sample_rate,
        input_channels,
        output_config.sample_rate.0
    );

    // Shared buffers
    let audio_input_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let audio_output_buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));

    // Pre-roll buffer: keeps recent audio to capture word beginnings
    // Size = sample_rate * PRE_ROLL_MS / 1000
    let pre_roll_size = (input_sample_rate as u64 * PRE_ROLL_MS / 1000) as usize;
    let pre_roll_buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(pre_roll_size)));

    // Voice activity detection state
    let speech_start: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let last_voice_activity: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let last_debug_log: Arc<Mutex<Instant>> = Arc::new(Mutex::new(Instant::now()));

    // Clone for input callback
    let input_buffer_clone = Arc::clone(&audio_input_buffer);
    let pre_roll_clone = Arc::clone(&pre_roll_buffer);
    let speech_start_clone = Arc::clone(&speech_start);
    let last_activity_clone = Arc::clone(&last_voice_activity);
    let last_debug_clone = Arc::clone(&last_debug_log);
    let stop_clone = Arc::clone(&stop_signal);

    // Create input stream
    let _input_stream = create_input_stream(
        &input_device,
        &input_config,
        sample_format,
        move |data: Vec<f32>| {
            if stop_clone.load(Ordering::SeqCst) {
                return;
            }

            // Convert to mono if stereo
            let mono_data: Vec<f32> = if input_channels > 1 {
                data.chunks(input_channels as usize)
                    .map(|chunk| chunk.iter().sum::<f32>() / input_channels as f32)
                    .collect()
            } else {
                data
            };

            // Detect voice activity
            let rms: f32 =
                (mono_data.iter().map(|s| s * s).sum::<f32>() / mono_data.len() as f32).sqrt();
            let is_speech = rms > SILENCE_THRESHOLD;

            let now = Instant::now();

            // Debug logging - log audio level periodically
            {
                let mut last_log = last_debug_clone.lock().unwrap();
                if now.duration_since(*last_log) >= Duration::from_millis(DEBUG_AUDIO_INTERVAL_MS) {
                    log::info!("Audio RMS: {:.4}, threshold: {:.4}, speech: {}", rms, SILENCE_THRESHOLD, is_speech);
                    *last_log = now;
                }
            }

            // Check if we're in an active recording session
            let speech_active = speech_start_clone.lock().unwrap().is_some();

            if is_speech {
                let mut start = speech_start_clone.lock().unwrap();
                let is_new_speech = start.is_none();
                if is_new_speech {
                    log::info!("Speech started");
                    *start = Some(now);
                }
                drop(start); // Release lock before acquiring others

                *last_activity_clone.lock().unwrap() = Some(now);

                let mut input_buf = input_buffer_clone.lock().unwrap();

                // If this is new speech, prepend the pre-roll buffer
                if is_new_speech {
                    let pre_roll = pre_roll_clone.lock().unwrap();
                    log::info!("Prepending {} samples from pre-roll buffer", pre_roll.len());
                    input_buf.extend(pre_roll.iter());
                }

                input_buf.extend_from_slice(&mono_data);
            } else if speech_active {
                // Not speech, but we're in an active recording session
                // Keep recording for POST_ROLL_MS after last voice activity
                let last_activity = last_activity_clone.lock().unwrap();
                if let Some(last) = *last_activity {
                    if now.duration_since(last) < Duration::from_millis(POST_ROLL_MS) {
                        // Still within post-roll window, keep recording
                        input_buffer_clone.lock().unwrap().extend_from_slice(&mono_data);
                    }
                }
            }

            // Always update pre-roll buffer (circular buffer of recent audio)
            {
                let mut pre_roll = pre_roll_clone.lock().unwrap();
                for sample in &mono_data {
                    if pre_roll.len() >= pre_roll_size {
                        pre_roll.pop_front();
                    }
                    pre_roll.push_back(*sample);
                }
            }
        },
    )?;

    // Create output stream
    let _output_stream = create_output_stream(
        &output_device,
        &output_config,
        Arc::clone(&audio_output_buffer),
    )?;

    log::info!("Audio streams started");

    // Processing loop
    while !stop_signal.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));

        let now = Instant::now();
        let should_process = {
            let last_activity = last_voice_activity.lock().unwrap();
            let speech_start_val = speech_start.lock().unwrap();

            if let (Some(last), Some(start)) = (*last_activity, *speech_start_val) {
                let silence_ms = state.get_silence_duration_ms();
                now.duration_since(last) >= Duration::from_millis(silence_ms)
                    && now.duration_since(start) >= Duration::from_millis(MIN_SPEECH_DURATION_MS)
            } else {
                false
            }
        };

        if should_process {
            // Get audio buffer
            let buffer: Vec<f32> = {
                let mut buf = audio_input_buffer.lock().unwrap();
                std::mem::take(&mut *buf)
            };

            // Reset VAD state
            *speech_start.lock().unwrap() = None;
            *last_voice_activity.lock().unwrap() = None;

            if !buffer.is_empty() {
                log::info!("Processing {} samples", buffer.len());
                emit_status(&app, "processing");

                // Transcribe
                let text = {
                    let stt = state.stt.lock().unwrap();
                    if stt.is_loaded() {
                        stt.transcribe(&buffer, input_sample_rate).ok()
                    } else {
                        log::warn!("Whisper model not loaded");
                        None
                    }
                };

                if let Some(text) = text {
                    let text = text.trim();
                    // Filter out blank audio markers and very short/noisy transcriptions
                    if !text.is_empty() && !text.contains("[BLANK_AUDIO]") && text.len() > 1 {
                        log::info!("Transcribed: {}", text);
                        emit_status(&app, "speaking");

                        // Synthesize
                        let (audio, tts_sample_rate) = {
                            let tts = state.tts.lock().unwrap();
                            if tts.is_ready() {
                                let rate = tts.get_sample_rate();
                                (tts.synthesize(&text).ok(), rate)
                            } else {
                                log::warn!("TTS not ready");
                                (None, 22050)
                            }
                        };

                        if let Some(audio) = audio {
                            log::info!("Synthesized {} samples at {} Hz", audio.len(), tts_sample_rate);

                            // Resample TTS output to match output device sample rate
                            let resampled = if tts_sample_rate != output_sample_rate {
                                log::info!("Resampling from {} Hz to {} Hz", tts_sample_rate, output_sample_rate);
                                match resample_audio(&audio, tts_sample_rate, output_sample_rate) {
                                    Ok(data) => data,
                                    Err(e) => {
                                        log::error!("Resampling failed: {}", e);
                                        audio
                                    }
                                }
                            } else {
                                audio
                            };

                            log::info!("Output {} samples to playback buffer", resampled.len());
                            let mut out = audio_output_buffer.lock().unwrap();
                            out.extend(resampled.into_iter());
                        }
                    }
                }

                emit_status(&app, "listening");
            }
        }
    }

    log::info!("Pipeline stopped");
    emit_status(&app, "stopped");
    state.is_running.store(false, Ordering::SeqCst);
    Ok(())
}

/// Stops a running pipeline
pub fn stop_pipeline(state: &PipelineState) {
    if let Some(signal) = state.stop_signal.lock().unwrap().take() {
        signal.store(true, Ordering::SeqCst);
    }
    state.is_running.store(false, Ordering::SeqCst);
}
