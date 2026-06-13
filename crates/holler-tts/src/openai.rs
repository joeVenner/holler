//! OpenAI cloud text-to-speech (`POST /v1/audio/speech`, BYOK).
//!
//! Mirrors `holler-stt/openai.rs`: a blocking [`reqwest`] client (the request
//! runs on a worker thread, never the main loop), the key resolved from the
//! shared `"openai"` account in `secrets.toml` via [`from_stored_key`](OpenAiTts::from_stored_key),
//! and `{"error": {"message": ...}}` envelopes decoded for failures.
//!
//! The endpoint returns *encoded audio bytes* (not JSON). We ask for **WAV** so
//! playback can reuse the in-process `AVAudioPlayer` (AVFoundation is already
//! linked for the native backend — no extra audio dependency). Network and
//! playback both block the calling worker thread, honouring the [`TtsProvider`]
//! contract; cancellation flows through the same `Send + Sync` [`AtomicBool`]
//! stop-flag pattern as the native backend.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;

use crate::{load_key, TtsError, TtsProvider};

pub struct OpenAiTts {
    api_key: String,
    /// TTS model (e.g. `gpt-4o-mini-tts`).
    model: String,
    /// Built-in OpenAI voice name (e.g. `alloy`).
    voice: String,
    client: reqwest::blocking::Client,
    /// Set by [`stop`](Self::stop); the playback poll loop halts the player when
    /// it flips true. Cleared at the start of each [`speak`](Self::speak).
    stop_requested: AtomicBool,
}

impl OpenAiTts {
    const ENDPOINT: &'static str = "https://api.openai.com/v1/audio/speech";
    /// Low-latency, low-cost speech model — the dictation read-back use case
    /// doesn't need the premium tier.
    pub const DEFAULT_MODEL: &'static str = "gpt-4o-mini-tts";
    /// Default built-in voice when config doesn't specify one.
    pub const DEFAULT_VOICE: &'static str = "alloy";
    /// Audio container we request — WAV so `AVAudioPlayer` plays it directly.
    const RESPONSE_FORMAT: &'static str = "wav";
    /// The account name this provider's key is stored under (env var /
    /// `secrets.toml`). Shared with `holler-stt`'s OpenAI provider.
    pub const KEY_ACCOUNT: &'static str = "openai";

    pub fn new(api_key: String, model: String, voice: Option<String>) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            api_key,
            model,
            voice: normalise_voice(voice),
            client,
            stop_requested: AtomicBool::new(false),
        }
    }

    /// Build from the stored API key (env var or `secrets.toml`).
    pub fn from_stored_key(model: String, voice: Option<String>) -> Result<Self, TtsError> {
        let api_key = load_key(Self::KEY_ACCOUNT)
            .ok_or_else(|| TtsError::MissingKey(Self::KEY_ACCOUNT.into()))?;
        Ok(Self::new(api_key, model, voice))
    }

    /// Fetch synthesized WAV bytes for `text` from the speech endpoint. Pure
    /// network step (no playback) — kept separate so playback can be polled for
    /// cancellation.
    fn synthesize(&self, text: &str) -> Result<Vec<u8>, TtsError> {
        let body = serde_json::to_vec(&self.request_body(text))
            .map_err(|e| TtsError::Playback(e.to_string()))?;

        let response = self
            .client
            .post(Self::ENDPOINT)
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
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

    /// Build the JSON request body. Separated for unit testing without a network
    /// call (the only thing tests can verify about the request).
    fn request_body(&self, text: &str) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "voice": self.voice,
            "input": text,
            "response_format": Self::RESPONSE_FORMAT,
        })
    }
}

/// An OpenAI voice name must be non-empty; blank/`None` falls back to the
/// default rather than sending an invalid request.
fn normalise_voice(voice: Option<String>) -> String {
    voice
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OpenAiTts::DEFAULT_VOICE.to_string())
}

#[derive(Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
}

/// Extract the human-readable message from an OpenAI `{"error": {...}}` body,
/// falling back to the raw text for non-JSON edge/proxy errors.
fn parse_api_error(body: &str) -> String {
    serde_json::from_str::<ApiErrorEnvelope>(body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| body.to_string())
}

impl TtsProvider for OpenAiTts {
    #[cfg(target_os = "macos")]
    fn speak(&self, text: &str) -> Result<(), TtsError> {
        // Fresh utterance: clear any stale stop request from a prior call.
        self.stop_requested.store(false, Ordering::SeqCst);
        if text.trim().is_empty() {
            return Ok(());
        }

        let wav = self.synthesize(text)?;
        // A stop() arriving during the network round-trip should suppress
        // playback rather than start it.
        if self.stop_requested.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.play_wav(&wav)
    }

    #[cfg(not(target_os = "macos"))]
    fn speak(&self, text: &str) -> Result<(), TtsError> {
        // The cloud request is cross-platform, but in-process playback currently
        // relies on AVFoundation. Other hosts need their own sink (TODO).
        self.stop_requested.store(false, Ordering::SeqCst);
        if text.trim().is_empty() {
            return Ok(());
        }
        let _wav = self.synthesize(text)?;
        Err(TtsError::Unsupported(
            "cloud TTS playback is implemented for macOS only in this build".into(),
        ))
    }

    fn stop(&self) -> Result<(), TtsError> {
        self.stop_requested.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn name(&self) -> &str {
        "openai"
    }
}

#[cfg(target_os = "macos")]
impl OpenAiTts {
    /// Play WAV bytes in-process via `AVAudioPlayer`, blocking until playback
    /// finishes or [`stop`](Self::stop) is requested. Pumps a short `NSRunLoop`
    /// slice per iteration (same pattern as the native synthesizer) so the
    /// player's async callbacks run without ever touching the main loop.
    fn play_wav(&self, wav: &[u8]) -> Result<(), TtsError> {
        use objc2::rc::Retained;
        use objc2::AnyThread;
        use objc2_avf_audio::AVAudioPlayer;
        use objc2_foundation::{NSData, NSDate, NSRunLoop};

        // How long each run-loop slice services events before re-checking
        // playback state / the stop flag.
        const POLL_SLICE_SECS: f64 = 0.05;

        // `NSData::with_bytes` copies the slice into an owned NSData (no extra
        // crate features needed). SAFETY (below): `AVAudioPlayer::alloc` +
        // `initWithData:error:` is the documented designated initialiser; the
        // player and its run-loop pumping stay on this one thread.
        let data = NSData::with_bytes(wav);
        unsafe {
            let player: Retained<AVAudioPlayer> =
                AVAudioPlayer::initWithData_error(AVAudioPlayer::alloc(), &data)
                    .map_err(|e| TtsError::Playback(e.localizedDescription().to_string()))?;

            if !player.play() {
                return Err(TtsError::Playback("AVAudioPlayer refused to start".into()));
            }

            let run_loop = NSRunLoop::currentRunLoop();
            while player.isPlaying() {
                if self.stop_requested.load(Ordering::SeqCst) {
                    player.stop();
                    break;
                }
                let until = NSDate::dateWithTimeIntervalSinceNow(POLL_SLICE_SECS);
                run_loop.runUntilDate(&until);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenAiTts {
        OpenAiTts::new("sk-test".into(), OpenAiTts::DEFAULT_MODEL.into(), None)
    }

    #[test]
    fn request_body_carries_model_voice_input_and_wav_format() {
        let p = provider();
        let body = p.request_body("hello world");
        assert_eq!(body["model"], OpenAiTts::DEFAULT_MODEL);
        assert_eq!(body["voice"], OpenAiTts::DEFAULT_VOICE);
        assert_eq!(body["input"], "hello world");
        assert_eq!(body["response_format"], "wav");
    }

    #[test]
    fn explicit_voice_overrides_default_but_blank_falls_back() {
        let p = OpenAiTts::new("k".into(), "m".into(), Some("  nova ".into()));
        assert_eq!(p.request_body("x")["voice"], "nova");
        let p = OpenAiTts::new("k".into(), "m".into(), Some("   ".into()));
        assert_eq!(p.request_body("x")["voice"], OpenAiTts::DEFAULT_VOICE);
    }

    #[test]
    fn parse_api_error_reads_openai_envelope() {
        let body = r#"{"error":{"message":"Invalid API key","type":"auth"}}"#;
        assert_eq!(parse_api_error(body), "Invalid API key");
    }

    #[test]
    fn parse_api_error_falls_back_to_raw_body() {
        // Non-JSON proxy/gateway error must not be swallowed.
        assert_eq!(parse_api_error("502 Bad Gateway"), "502 Bad Gateway");
    }

    #[test]
    fn name_is_openai() {
        assert_eq!(provider().name(), "openai");
    }

    #[test]
    fn empty_text_is_a_silent_noop_without_a_network_call() {
        // No key path is exercised and synthesize() is never reached, so this
        // stays offline.
        let p = provider();
        assert!(p.speak("   ").is_ok());
    }

    #[test]
    fn stop_sets_the_flag_and_is_idempotent() {
        let p = provider();
        assert!(p.stop().is_ok());
        assert!(p.stop_requested.load(Ordering::SeqCst));
        assert!(p.stop().is_ok());
    }

    /// A stop requested before speaking must suppress the (would-be) network +
    /// playback path: speak() with empty text resets the flag, proving speak
    /// always re-arms.
    #[test]
    fn speak_resets_a_prior_stop_flag() {
        let p = provider();
        assert!(p.stop().is_ok());
        assert!(p.speak("").is_ok());
        assert!(!p.stop_requested.load(Ordering::SeqCst));
    }
}
