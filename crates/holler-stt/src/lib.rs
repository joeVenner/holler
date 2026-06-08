//! Holler speech-to-text (Phase 1).
//!
//! A provider-agnostic [`SttProvider`] trait (locked BYOK decision in
//! `docs/DECISIONS.md`) with [`OpenAiStt`] as the first implementation. Local
//! Whisper and Deepgram slot in behind the same trait later.
//!
//! Transcription is **blocking** and meant to run on a worker thread — never on
//! the main winit/event loop. The batch (transcribe-on-release) path doesn't
//! benefit from async, so we use `reqwest::blocking` and skip a tokio runtime.
//!
//! API keys live in the **OS keychain** via [`store_key`] / the provider's
//! `from_keychain` — never in config files.

pub mod secrets;

use std::io::Cursor;
use std::time::Duration;

use reqwest::blocking::multipart::{Form, Part};
use serde::Deserialize;

pub use secrets::{load_key, store_key};

/// Speech-to-text backends. Implementations must be `Send + Sync` so the app
/// can hand an `Arc<dyn SttProvider>` to a worker thread per utterance.
pub trait SttProvider: Send + Sync {
    /// Transcribe mono f32 samples (normalised to ~[-1.0, 1.0]) at `sample_rate`.
    fn transcribe(&self, samples: &[f32], sample_rate: u32) -> Result<String, SttError>;
    /// Short label for logging/UI (e.g. "openai").
    fn name(&self) -> &str;
}

/// Errors surfaced by the STT layer. Dependency-light (no `thiserror`); each
/// variant carries a rendered message so callers can log without matching on
/// foreign error types.
#[derive(Debug)]
pub enum SttError {
    /// No API key found in the keychain for this provider.
    MissingKey(String),
    /// Encoding the audio to WAV failed.
    Encode(String),
    /// The network request itself failed (DNS, TLS, timeout, …).
    Http(String),
    /// The API returned a non-success status; carries the server's message.
    Api(String),
    /// The success response could not be parsed.
    Parse(String),
}

impl std::fmt::Display for SttError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttError::MissingKey(m) => write!(f, "no API key in keychain: {m}"),
            SttError::Encode(m) => write!(f, "audio encode failed: {m}"),
            SttError::Http(m) => write!(f, "request failed: {m}"),
            SttError::Api(m) => write!(f, "transcription API error: {m}"),
            SttError::Parse(m) => write!(f, "could not parse response: {m}"),
        }
    }
}

impl std::error::Error for SttError {}

impl From<reqwest::Error> for SttError {
    fn from(e: reqwest::Error) -> Self {
        SttError::Http(e.to_string())
    }
}

/// OpenAI cloud transcription (`POST /v1/audio/transcriptions`, multipart, BYOK).
pub struct OpenAiStt {
    api_key: String,
    model: String,
    client: reqwest::blocking::Client,
}

impl OpenAiStt {
    const ENDPOINT: &'static str = "https://api.openai.com/v1/audio/transcriptions";
    /// Best accuracy-per-cost for short single-speaker dictation clips.
    /// `gpt-4o-transcribe` is the higher-accuracy opt-in. `whisper-1` is legacy.
    pub const DEFAULT_MODEL: &'static str = "gpt-4o-mini-transcribe";
    /// The keychain account name this provider's key is stored under.
    pub const KEY_ACCOUNT: &'static str = "openai";

    pub fn new(api_key: String, model: String) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            api_key,
            model,
            client,
        }
    }

    /// Build from the API key stored in the OS keychain.
    pub fn from_keychain(model: String) -> Result<Self, SttError> {
        let api_key =
            load_key(Self::KEY_ACCOUNT).map_err(|e| SttError::MissingKey(e.to_string()))?;
        Ok(Self::new(api_key, model))
    }
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
}

impl SttProvider for OpenAiStt {
    fn transcribe(&self, samples: &[f32], sample_rate: u32) -> Result<String, SttError> {
        let wav = encode_wav(samples, sample_rate)?;

        let file = Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| SttError::Encode(e.to_string()))?;
        let form = Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json")
            .part("file", file);

        let response = self
            .client
            .post(Self::ENDPOINT)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()?;

        let status = response.status();
        let body = response.text()?;

        if !status.is_success() {
            // OpenAI errors are `{"error": {"message": ...}}`; fall back to the
            // raw body for non-JSON edge/proxy errors.
            let message = serde_json::from_str::<ApiErrorEnvelope>(&body)
                .map(|e| e.error.message)
                .unwrap_or(body);
            return Err(SttError::Api(format!("HTTP {status}: {message}")));
        }

        serde_json::from_str::<TranscriptionResponse>(&body)
            .map(|r| r.text.trim().to_string())
            .map_err(|e| SttError::Parse(e.to_string()))
    }

    fn name(&self) -> &str {
        "openai"
    }
}

/// Encode mono f32 samples as 16-bit PCM WAV in memory (a container OpenAI
/// accepts directly — no re-encode).
fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, SttError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = Cursor::new(Vec::<u8>::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)
            .map_err(|e| SttError::Encode(e.to_string()))?;
        for &s in samples {
            let scaled = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(scaled)
                .map_err(|e| SttError::Encode(e.to_string()))?;
        }
        writer
            .finalize()
            .map_err(|e| SttError::Encode(e.to_string()))?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_has_riff_header_and_expected_size() {
        // 100 mono samples -> 44-byte WAV header + 100 * 2 bytes of PCM.
        let samples = vec![0.0f32; 100];
        let wav = encode_wav(&samples, 16_000).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 100 * 2);
    }

    #[test]
    fn wav_clamps_out_of_range_samples() {
        // Encoding must not panic on samples outside [-1.0, 1.0].
        let samples = vec![2.0f32, -2.0, 0.5];
        let wav = encode_wav(&samples, 16_000).unwrap();
        assert_eq!(wav.len(), 44 + 3 * 2);
    }
}
