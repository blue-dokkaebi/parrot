use anyhow::{anyhow, Result};
use std::path::PathBuf;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct SpeechToText {
    ctx: Option<WhisperContext>,
    model_path: Option<PathBuf>,
}

impl SpeechToText {
    pub fn new() -> Self {
        Self {
            ctx: None,
            model_path: None,
        }
    }

    pub fn load_model(&mut self, model_path: PathBuf) -> Result<()> {
        log::info!("Loading Whisper model from: {:?}", model_path);

        if !model_path.exists() {
            return Err(anyhow!("Model file not found: {:?}", model_path));
        }

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or_else(|| anyhow!("Invalid path"))?,
            WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow!("Failed to load Whisper model: {}", e))?;

        self.ctx = Some(ctx);
        self.model_path = Some(model_path);

        log::info!("Whisper model loaded successfully");
        Ok(())
    }

    pub fn transcribe(&self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| anyhow!("Whisper model not loaded"))?;

        // Resample to 16kHz if needed (Whisper expects 16kHz mono)
        // Your mic may run at 96kHz, 48kHz, 44.1kHz, etc - we convert to what Whisper needs
        let mut audio_16k = if sample_rate != 16000 {
            log::debug!("Resampling from {} Hz to 16000 Hz", sample_rate);
            resample_audio(audio_data, sample_rate, 16000)?
        } else {
            audio_data.to_vec()
        };

        // Whisper requires at least 1 second of audio (16000 samples at 16kHz)
        // Pad to 1.1 seconds (17600 samples) to be safe with rounding
        const MIN_SAMPLES: usize = 17600;
        if audio_16k.len() < MIN_SAMPLES {
            log::info!("Padding short audio ({} samples) to {} samples for Whisper", audio_16k.len(), MIN_SAMPLES);
            audio_16k.resize(MIN_SAMPLES, 0.0);
        }

        let mut state = ctx.create_state().map_err(|e| anyhow!("Failed to create state: {}", e))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        // Configure for real-time, English-only transcription
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(false);  // Don't suppress short utterances
        params.set_suppress_nst(true);  // But do suppress non-speech noise

        state
            .full(params, &audio_16k)
            .map_err(|e| anyhow!("Transcription failed: {}", e))?;

        let num_segments = state.full_n_segments().map_err(|e| anyhow!("Failed to get segments: {}", e))?;

        let mut text = String::new();
        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
                text.push(' ');
            }
        }

        Ok(text.trim().to_string())
    }

    pub fn is_loaded(&self) -> bool {
        self.ctx.is_some()
    }
}

impl Default for SpeechToText {
    fn default() -> Self {
        Self::new()
    }
}

fn resample_audio(input: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

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

    let mut resampler = SincFixedIn::<f32>::new(
        to_rate as f64 / from_rate as f64,
        2.0,
        params,
        input.len(),
        1,
    )
    .map_err(|e| anyhow!("Failed to create resampler: {}", e))?;

    let waves_in = vec![input.to_vec()];
    let waves_out = resampler
        .process(&waves_in, None)
        .map_err(|e| anyhow!("Resampling failed: {}", e))?;

    Ok(waves_out.into_iter().next().unwrap_or_default())
}
