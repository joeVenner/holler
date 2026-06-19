//! Deepgram cloud text-to-speech (`POST /v1/speak`, BYOK).
//!
//! Mirrors [`crate::OpenAiTts`], with Deepgram's conventions (same as
//! `holler-stt/deepgram.rs`): auth via the `Token` scheme (not `Bearer`), and
//! synthesis options passed as **query params** rather than in the JSON body —
//! the body carries only `{"text": ...}`.
//!
//! Deepgram encodes both the voice and the model into one `model` identifier
//! (e.g. `aura-2-thalia-en`), so the config "voice" field maps straight onto it.
//! We request a 24 kHz **linear16 WAV** container so playback can reuse the same
//! in-process `AVAudioPlayer` path as the OpenAI backend. The shared `"deepgram"`
//! key in `secrets.toml` already serves `holler-stt`'s transcription provider.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

use crate::{load_key, PreparedAudio, SpeakPhase, TtsError, TtsProvider};

pub struct DeepgramTts {
    api_key: String,
    /// Aura voice-model identifier (e.g. `aura-2-thalia-en`).
    model: String,
    client: reqwest::blocking::Client,
    /// Set by [`stop`](Self::stop); the playback poll loop halts the player when
    /// it flips true. Cleared at the start of each [`speak`](Self::speak).
    stop_requested: AtomicBool,
}

impl DeepgramTts {
    const ENDPOINT: &'static str = "https://api.deepgram.com/v1/speak";
    /// Default Aura-2 English voice. Overridable via the config voice field.
    pub const DEFAULT_MODEL: &'static str = "aura-2-thalia-en";
    /// The account name this provider's key is stored under (env var /
    /// `secrets.toml`). Shared with `holler-stt`'s Deepgram provider.
    pub const KEY_ACCOUNT: &'static str = "deepgram";
    /// Output container/encoding — a 24 kHz linear16 WAV plays directly through
    /// `AVAudioPlayer`, matching the OpenAI backend's WAV path.
    const ENCODING: &'static str = "linear16";
    const CONTAINER: &'static str = "wav";
    const SAMPLE_RATE: u32 = 24_000;

    pub fn new(api_key: String, model: Option<String>) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            api_key,
            model: normalise_model(model),
            client,
            stop_requested: AtomicBool::new(false),
        }
    }

    /// Build from the stored API key (env var or `secrets.toml`). `model` comes
    /// from the config voice field; blank falls back to [`DEFAULT_MODEL`].
    ///
    /// [`DEFAULT_MODEL`]: Self::DEFAULT_MODEL
    pub fn from_stored_key(model: Option<String>) -> Result<Self, TtsError> {
        let api_key =
            load_key(Self::KEY_ACCOUNT).ok_or_else(|| TtsError::MissingKey(Self::KEY_ACCOUNT.into()))?;
        Ok(Self::new(api_key, model))
    }

    /// The full request URL including the synthesis query params. Separated for
    /// unit testing without a network call.
    fn request_url(&self) -> String {
        format!(
            "{}?model={}&encoding={}&container={}&sample_rate={}",
            Self::ENDPOINT,
            self.model,
            Self::ENCODING,
            Self::CONTAINER,
            Self::SAMPLE_RATE,
        )
    }

    /// Fetch synthesized WAV bytes for `text`. Pure network step (no playback) so
    /// playback can be polled for cancellation, exactly like the OpenAI backend.
    fn synthesize(&self, text: &str) -> Result<Vec<u8>, TtsError> {
        let body = serde_json::to_vec(&serde_json::json!({ "text": text }))
            .map_err(|e| TtsError::Playback(e.to_string()))?;

        let response = self
            .client
            .post(self.request_url())
            .header(AUTHORIZATION, format!("Token {}", self.api_key))
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()?;

        let status = response.status();
        if !status.is_success() {
            // Error responses are JSON even though success is raw audio.
            let body = response.text()?;
            return Err(TtsError::Api(format!(
                "HTTP {status}: {}",
                parse_api_error(&body)
            )));
        }

        let bytes = response.bytes()?;
        if bytes.is_empty() {
            return Err(TtsError::Playback("empty audio response".into()));
        }
        Ok(bytes.to_vec())
    }
}

/// A Deepgram model id must be non-empty; blank/`None` falls back to the default.
fn normalise_model(model: Option<String>) -> String {
    model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| DeepgramTts::DEFAULT_MODEL.to_string())
}

/// Extract a human-readable message from a Deepgram error body, trying the
/// fields Deepgram uses (`err_msg`, then `reason`/`message`), and falling back to
/// the raw text for non-JSON edge/proxy errors.
fn parse_api_error(body: &str) -> String {
    #[derive(Deserialize)]
    struct DgError {
        err_msg: Option<String>,
        reason: Option<String>,
        message: Option<String>,
    }
    serde_json::from_str::<DgError>(body)
        .ok()
        .and_then(|e| e.err_msg.or(e.reason).or(e.message))
        .unwrap_or_else(|| body.to_string())
}

impl TtsProvider for DeepgramTts {
    #[cfg(target_os = "macos")]
    fn speak(&self, text: &str, on_phase: &dyn Fn(SpeakPhase)) -> Result<(), TtsError> {
        self.stop_requested.store(false, Ordering::SeqCst);
        if text.trim().is_empty() {
            return Ok(());
        }
        on_phase(SpeakPhase::Synthesizing);
        let wav = self.synthesize(text)?;
        // A stop() during the network round-trip should suppress playback.
        if self.stop_requested.load(Ordering::SeqCst) {
            return Ok(());
        }
        on_phase(SpeakPhase::Playing);
        crate::playback::play_audio(&wav, &self.stop_requested)
    }

    #[cfg(not(target_os = "macos"))]
    fn speak(&self, _text: &str, _on_phase: &dyn Fn(SpeakPhase)) -> Result<(), TtsError> {
        // In-process playback uses AVFoundation (macOS-only). Don't make a network
        // call that would be discarded — return immediately so the caller falls back.
        Err(TtsError::Unsupported(
            "cloud TTS playback is implemented for macOS only in this build".into(),
        ))
    }

    fn stop(&self) -> Result<(), TtsError> {
        self.stop_requested.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn name(&self) -> &str {
        "deepgram"
    }

    // In-process AVFoundation playback is macOS-only, so prefetch is offered
    // only there; elsewhere the trait defaults leave `can_prefetch()` false.
    fn can_prefetch(&self) -> bool {
        cfg!(target_os = "macos")
    }

    /// Pre-synthesize without playing — the network step of [`speak`](Self::speak)
    /// on its own, so the worker can fetch the next batch while this one plays.
    #[cfg(target_os = "macos")]
    fn prepare(&self, text: &str) -> Result<PreparedAudio, TtsError> {
        Ok(PreparedAudio::new(self.synthesize(text)?))
    }

    #[cfg(target_os = "macos")]
    fn play_prepared(
        &self,
        audio: PreparedAudio,
        on_phase: &dyn Fn(SpeakPhase),
    ) -> Result<(), TtsError> {
        // Clear any stale stop request, mirroring `speak`'s re-arm semantics.
        self.stop_requested.store(false, Ordering::SeqCst);
        on_phase(SpeakPhase::Playing);
        crate::playback::play_audio(audio.as_bytes(), &self.stop_requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> DeepgramTts {
        DeepgramTts::new("dg-test".into(), None)
    }

    #[test]
    fn url_carries_model_and_wav_params() {
        let url = provider().request_url();
        assert!(url.starts_with(DeepgramTts::ENDPOINT));
        assert!(url.contains(&format!("model={}", DeepgramTts::DEFAULT_MODEL)));
        assert!(url.contains("encoding=linear16"));
        assert!(url.contains("container=wav"));
        assert!(url.contains("sample_rate=24000"));
    }

    #[test]
    fn explicit_model_overrides_default_but_blank_falls_back() {
        let p = DeepgramTts::new("k".into(), Some("  aura-2-orion-en ".into()));
        assert!(p.request_url().contains("model=aura-2-orion-en"));
        let p = DeepgramTts::new("k".into(), Some("   ".into()));
        assert!(p.request_url().contains(&format!("model={}", DeepgramTts::DEFAULT_MODEL)));
    }

    #[test]
    fn parse_api_error_reads_deepgram_envelope() {
        let body = r#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials","request_id":"x"}"#;
        assert_eq!(parse_api_error(body), "Invalid credentials");
        // `reason` is used by some endpoints when `err_msg` is absent.
        let body = r#"{"reason":"model not found"}"#;
        assert_eq!(parse_api_error(body), "model not found");
    }

    #[test]
    fn parse_api_error_falls_back_to_raw_body() {
        assert_eq!(parse_api_error("502 Bad Gateway"), "502 Bad Gateway");
    }

    #[test]
    fn name_is_deepgram() {
        assert_eq!(provider().name(), "deepgram");
    }

    #[test]
    fn empty_text_is_a_silent_noop_without_a_network_call() {
        assert!(provider().speak("   ", &|_| {}).is_ok());
    }

    #[test]
    fn stop_sets_the_flag_and_is_idempotent() {
        let p = provider();
        assert!(p.stop().is_ok());
        assert!(p.stop_requested.load(Ordering::SeqCst));
        assert!(p.stop().is_ok());
    }

    #[test]
    fn speak_resets_a_prior_stop_flag() {
        let p = provider();
        assert!(p.stop().is_ok());
        assert!(p.speak("", &|_| {}).is_ok());
        assert!(!p.stop_requested.load(Ordering::SeqCst));
    }
}
