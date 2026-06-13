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

use crate::{NativeTts, OpenAiTts, TtsBackend, TtsProvider};

/// The concrete backend a config resolves to, after accounting for a missing
/// cloud key. Split out from [`build_tts`] so the selection logic is unit-
/// testable without touching `secrets.toml` or the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedBackend {
    /// Offline macOS native voice.
    Native,
    /// OpenAI cloud TTS (a key was found).
    Cloud,
}

/// Decide which backend to actually build for `cfg`. `cloud_key_present` is the
/// (side-effecting) check pulled out so tests can drive the branch directly.
fn resolve_backend(cfg: &Config, cloud_key_present: bool) -> ResolvedBackend {
    match TtsBackend::from_config(&cfg.tts_backend) {
        TtsBackend::Cloud if cloud_key_present => ResolvedBackend::Cloud,
        TtsBackend::Cloud => {
            eprintln!(
                "[holler] tts_backend=cloud but no OpenAI key configured; \
                 falling back to the native voice"
            );
            ResolvedBackend::Native
        }
        TtsBackend::Native => ResolvedBackend::Native,
    }
}

/// Build the configured TTS provider, reading the cloud key from `secrets.toml`
/// / the env var when needed. Native is the always-available default; Cloud is
/// used only when selected *and* a key is present, else it degrades to Native.
///
/// Always returns a usable provider (never `None`) — read-aloud should work out
/// of the box on macOS without any key.
pub fn build_tts(cfg: &Config) -> Arc<dyn TtsProvider> {
    let voice = cfg.tts_voice_override().map(str::to_string);
    let rate = cfg.tts_rate_override();

    let key_present = OpenAiTts::from_stored_key(
        OpenAiTts::DEFAULT_MODEL.to_string(),
        voice.clone(),
    )
    .is_ok();

    match resolve_backend(cfg, key_present) {
        ResolvedBackend::Cloud => {
            // Key was present a moment ago; if it raced away, still fall back.
            match OpenAiTts::from_stored_key(OpenAiTts::DEFAULT_MODEL.to_string(), voice.clone()) {
                Ok(p) => Arc::new(p) as Arc<dyn TtsProvider>,
                Err(_) => Arc::new(NativeTts::new(voice, rate)) as Arc<dyn TtsProvider>,
            }
        }
        ResolvedBackend::Native => Arc::new(NativeTts::new(voice, rate)) as Arc<dyn TtsProvider>,
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
        assert_eq!(resolve_backend(&cfg, false), ResolvedBackend::Native);
        assert_eq!(resolve_backend(&cfg, true), ResolvedBackend::Native);
    }

    #[test]
    fn default_config_resolves_native() {
        let cfg = Config::default();
        assert_eq!(resolve_backend(&cfg, true), ResolvedBackend::Native);
    }

    #[test]
    fn cloud_config_uses_cloud_only_with_key() {
        let cfg = cfg_with_backend("cloud");
        assert_eq!(resolve_backend(&cfg, true), ResolvedBackend::Cloud);
    }

    #[test]
    fn cloud_config_falls_back_to_native_without_key() {
        let cfg = cfg_with_backend("cloud");
        assert_eq!(resolve_backend(&cfg, false), ResolvedBackend::Native);
        // The "openai" alias for the cloud backend must behave identically.
        let cfg = cfg_with_backend("openai");
        assert_eq!(resolve_backend(&cfg, false), ResolvedBackend::Native);
    }

    #[test]
    fn unknown_backend_resolves_native() {
        let cfg = cfg_with_backend("wat");
        assert_eq!(resolve_backend(&cfg, true), ResolvedBackend::Native);
    }

    #[test]
    fn build_tts_always_returns_a_provider() {
        // Without a stored key the factory must still yield a working provider.
        let provider = build_tts(&Config::default());
        assert_eq!(provider.name(), "native");
    }
}
