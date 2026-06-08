//! Holler configuration (Phase 1): a TOML file in the OS config dir holding
//! non-secret settings. API keys are NOT here — they live in the keychain
//! (`holler-stt::secrets`).
//!
//! `#[serde(default)]` makes every field optional, so older/newer config files
//! load without error and missing fields fall back to defaults.

pub mod ptt;
pub use ptt::parse_ptt_key;

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
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ptt_key: "ctrl+alt+space".to_string(),
            stt_provider: "deepgram".to_string(),
            stt_model: String::new(),
            injection_mode: "paste".to_string(),
            vad: true,
        }
    }
}

impl Config {
    /// `stt_model` if set, else `None` so the caller can use a provider default.
    pub fn model_override(&self) -> Option<&str> {
        let m = self.stt_model.trim();
        (!m.is_empty()).then_some(m)
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

/// `<config_dir>/Holler/config.toml`.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    let dirs = ProjectDirs::from("com", "Holler", "Holler")
        .ok_or_else(|| ConfigError::Path("could not determine a config directory".into()))?;
    Ok(dirs.config_dir().join("config.toml"))
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
    fn model_override_trims_and_empties() {
        let mut c = Config::default();
        c.stt_model = "  nova-3  ".to_string();
        assert_eq!(c.model_override(), Some("nova-3"));
        c.stt_model = "   ".to_string();
        assert_eq!(c.model_override(), None);
    }
}
