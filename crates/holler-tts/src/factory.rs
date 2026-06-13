//! Backend factory: resolve the configured [`TtsProvider`] from a
//! [`holler_config::Config`].
//!
//! Lives in `holler-tts` (not `holler-config`) so the dependency arrow stays
//! one-way: `holler-tts` already depends on `holler-config`, and the reverse
//! must never hold (config knows nothing about concrete providers). Mirrors
//! `holler-app`'s `build_provider` for STT.
//!
//! The cloud backend is only built when an OpenAI key is present; a misconfigured
//! `Cloud` selection (no key) **gracefully falls back to Native** so read-aloud
//! never panics or silently does nothing at the app level.

use std::sync::Arc;

use holler_config::Config;

use crate::{DeepgramTts, NativeTts, OpenAiTts, TtsBackend, TtsProvider};

/// The concrete backend a config resolves to, after accounting for a missing
/// cloud key. Split out from [`build_tts`] so the selection logic is unit-
/// testable without touching `secrets.toml` or the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedBackend {
    /// Offline macOS native voice.
    Native,
    /// OpenAI cloud TTS (a key was found).
    OpenAi,
    /// Deepgram cloud TTS (a key was found).
    Deepgram,
}

/// Decide which backend to actually build for `cfg`. The `*_key` flags are the
/// (side-effecting) key checks pulled out so tests can drive the branches
/// directly. A cloud backend selected without its key degrades to Native so
/// read-aloud never silently does nothing.
fn resolve_backend(cfg: &Config, openai_key: bool, deepgram_key: bool) -> ResolvedBackend {
    match TtsBackend::from_config(&cfg.tts_backend) {
        TtsBackend::OpenAi if openai_key => ResolvedBackend::OpenAi,
        TtsBackend::Deepgram if deepgram_key => ResolvedBackend::Deepgram,
        TtsBackend::OpenAi | TtsBackend::Deepgram => {
            eprintln!(
                "[holler] tts_backend={} but no API key configured; \
                 falling back to the native voice",
                cfg.tts_backend
            );
            ResolvedBackend::Native
        }
        TtsBackend::Native => ResolvedBackend::Native,
    }
}

/// Build the configured TTS provider, reading the cloud key from `secrets.toml`
/// / the env var when needed. Native is the always-available default; a cloud
/// backend is used only when selected *and* its key is present, else it degrades
/// to Native.
///
/// Always returns a usable provider (never `None`) — read-aloud should work out
/// of the box on macOS without any key. The config "voice" field is the OpenAI
/// voice name for the OpenAI backend and the Aura model id for Deepgram.
pub fn build_tts(cfg: &Config) -> Arc<dyn TtsProvider> {
    let voice = cfg.tts_voice_override().map(str::to_string);
    let rate = cfg.tts_rate_override();

    let native = || Arc::new(NativeTts::new(voice.clone(), rate)) as Arc<dyn TtsProvider>;

    let openai_key =
        OpenAiTts::from_stored_key(OpenAiTts::DEFAULT_MODEL.to_string(), voice.clone()).is_ok();
    let deepgram_key = DeepgramTts::from_stored_key(voice.clone()).is_ok();

    match resolve_backend(cfg, openai_key, deepgram_key) {
        // Keys were present a moment ago; if one raced away, still fall back.
        ResolvedBackend::OpenAi => {
            match OpenAiTts::from_stored_key(OpenAiTts::DEFAULT_MODEL.to_string(), voice.clone()) {
                Ok(p) => Arc::new(p) as Arc<dyn TtsProvider>,
                Err(_) => native(),
            }
        }
        ResolvedBackend::Deepgram => match DeepgramTts::from_stored_key(voice.clone()) {
            Ok(p) => Arc::new(p) as Arc<dyn TtsProvider>,
            Err(_) => native(),
        },
        ResolvedBackend::Native => native(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_backend(backend: &str) -> Config {
        Config {
            tts_backend: backend.to_string(),
            ..Config::default()
        }
    }

    #[test]
    fn native_config_resolves_native_regardless_of_key() {
        let cfg = cfg_with_backend("native");
        assert_eq!(resolve_backend(&cfg, false, false), ResolvedBackend::Native);
        assert_eq!(resolve_backend(&cfg, true, true), ResolvedBackend::Native);
    }

    #[test]
    fn default_config_resolves_native() {
        let cfg = Config::default();
        assert_eq!(resolve_backend(&cfg, true, true), ResolvedBackend::Native);
    }

    #[test]
    fn openai_config_uses_openai_only_with_key() {
        let cfg = cfg_with_backend("cloud");
        assert_eq!(resolve_backend(&cfg, true, false), ResolvedBackend::OpenAi);
        // The "openai" value behaves identically to the "cloud" alias.
        let cfg = cfg_with_backend("openai");
        assert_eq!(resolve_backend(&cfg, true, false), ResolvedBackend::OpenAi);
    }

    #[test]
    fn deepgram_config_uses_deepgram_only_with_key() {
        let cfg = cfg_with_backend("deepgram");
        assert_eq!(resolve_backend(&cfg, false, true), ResolvedBackend::Deepgram);
    }

    #[test]
    fn cloud_config_falls_back_to_native_without_its_key() {
        // OpenAI selected, only a Deepgram key present -> native.
        let cfg = cfg_with_backend("cloud");
        assert_eq!(resolve_backend(&cfg, false, true), ResolvedBackend::Native);
        // Deepgram selected, only an OpenAI key present -> native.
        let cfg = cfg_with_backend("deepgram");
        assert_eq!(resolve_backend(&cfg, true, false), ResolvedBackend::Native);
    }

    #[test]
    fn unknown_backend_resolves_native() {
        let cfg = cfg_with_backend("wat");
        assert_eq!(resolve_backend(&cfg, true, true), ResolvedBackend::Native);
    }

    #[test]
    fn build_tts_always_returns_a_provider() {
        // Without a stored key the factory must still yield a working provider.
        let provider = build_tts(&Config::default());
        assert_eq!(provider.name(), "native");
    }
}
