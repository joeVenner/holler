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

mod deepgram;
mod factory;
mod native;
mod openai;
#[cfg(target_os = "macos")]
mod playback;
pub mod secrets;

pub use deepgram::DeepgramTts;
pub use factory::{build_tts, ResolvedBackend};
pub use native::NativeTts;
pub use openai::OpenAiTts;
pub use secrets::{load_key, store_key};

/// Progress reported as [`TtsProvider::speak`] advances, so a caller can drive a
/// status UI without knowing each backend's internals. Cloud backends emit
/// [`Synthesizing`](SpeakPhase::Synthesizing) during the network round-trip then
/// [`Playing`](SpeakPhase::Playing) when audio starts; the native `say` backend
/// has no separable synthesis step and emits `Playing` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakPhase {
    /// Audio is being generated (a cloud network request is in flight).
    Synthesizing,
    /// Audio playback has begun.
    Playing,
}

/// Encoded audio (e.g. WAV bytes) synthesized by [`TtsProvider::prepare`] ahead
/// of playback, so a caller can prefetch upcoming batches while the current one
/// is still playing. Opaque: the bytes are produced and consumed by the same
/// backend, and only [`TtsProvider::play_prepared`] interprets them.
#[derive(Debug, Clone)]
pub struct PreparedAudio(Vec<u8>);

impl PreparedAudio {
    /// Wrap raw encoded audio bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
    /// The encoded bytes, handed to the playback sink.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Text-to-speech backends. Implementations must be `Send + Sync` so the app
/// can hand an `Arc<dyn TtsProvider>` to a worker thread per utterance.
pub trait TtsProvider: Send + Sync {
    /// Speak `text` aloud, blocking until playback finishes (or the synthesis
    /// request completes, for cloud backends that hand off to an audio sink).
    /// `on_phase` is invoked as the call moves through [`SpeakPhase`]s — once per
    /// transition, on the calling (worker) thread — so the caller can surface
    /// "generating" vs "speaking" without polling. Empty/whitespace `text` is a
    /// silent no-op and reports no phase.
    fn speak(&self, text: &str, on_phase: &dyn Fn(SpeakPhase)) -> Result<(), TtsError>;
    /// Stop any in-progress speech immediately. A no-op if nothing is playing.
    fn stop(&self) -> Result<(), TtsError>;
    /// Short label for logging/UI (e.g. "native").
    fn name(&self) -> &str;

    /// Whether this backend can synthesize audio separately from playing it (via
    /// [`prepare`](Self::prepare) + [`play_prepared`](Self::play_prepared)),
    /// letting a caller prefetch the next batch while the current one plays.
    /// Cloud backends can (each batch is a network round-trip worth hiding); the
    /// in-process native voice cannot separate the two. Default: `false`.
    fn can_prefetch(&self) -> bool {
        false
    }

    /// Synthesize `text` to ready-to-play audio WITHOUT playing it. Only called
    /// when [`can_prefetch`](Self::can_prefetch) is true. Safe to call from a
    /// background prefetch thread concurrently with playback. Default: the
    /// backend doesn't support pre-synthesis.
    fn prepare(&self, text: &str) -> Result<PreparedAudio, TtsError> {
        let _ = text;
        Err(TtsError::Unsupported(
            "this backend cannot pre-synthesize audio".into(),
        ))
    }

    /// Play audio produced by [`prepare`](Self::prepare), blocking until playback
    /// finishes or [`stop`](Self::stop) is called. `on_phase` reports
    /// [`Playing`](SpeakPhase::Playing). Default: unsupported.
    fn play_prepared(
        &self,
        audio: PreparedAudio,
        on_phase: &dyn Fn(SpeakPhase),
    ) -> Result<(), TtsError> {
        let _ = (audio, on_phase);
        Err(TtsError::Unsupported(
            "this backend cannot play pre-synthesized audio".into(),
        ))
    }
}

/// Which TTS backend the app should build, parsed from config. Unknown values
/// fall back to the offline [`TtsBackend::Native`] default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TtsBackend {
    /// Offline macOS system voice (the `say` binary). No key needed.
    #[default]
    Native,
    /// OpenAI cloud TTS (BYOK via `secrets.toml`). The legacy `"cloud"` config
    /// value still maps here.
    OpenAi,
    /// Deepgram cloud TTS — Aura voices (BYOK via `secrets.toml`).
    Deepgram,
}

impl TtsBackend {
    /// Parse a config string; unknown values fall back to the default (`Native`).
    /// `"cloud"` remains an alias for OpenAI so pre-existing configs keep working.
    pub fn from_config(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "cloud" | "openai" => TtsBackend::OpenAi,
            "deepgram" => TtsBackend::Deepgram,
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
        // "cloud" stays an OpenAI alias for backward compatibility.
        assert_eq!(TtsBackend::from_config("Cloud"), TtsBackend::OpenAi);
        assert_eq!(TtsBackend::from_config("OPENAI"), TtsBackend::OpenAi);
        assert_eq!(TtsBackend::from_config("Deepgram"), TtsBackend::Deepgram);
        assert_eq!(TtsBackend::from_config("native"), TtsBackend::Native);
        // Unknown -> default.
        assert_eq!(TtsBackend::from_config("wat"), TtsBackend::Native);
        assert_eq!(TtsBackend::default(), TtsBackend::Native);
    }
}
