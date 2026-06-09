//! Deepgram cloud transcription (`POST /v1/listen`, raw audio body, BYOK).
//!
//! Unlike OpenAI this is NOT multipart: the WAV bytes are the request body with
//! `Content-Type: audio/wav`, options are query params, and auth uses the
//! `Token` scheme (not `Bearer`).

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

use crate::{encode_wav, load_key, SttError, SttProvider};

pub struct DeepgramStt {
    api_key: String,
    model: String,
    client: reqwest::blocking::Client,
}

impl DeepgramStt {
    const ENDPOINT: &'static str = "https://api.deepgram.com/v1/listen";
    /// Deepgram's highest-performing general-purpose English model. `nova-2` is
    /// the previous generation (only for languages nova-3 lacks); older models
    /// (`nova`, `enhanced`, `base`) are legacy.
    pub const DEFAULT_MODEL: &'static str = "nova-3";
    /// The account name this provider's key is stored under (env var /
    /// `secrets.toml`).
    pub const KEY_ACCOUNT: &'static str = "deepgram";

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

    /// Build from the stored API key (env var or `secrets.toml`).
    pub fn from_stored_key(model: String) -> Result<Self, SttError> {
        let api_key =
            load_key(Self::KEY_ACCOUNT).ok_or_else(|| SttError::MissingKey(Self::KEY_ACCOUNT.into()))?;
        Ok(Self::new(api_key, model))
    }
}

#[derive(Deserialize)]
struct DeepgramResponse {
    results: Results,
}

#[derive(Deserialize)]
struct Results {
    channels: Vec<Channel>,
}

#[derive(Deserialize)]
struct Channel {
    alternatives: Vec<Alternative>,
}

#[derive(Deserialize)]
struct Alternative {
    transcript: String,
}

#[derive(Deserialize)]
struct DeepgramError {
    err_msg: Option<String>,
    request_id: Option<String>,
}

impl SttProvider for DeepgramStt {
    fn transcribe(&self, samples: &[f32], sample_rate: u32) -> Result<String, SttError> {
        let wav = encode_wav(samples, sample_rate)?;

        // Cleanup happens server-side (no LLM needed for most dictation):
        //   smart_format → punctuation, caps, numbers/dates/currency/URLs
        //   dictation    → spoken "period"/"comma"/"new line" become real marks
        //   punctuate    → explicit; also required by dictation
        //   language=multi → nova-3 multilingual/code-switching, the closest
        //                    analog to OpenAI's auto-detect (nova-3 defaults to
        //                    English-only if language is omitted entirely).
        // "um"/"uh" are stripped by default (filler_words defaults to false).
        // (Values are fixed/URL-safe, so building the query inline is fine.)
        let url = format!(
            "{}?model={}&smart_format=true&dictation=true&punctuate=true&language=multi",
            Self::ENDPOINT,
            self.model
        );

        let response = self
            .client
            .post(url)
            .header(AUTHORIZATION, format!("Token {}", self.api_key))
            .header(CONTENT_TYPE, "audio/wav")
            .body(wav)
            .send()?;

        let status = response.status();
        let body = response.text()?;

        if !status.is_success() {
            // Deepgram errors are `{err_code, err_msg, request_id}`; include the
            // request_id (support asks for it). Fall back to the raw body.
            let message = serde_json::from_str::<DeepgramError>(&body)
                .ok()
                .and_then(|e| {
                    e.err_msg.map(|m| match e.request_id {
                        Some(id) => format!("{m} (request_id: {id})"),
                        None => m,
                    })
                })
                .unwrap_or(body);
            return Err(SttError::Api(format!("HTTP {status}: {message}")));
        }

        let parsed = serde_json::from_str::<DeepgramResponse>(&body)
            .map_err(|e| SttError::Parse(e.to_string()))?;

        let transcript = parsed
            .results
            .channels
            .into_iter()
            .next()
            .and_then(|c| c.alternatives.into_iter().next())
            .map(|a| a.transcript.trim().to_string())
            .ok_or_else(|| SttError::Parse("response contained no transcript".into()))?;

        Ok(transcript)
    }

    fn name(&self) -> &str {
        "deepgram"
    }
}
