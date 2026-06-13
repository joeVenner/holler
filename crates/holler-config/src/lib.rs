//! Holler configuration (Phase 1): a TOML file in the OS config dir holding
//! non-secret settings. API keys are NOT here — they live in a separate
//! `secrets.toml` (see [`secrets`]) so the config file stays safe to share.
//!
//! `#[serde(default)]` makes every field optional, so older/newer config files
//! load without error and missing fields fall back to defaults.

pub mod ptt;
pub mod secrets;
pub use ptt::{parse_ptt_key, try_parse_ptt_key};
pub use secrets::{load_secret, remove_secret, secret_status, secrets_path, store_secret, SecretStatus};

use std::fs;
use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Push-to-talk combo, e.g. "ctrl+alt+space".
    pub ptt_key: String,
    /// STT provider: "deepgram" or "openai".
    pub stt_provider: String,
    /// Model name; empty string means "use the provider's default".
    pub stt_model: String,
    /// Injection strategy: "paste" or "type".
    pub injection_mode: String,
    /// Whether to trim leading/trailing silence via WebRTC VAD before STT.
    #[serde(default = "default_true")]
    pub vad: bool,
    /// Show an on-screen "Copied to clipboard — paste it" toast when auto-paste
    /// can't run (Accessibility not granted, or injection failed). Default on.
    #[serde(default = "default_true")]
    pub clipboard_toast: bool,
    /// TTS (read-aloud) backend: "native" (offline macOS voice, default),
    /// "cloud"/"openai" (OpenAI `/v1/audio/speech`, BYOK), or "deepgram"
    /// (Deepgram Aura `/v1/speak`, BYOK). Parsed leniently by
    /// `holler_tts::TtsBackend::from_config`; unknown values fall back to native.
    #[serde(default = "default_tts_backend")]
    pub tts_backend: String,
    /// TTS voice name; empty = the backend's default voice.
    #[serde(default)]
    pub tts_voice: String,
    /// TTS speaking rate in words-per-minute; 0 = the backend default.
    #[serde(default)]
    pub tts_rate: u32,
    /// Hotkey combo (e.g. "ctrl+alt+r") that reads the current selection aloud.
    #[serde(default = "default_tts_read_hotkey")]
    pub tts_read_hotkey: String,
    /// Hotkey combo that reads the clipboard contents aloud.
    #[serde(default = "default_tts_read_clipboard_hotkey")]
    pub tts_read_clipboard_hotkey: String,
    /// Hotkey combo that stops any in-progress speech.
    #[serde(default = "default_tts_stop_hotkey")]
    pub tts_stop_hotkey: String,
}

fn default_true() -> bool {
    true
}

fn default_tts_backend() -> String {
    "native".to_string()
}

fn default_tts_read_hotkey() -> String {
    "ctrl+alt+r".to_string()
}

fn default_tts_read_clipboard_hotkey() -> String {
    "ctrl+alt+c".to_string()
}

fn default_tts_stop_hotkey() -> String {
    "ctrl+alt+period".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ptt_key: "ctrl+alt+space".to_string(),
            stt_provider: "deepgram".to_string(),
            stt_model: String::new(),
            injection_mode: "paste".to_string(),
            vad: true,
            clipboard_toast: true,
            tts_backend: default_tts_backend(),
            tts_voice: String::new(),
            tts_rate: 0,
            tts_read_hotkey: default_tts_read_hotkey(),
            tts_read_clipboard_hotkey: default_tts_read_clipboard_hotkey(),
            tts_stop_hotkey: default_tts_stop_hotkey(),
        }
    }
}

impl Config {
    /// `stt_model` if set, else `None` so the caller can use a provider default.
    pub fn model_override(&self) -> Option<&str> {
        let m = self.stt_model.trim();
        (!m.is_empty()).then_some(m)
    }

    /// `tts_voice` if set, else `None` so the backend can use its default voice.
    pub fn tts_voice_override(&self) -> Option<&str> {
        let v = self.tts_voice.trim();
        (!v.is_empty()).then_some(v)
    }

    /// `tts_rate` words-per-minute if non-zero, else `None` (backend default).
    pub fn tts_rate_override(&self) -> Option<u32> {
        (self.tts_rate != 0).then_some(self.tts_rate)
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Path(String),
    Io(String),
    Parse(String),
    Serialize(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Path(m) => write!(f, "config path error: {m}"),
            ConfigError::Io(m) => write!(f, "config i/o error: {m}"),
            ConfigError::Parse(m) => write!(f, "config parse error: {m}"),
            ConfigError::Serialize(m) => write!(f, "config serialize error: {m}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The Holler project directories (config/data) — shared by `config.toml` and
/// `secrets.toml` so they always resolve to the same folder.
pub(crate) fn project_dirs() -> Result<ProjectDirs, ConfigError> {
    ProjectDirs::from("com", "Holler", "Holler")
        .ok_or_else(|| ConfigError::Path("could not determine a config directory".into()))
}

/// `<config_dir>/Holler/config.toml`.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    Ok(project_dirs()?.config_dir().join("config.toml"))
}

/// `<data_dir>/Holler/holler.log` — the rolling diagnostics log. Lives in the
/// data dir (next to `history.db`), not the config dir: it's machine state, not
/// user-editable settings. A tray agent launched from Finder has no console, so
/// this file is the only place `eprintln!`/panic output can actually be seen.
pub fn log_path() -> Result<PathBuf, ConfigError> {
    Ok(project_dirs()?.data_dir().join("holler.log"))
}

/// Load the config, creating a default file on first run.
pub fn load_or_create() -> Result<Config, ConfigError> {
    let path = config_path()?;
    if path.exists() {
        let text = fs::read_to_string(&path).map_err(|e| ConfigError::Io(e.to_string()))?;
        toml::from_str(&text).map_err(|e| ConfigError::Parse(e.to_string()))
    } else {
        let cfg = Config::default();
        save(&cfg)?;
        Ok(cfg)
    }
}

/// Write `cfg` to the config path (creating the directory if needed).
pub fn save(cfg: &Config) -> Result<(), ConfigError> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| ConfigError::Serialize(e.to_string()))?;
    fs::write(&path, text).map_err(|e| ConfigError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.stt_provider, "deepgram");
        assert_eq!(c.injection_mode, "paste");
        assert_eq!(c.model_override(), None);
        assert!(c.vad);
        assert!(c.clipboard_toast);
    }

    #[test]
    fn clipboard_toast_defaults_on_when_absent() {
        // An older config file with no clipboard_toast key must opt in by default.
        let back: Config = toml::from_str("stt_provider = \"openai\"\n").unwrap();
        assert!(back.clipboard_toast);
    }

    #[test]
    fn toml_roundtrips() {
        let c = Config::default();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        // Only one field set; the rest must come from Default.
        let back: Config = toml::from_str("stt_provider = \"openai\"\n").unwrap();
        assert_eq!(back.stt_provider, "openai");
        assert_eq!(back.injection_mode, "paste");
    }

    #[test]
    fn tts_defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.tts_backend, "native");
        assert_eq!(c.tts_voice_override(), None);
        assert_eq!(c.tts_rate_override(), None);
        assert_eq!(c.tts_read_hotkey, "ctrl+alt+r");
        assert_eq!(c.tts_read_clipboard_hotkey, "ctrl+alt+c");
        assert_eq!(c.tts_stop_hotkey, "ctrl+alt+period");
    }

    #[test]
    fn tts_fields_default_when_absent() {
        // An older config file with no tts_* keys must fall back to defaults.
        let back: Config = toml::from_str("stt_provider = \"openai\"\n").unwrap();
        assert_eq!(back.tts_backend, "native");
        assert_eq!(back.tts_voice, "");
        assert_eq!(back.tts_rate, 0);
        assert_eq!(back.tts_read_hotkey, "ctrl+alt+r");
        assert_eq!(back.tts_stop_hotkey, "ctrl+alt+period");
    }

    #[test]
    fn tts_voice_and_rate_overrides() {
        let c = Config {
            tts_voice: "  Samantha  ".to_string(),
            tts_rate: 180,
            ..Default::default()
        };
        assert_eq!(c.tts_voice_override(), Some("Samantha"));
        assert_eq!(c.tts_rate_override(), Some(180));
    }

    #[test]
    fn model_override_trims_and_empties() {
        let mut c = Config {
            stt_model: "  nova-3  ".to_string(),
            ..Default::default()
        };
        assert_eq!(c.model_override(), Some("nova-3"));
        c.stt_model = "   ".to_string();
        assert_eq!(c.model_override(), None);
    }
}
