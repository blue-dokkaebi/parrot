use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::io::Write;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Clone, Debug)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub model_path: PathBuf,
    pub config_path: PathBuf,
}

pub struct TextToSpeech {
    piper_path: Option<PathBuf>,
    voices: Vec<Voice>,
    current_voice: Option<Voice>,
}

impl TextToSpeech {
    pub fn new() -> Self {
        Self {
            piper_path: None,
            voices: Vec::new(),
            current_voice: None,
        }
    }

    pub fn set_piper_path(&mut self, path: PathBuf) -> Result<()> {
        if !path.exists() {
            return Err(anyhow!("Piper executable not found: {:?}", path));
        }
        self.piper_path = Some(path);
        Ok(())
    }

    pub fn add_voice(&mut self, id: &str, name: &str, model_path: PathBuf, config_path: PathBuf) -> Result<()> {
        if !model_path.exists() {
            return Err(anyhow!("Voice model not found: {:?}", model_path));
        }
        if !config_path.exists() {
            return Err(anyhow!("Voice config not found: {:?}", config_path));
        }

        self.voices.push(Voice {
            id: id.to_string(),
            name: name.to_string(),
            model_path,
            config_path,
        });

        Ok(())
    }

    pub fn list_voices(&self) -> Vec<(String, String)> {
        self.voices
            .iter()
            .map(|v| (v.id.clone(), v.name.clone()))
            .collect()
    }

    pub fn select_voice(&mut self, voice_id: &str) -> Result<()> {
        let voice = self
            .voices
            .iter()
            .find(|v| v.id == voice_id)
            .cloned()
            .ok_or_else(|| anyhow!("Voice not found: {}", voice_id))?;

        self.current_voice = Some(voice);
        Ok(())
    }

    pub fn synthesize(&self, text: &str) -> Result<Vec<f32>> {
        let piper_path = self
            .piper_path
            .as_ref()
            .ok_or_else(|| anyhow!("Piper path not set"))?;

        let voice = self
            .current_voice
            .as_ref()
            .ok_or_else(|| anyhow!("No voice selected"))?;

        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        log::info!("Synthesizing: {}", text);

        // Run piper and capture raw audio output
        let mut cmd = Command::new(piper_path);
        cmd.args([
                "--model", voice.model_path.to_str().unwrap(),
                "--config", voice.config_path.to_str().unwrap(),
                "--output-raw",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Hide console window on Windows
        #[cfg(windows)]
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

        let mut child = cmd.spawn()
            .map_err(|e| anyhow!("Failed to spawn piper: {}", e))?;

        // Write text to stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }

        let output = child.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Piper failed: {}", stderr));
        }

        // Convert raw PCM (16-bit signed, 22050 Hz) to f32
        let samples: Vec<f32> = output
            .stdout
            .chunks_exact(2)
            .map(|chunk| {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                sample as f32 / 32768.0
            })
            .collect();

        log::info!("Synthesized {} samples", samples.len());

        Ok(samples)
    }

    pub fn get_sample_rate(&self) -> u32 {
        // Piper outputs at 22050 Hz by default
        22050
    }

    pub fn is_ready(&self) -> bool {
        self.piper_path.is_some() && self.current_voice.is_some()
    }
}

impl Default for TextToSpeech {
    fn default() -> Self {
        Self::new()
    }
}
