//! Holler text-to-speech (Phase 3 — read-selection).
//!
//! A provider-agnostic [`TtsProvider`] trait mirroring `holler-stt`'s pluggable
//! design. Backends are config-selectable:
//!
//! - **Native** ([`NativeTts`], the default): the macOS system voice — offline,
//!   no API key. Prefers the in-process `AVSpeechSynthesizer` and falls back to
//!   the `say` binary.
//! - **Cloud** ([`OpenAiTts`], optional): OpenAI's `/v1/audio/speech` endpoint
//!   via a key in `secrets.toml`, mirroring `holler-stt/openai.rs` + `secrets.rs`.
//!   Synthesised WAV bytes are played in-process via `AVAudioPlayer`.
//!
//! Speech is **blocking** and meant to run on a worker thread — never on the
//! main winit/event loop. A [`TtsProvider`] owns no live audio handle between
//! calls, so the app holds an `Arc<dyn TtsProvider>` and speaks per request.

mod factory;
mod native;
mod openai;
pub mod secrets;

pub use factory::{build_tts, ResolvedBackend};
pub use native::NativeTts;
pub use openai::OpenAiTts;
pub use secrets::{load_key, store_key};

/// Text-to-speech backends. Implementations must be `Send + Sync` so the app
/// can hand an `Arc<dyn TtsProvider>` to a worker thread per utterance.
pub trait TtsProvider: Send + Sync {
    /// Speak `text` aloud, blocking until playback finishes (or the synthesis
    /// request completes, for cloud backends that hand off to an audio sink).
    fn speak(&self, text: &str) -> Result<(), TtsError>;
    /// Stop any in-progress speech immediately. A no-op if nothing is playing.
    fn stop(&self) -> Result<(), TtsError>;
    /// Short label for logging/UI (e.g. "native").
    fn name(&self) -> &str;
}

/// Which TTS backend the app should build, parsed from config. Unknown values
/// fall back to the offline [`TtsBackend::Native`] default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TtsBackend {
    /// Offline macOS system voice (`say` / AVSpeechSynthesizer). No key needed.
    #[default]
    Native,
    /// OpenAI cloud TTS (BYOK via `secrets.toml`).
    Cloud,
}

impl TtsBackend {
    /// Parse a config string; unknown values fall back to the default (`Native`).
    pub fn from_config(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "cloud" | "openai" => TtsBackend::Cloud,
            _ => TtsBackend::Native,
        }
    }
}

/// Errors surfaced by the TTS layer. Dependency-light (no `thiserror`); each
/// variant carries a rendered message so callers can log without matching on
/// foreign error types.
#[derive(Debug)]
pub enum TtsError {
    /// No API key configured for this provider (env var or `secrets.toml`).
    MissingKey(String),
    /// The platform speech engine could not be reached/launched.
    Engine(String),
    /// The network request itself failed (DNS, TLS, timeout, …).
    Http(String),
    /// The API returned a non-success status; carries the server's message.
    Api(String),
    /// The success response could not be played/parsed.
    Playback(String),
    /// The requested backend is not implemented on this platform yet.
    Unsupported(String),
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TtsError::MissingKey(m) => write!(f, "no API key configured for {m}"),
            TtsError::Engine(m) => write!(f, "speech engine error: {m}"),
            TtsError::Http(m) => write!(f, "request failed: {m}"),
            TtsError::Api(m) => write!(f, "tts API error: {m}"),
            TtsError::Playback(m) => write!(f, "playback failed: {m}"),
            TtsError::Unsupported(m) => write!(f, "tts not supported here: {m}"),
        }
    }
}

impl std::error::Error for TtsError {}

impl From<reqwest::Error> for TtsError {
    fn from(e: reqwest::Error) -> Self {
        TtsError::Http(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_parses_case_insensitively() {
        assert_eq!(TtsBackend::from_config("Cloud"), TtsBackend::Cloud);
        assert_eq!(TtsBackend::from_config("OPENAI"), TtsBackend::Cloud);
        assert_eq!(TtsBackend::from_config("native"), TtsBackend::Native);
        // Unknown -> default.
        assert_eq!(TtsBackend::from_config("wat"), TtsBackend::Native);
        assert_eq!(TtsBackend::default(), TtsBackend::Native);
    }
}
