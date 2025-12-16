use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_NAME: &str = "parrot";
const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub voice_id: Option<String>,
    #[serde(default = "default_silence_duration")]
    pub silence_duration_ms: u64,
}

fn default_silence_duration() -> u64 {
    700
}

impl Settings {
    pub fn new() -> Self {
        Self {
            input_device: None,
            output_device: None,
            voice_id: None,
            silence_duration_ms: default_silence_duration(),
        }
    }

    fn settings_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join(APP_NAME).join(SETTINGS_FILE))
    }

    pub fn load() -> Result<Self> {
        let path = Self::settings_path()
            .context("Could not determine config directory")?;

        if !path.exists() {
            log::info!("No settings file found at {:?}, using defaults", path);
            return Ok(Self::new());
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read settings from {:?}", path))?;

        let settings: Settings = serde_json::from_str(&contents)
            .with_context(|| format!("Failed to parse settings from {:?}", path))?;

        log::info!("Loaded settings from {:?}", path);
        Ok(settings)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::settings_path()
            .context("Could not determine config directory")?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory {:?}", parent))?;
        }

        let contents = serde_json::to_string_pretty(self)
            .context("Failed to serialize settings")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write settings to {:?}", path))?;

        log::info!("Saved settings to {:?}", path);
        Ok(())
    }
}
