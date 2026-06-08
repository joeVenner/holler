//! OpenAI cloud transcription (`POST /v1/audio/transcriptions`, multipart, BYOK).

use std::time::Duration;

use reqwest::blocking::multipart::{Form, Part};
use serde::Deserialize;

use crate::{encode_wav, load_key, SttError, SttProvider};

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
