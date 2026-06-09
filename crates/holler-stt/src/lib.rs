//! Holler speech-to-text (Phase 1).
//!
//! A provider-agnostic [`SttProvider`] trait (locked BYOK decision in
//! `docs/DECISIONS.md`). Cloud providers live in their own modules:
//! [`OpenAiStt`] and [`DeepgramStt`]; local Whisper slots in behind the same
//! trait later.
//!
//! Transcription is **blocking** and meant to run on a worker thread — never on
//! the main winit/event loop. The batch (transcribe-on-release) path doesn't
//! benefit from async, so we use `reqwest::blocking` and skip a tokio runtime.
//!
//! API keys live in a local **`secrets.toml`** (or the `HOLLER_<PROVIDER>_KEY`
//! env var) via [`store_key`] / each provider's `from_stored_key` — managed by
//! `holler-config`, never in `config.toml`.

mod deepgram;
mod openai;
pub mod secrets;

use std::io::Cursor;

pub use deepgram::DeepgramStt;
pub use openai::OpenAiStt;
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
    /// No API key configured for this provider (env var or `secrets.toml`).
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
            SttError::MissingKey(m) => write!(f, "no API key configured for {m}"),
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

/// Encode mono f32 samples as 16-bit PCM WAV in memory — a container both
/// OpenAI and Deepgram accept directly (no re-encode). Shared by providers.
pub(crate) fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, SttError> {
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
