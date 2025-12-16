import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Settings {
  input_device: string | null;
  output_device: string | null;
  voice_id: string | null;
  silence_duration_ms: number;
}

function App() {
  const [isActive, setIsActive] = useState(false);
  const [status, setStatus] = useState("Ready");
  const [inputDevices, setInputDevices] = useState<string[]>([]);
  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [selectedInput, setSelectedInput] = useState<string>("");
  const [selectedOutput, setSelectedOutput] = useState<string>("");
  const [silenceDuration, setSilenceDuration] = useState(700);
  const [voices, setVoices] = useState<[string, string][]>([]);
  const [selectedVoice, setSelectedVoice] = useState<string>("");
  const settingsLoaded = useRef(false);

  useEffect(() => {
    initializeApp();

    // Listen for pipeline status events
    const unlisten = listen<string>("pipeline-status", (event) => {
      const statusMap: Record<string, string> = {
        listening: "Listening...",
        processing: "Processing...",
        speaking: "Speaking...",
        stopped: "Stopped",
      };
      setStatus(statusMap[event.payload] || event.payload);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  async function initializeApp() {
    // Load saved settings first
    let savedSettings: Settings | null = null;
    try {
      savedSettings = await invoke<Settings>("load_settings");
      settingsLoaded.current = true;
    } catch (error) {
      console.error("Failed to load settings:", error);
    }

    // Load devices, voices, then apply saved settings or defaults
    await loadDevices(savedSettings);
    await loadVoices(savedSettings);

    // Apply silence duration from saved settings
    if (savedSettings?.silence_duration_ms) {
      setSilenceDuration(savedSettings.silence_duration_ms);
    }
  }

  async function saveCurrentSettings(overrides: Partial<Settings> = {}) {
    const settings: Settings = {
      input_device: overrides.input_device !== undefined ? overrides.input_device : selectedInput || null,
      output_device: overrides.output_device !== undefined ? overrides.output_device : selectedOutput || null,
      voice_id: overrides.voice_id !== undefined ? overrides.voice_id : selectedVoice || null,
      silence_duration_ms: overrides.silence_duration_ms !== undefined ? overrides.silence_duration_ms : silenceDuration,
    };
    try {
      await invoke("save_settings", { settings });
    } catch (error) {
      console.error("Failed to save settings:", error);
    }
  }

  async function loadVoices(savedSettings: Settings | null) {
    try {
      const voiceList = await invoke<[string, string][]>("list_voices");
      setVoices(voiceList);

      // Use saved voice if available and exists in voice list, otherwise use first voice
      const savedVoiceExists = savedSettings?.voice_id && voiceList.some(([id]) => id === savedSettings.voice_id);
      const voiceToUse = savedVoiceExists ? savedSettings!.voice_id! : voiceList[0]?.[0];

      if (voiceToUse) {
        setSelectedVoice(voiceToUse);
        await invoke("select_voice", { voiceId: voiceToUse });
      }
    } catch (error) {
      console.error("Failed to load voices:", error);
    }
  }

  async function handleVoiceChange(voiceId: string) {
    try {
      await invoke("select_voice", { voiceId });
      setSelectedVoice(voiceId);
      await saveCurrentSettings({ voice_id: voiceId });
    } catch (error) {
      setStatus(`Error: ${error}`);
    }
  }

  async function handleSilenceDurationChange(ms: number) {
    try {
      await invoke("set_silence_duration", { ms });
      setSilenceDuration(ms);
      await saveCurrentSettings({ silence_duration_ms: ms });
    } catch (error) {
      setStatus(`Error: ${error}`);
    }
  }

  async function loadDevices(savedSettings: Settings | null) {
    try {
      const [inputs, outputs, defaultInput, defaultOutput] = await Promise.all([
        invoke<string[]>("list_input_devices"),
        invoke<string[]>("list_output_devices"),
        invoke<string | null>("get_default_input_device"),
        invoke<string | null>("get_default_output_device"),
      ]);
      setInputDevices(inputs);
      setOutputDevices(outputs);

      // Priority: saved settings > system default > first device
      const savedInputExists = savedSettings?.input_device && inputs.includes(savedSettings.input_device);
      const savedOutputExists = savedSettings?.output_device && outputs.includes(savedSettings.output_device);

      const inputToUse = savedInputExists
        ? savedSettings!.input_device!
        : defaultInput && inputs.includes(defaultInput)
          ? defaultInput
          : inputs[0];

      const outputToUse = savedOutputExists
        ? savedSettings!.output_device!
        : defaultOutput && outputs.includes(defaultOutput)
          ? defaultOutput
          : outputs[0];

      if (inputToUse) {
        setSelectedInput(inputToUse);
        await invoke("set_input_device", { name: inputToUse });
      }
      if (outputToUse) {
        setSelectedOutput(outputToUse);
        await invoke("set_output_device", { name: outputToUse });
      }
    } catch (error) {
      console.error("Failed to load devices:", error);
    }
  }

  async function handleInputChange(device: string) {
    try {
      await invoke("set_input_device", { name: device });
      setSelectedInput(device);
      await saveCurrentSettings({ input_device: device });
    } catch (error) {
      setStatus(`Error: ${error}`);
    }
  }

  async function handleOutputChange(device: string) {
    try {
      await invoke("set_output_device", { name: device });
      setSelectedOutput(device);
      await saveCurrentSettings({ output_device: device });
    } catch (error) {
      setStatus(`Error: ${error}`);
    }
  }

  async function toggleVoiceChanger() {
    try {
      if (isActive) {
        await invoke("cmd_stop_pipeline");
        setStatus("Stopped");
      } else {
        await invoke("start_pipeline");
        setStatus("Listening...");
      }
      setIsActive(!isActive);
    } catch (error) {
      setStatus(`Error: ${error}`);
    }
  }

  return (
    <main className="container">
      <h1>Parrot</h1>
      <p className="subtitle">Voice Anonymizer</p>

      <div className="device-selectors">
        <div className="device-select">
          <label>Input Device (Microphone)</label>
          <select
            value={selectedInput}
            onChange={(e) => handleInputChange(e.target.value)}
            disabled={isActive}
          >
            {inputDevices.map((device) => (
              <option key={device} value={device}>
                {device}
              </option>
            ))}
          </select>
        </div>

        <div className="device-select">
          <label>Output Device (Speakers)</label>
          <select
            value={selectedOutput}
            onChange={(e) => handleOutputChange(e.target.value)}
            disabled={isActive}
          >
            {outputDevices.map((device) => (
              <option key={device} value={device}>
                {device}
              </option>
            ))}
          </select>
        </div>

        <div className="device-select">
          <label>Voice</label>
          <select
            value={selectedVoice}
            onChange={(e) => handleVoiceChange(e.target.value)}
            disabled={isActive || voices.length === 0}
          >
            {voices.length === 0 ? (
              <option value="">No voices available</option>
            ) : (
              voices.map(([id, name]) => (
                <option key={id} value={id}>
                  {name}
                </option>
              ))
            )}
          </select>
        </div>

        <div className="slider-control">
          <label>
            Silence Detection: {silenceDuration}ms
            <span className="slider-hint">
              {silenceDuration < 500 ? "(faster, may cut off)" : silenceDuration > 800 ? "(slower, captures more)" : "(balanced)"}
            </span>
          </label>
          <input
            type="range"
            min="300"
            max="1200"
            step="50"
            value={silenceDuration}
            onChange={(e) => handleSilenceDurationChange(Number(e.target.value))}
          />
        </div>
      </div>

      <div className="status-display">
        <div className={`status-indicator ${isActive ? status.toLowerCase().replace("...", "") : ""}`} />
        <span>{status}</span>
      </div>

      <button
        className={`toggle-button ${isActive ? "active" : ""}`}
        onClick={toggleVoiceChanger}
      >
        {isActive ? "Stop" : "Start"}
      </button>

      <p className="hint">
        {isActive
          ? "Speak into your microphone..."
          : "Click Start to begin voice anonymization"}
      </p>
    </main>
  );
}

export default App;
