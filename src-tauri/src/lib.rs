mod audio;
mod pipeline;
mod settings;
mod stt;
mod tts;

use pipeline::{run_pipeline, stop_pipeline, PipelineState};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, State};

struct AppState {
    pipeline: Arc<PipelineState>,
}

#[tauri::command]
fn start_pipeline(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    if state.pipeline.is_running() {
        return Ok(());
    }

    let pipeline = Arc::clone(&state.pipeline);
    thread::spawn(move || {
        if let Err(e) = run_pipeline(pipeline, app) {
            log::error!("Pipeline error: {}", e);
        }
    });

    Ok(())
}

#[tauri::command]
fn cmd_stop_pipeline(state: State<AppState>) -> Result<(), String> {
    stop_pipeline(&state.pipeline);
    Ok(())
}

#[tauri::command]
fn is_pipeline_running(state: State<AppState>) -> Result<bool, String> {
    Ok(state.pipeline.is_running())
}

#[tauri::command]
fn list_input_devices(state: State<AppState>) -> Result<Vec<String>, String> {
    let manager = state.pipeline.audio_manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.list_input_devices())
}

#[tauri::command]
fn list_output_devices(state: State<AppState>) -> Result<Vec<String>, String> {
    let manager = state.pipeline.audio_manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.list_output_devices())
}

#[tauri::command]
fn get_default_input_device(state: State<AppState>) -> Result<Option<String>, String> {
    let manager = state.pipeline.audio_manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.get_default_input_device_name())
}

#[tauri::command]
fn get_default_output_device(state: State<AppState>) -> Result<Option<String>, String> {
    let manager = state.pipeline.audio_manager.lock().map_err(|e| e.to_string())?;
    Ok(manager.get_default_output_device_name())
}

#[tauri::command]
fn set_input_device(state: State<AppState>, name: String) -> Result<(), String> {
    let mut manager = state.pipeline.audio_manager.lock().map_err(|e| e.to_string())?;
    manager.set_input_device(&name).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_output_device(state: State<AppState>, name: String) -> Result<(), String> {
    let mut manager = state.pipeline.audio_manager.lock().map_err(|e| e.to_string())?;
    manager.set_output_device(&name).map_err(|e| e.to_string())
}

#[tauri::command]
fn load_whisper_model(state: State<AppState>, path: String) -> Result<(), String> {
    let mut stt = state.pipeline.stt.lock().map_err(|e| e.to_string())?;
    stt.load_model(PathBuf::from(path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_voices(state: State<AppState>) -> Result<Vec<(String, String)>, String> {
    let tts = state.pipeline.tts.lock().map_err(|e| e.to_string())?;
    Ok(tts.list_voices())
}

#[tauri::command]
fn select_voice(state: State<AppState>, voice_id: String) -> Result<(), String> {
    let mut tts = state.pipeline.tts.lock().map_err(|e| e.to_string())?;
    tts.select_voice(&voice_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_piper_path(state: State<AppState>, path: String) -> Result<(), String> {
    let mut tts = state.pipeline.tts.lock().map_err(|e| e.to_string())?;
    tts.set_piper_path(PathBuf::from(path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn add_voice(
    state: State<AppState>,
    id: String,
    name: String,
    model_path: String,
    config_path: String,
) -> Result<(), String> {
    let mut tts = state.pipeline.tts.lock().map_err(|e| e.to_string())?;
    tts.add_voice(&id, &name, PathBuf::from(model_path), PathBuf::from(config_path))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_silence_duration(state: State<AppState>) -> Result<u64, String> {
    Ok(state.pipeline.get_silence_duration_ms())
}

#[tauri::command]
fn set_silence_duration(state: State<AppState>, ms: u64) -> Result<(), String> {
    state.pipeline.set_silence_duration_ms(ms);
    Ok(())
}

#[tauri::command]
fn load_settings() -> Result<settings::Settings, String> {
    settings::Settings::load().map_err(|e| e.to_string())
}

#[tauri::command]
fn save_settings(settings: settings::Settings) -> Result<(), String> {
    settings.save().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let pipeline = Arc::new(PipelineState::new().expect("Failed to create pipeline"));

    // Auto-load models on startup
    {
        // Get the executable's directory to find models relative to it
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        // In production builds, Tauri places resources in a "resources" folder next to the exe
        // In dev builds, resources are in the project root
        let resource_dir = exe_dir.join("resources");
        let dev_root = exe_dir.join("..").join("..").join("..");  // target/debug -> project root

        // Try to find models in various locations (production first, then dev)
        let possible_dirs = vec![
            resource_dir.clone(),       // Production: resources folder next to exe
            dev_root.clone(),           // Dev: project root from target/debug
            PathBuf::from("."),         // Current directory
        ];

        log::info!("Looking for resources in: {:?}", possible_dirs);

        // Load Whisper model (tiny model - fastest)
        let whisper_model = possible_dirs.iter()
            .flat_map(|d| vec![
                d.join("ggml-tiny.en.bin"),                    // Production: flat in resources
                d.join("models").join("ggml-tiny.en.bin"),    // Dev: in models folder
            ])
            .find(|p| p.exists());

        if let Some(model_path) = whisper_model {
            log::info!("Loading Whisper model from: {:?}", model_path);
            if let Ok(mut stt) = pipeline.stt.lock() {
                if let Err(e) = stt.load_model(model_path) {
                    log::error!("Failed to load Whisper model: {}", e);
                } else {
                    log::info!("Whisper model loaded successfully");
                }
            }
        } else {
            log::warn!("Whisper model not found");
        }

        // Configure Piper TTS
        let piper_exe = possible_dirs.iter()
            .flat_map(|d| vec![
                d.join("piper.exe"),                          // Production: flat in resources
                d.join("piper").join("piper").join("piper.exe"),  // Dev: in piper folder
            ])
            .find(|p| p.exists());

        log::info!("Piper exe search result: {:?}", piper_exe);

        if let Some(piper_path) = piper_exe {
            log::info!("Configuring Piper TTS from: {:?}", piper_path);
            if let Ok(mut tts) = pipeline.tts.lock() {
                if let Err(e) = tts.set_piper_path(piper_path) {
                    log::error!("Failed to set Piper path: {}", e);
                } else {
                    // Define available voices
                    let voices = [
                        ("lessac", "Lessac (Neutral)", "en_US-lessac-medium"),
                        ("ryan", "Ryan (Male)", "en_US-ryan-medium"),
                        ("alba", "Alba (Female)", "en_GB-alba-medium"),
                    ];

                    let mut first_voice = None;
                    for (id, name, file_base) in voices {
                        // Find voice files (production: in voices/ subfolder, dev: in models/voices)
                        let model_path = possible_dirs.iter()
                            .flat_map(|d| vec![
                                d.join("voices").join(format!("{}.onnx", file_base)),  // Production
                                d.join("models").join("voices").join(format!("{}.onnx", file_base)),  // Dev
                            ])
                            .find(|p| p.exists());
                        let config_path = possible_dirs.iter()
                            .flat_map(|d| vec![
                                d.join("voices").join(format!("{}.onnx.json", file_base)),  // Production
                                d.join("models").join("voices").join(format!("{}.onnx.json", file_base)),  // Dev
                            ])
                            .find(|p| p.exists());

                        if let (Some(model), Some(config)) = (model_path, config_path) {
                            if let Err(e) = tts.add_voice(id, name, model, config) {
                                log::error!("Failed to add voice {}: {}", id, e);
                            } else {
                                log::info!("Added voice: {} ({})", name, id);
                                if first_voice.is_none() {
                                    first_voice = Some(id);
                                }
                            }
                        } else {
                            log::warn!("Voice files not found for: {}", id);
                        }
                    }

                    // Select the first available voice
                    if let Some(voice_id) = first_voice {
                        if let Err(e) = tts.select_voice(voice_id) {
                            log::error!("Failed to select voice: {}", e);
                        } else {
                            log::info!("Selected default voice: {}", voice_id);
                        }
                    }
                }
            }
        } else {
            log::warn!("Piper executable not found");
        }
    }

    tauri::Builder::default()
        .manage(AppState { pipeline })
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_pipeline,
            cmd_stop_pipeline,
            is_pipeline_running,
            list_input_devices,
            list_output_devices,
            get_default_input_device,
            get_default_output_device,
            set_input_device,
            set_output_device,
            load_whisper_model,
            list_voices,
            select_voice,
            set_piper_path,
            add_voice,
            get_silence_duration,
            set_silence_duration,
            load_settings,
            save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
