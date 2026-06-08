//! Holler audio capture (Phase 1).
//!
//! Opens the default microphone **only while the PTT key is held** (PLAN.md §6:
//! the stream exists for the duration of a session and is dropped after), then
//! turns the recorded clip into the **16 kHz mono f32** buffer that
//! `whisper-rs` expects:
//!
//! ```text
//! cpal input (native rate, N channels, any sample format)
//!   -> normalise to f32 in the callback
//!   -> downmix to mono   (average channels)
//!   -> resample to 16 kHz (rubato sinc, anti-aliased — speech quality matters)
//! ```
//!
//! The cpal `Stream` is `!Send`, so [`AudioCapture`] must live on the thread
//! that created it — in Holler that is the main winit thread, which is exactly
//! where the PTT events arrive. No audio data crosses a thread boundary except
//! through the callback's shared buffer.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    calculate_cutoff, Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

/// The sample rate Whisper is trained on.
const TARGET_RATE: usize = 16_000;

/// Errors surfaced by the capture pipeline. Kept dependency-free (no
/// `thiserror`) — the variants carry a rendered message from the underlying
/// cpal/rubato error so callers can log without matching on foreign types.
#[derive(Debug)]
pub enum AudioError {
    NoInputDevice,
    Config(String),
    BuildStream(String),
    PlayStream(String),
    Resample(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::NoInputDevice => {
                write!(f, "no default input (microphone) device available")
            }
            AudioError::Config(m) => write!(f, "audio config error: {m}"),
            AudioError::BuildStream(m) => write!(f, "failed to build input stream: {m}"),
            AudioError::PlayStream(m) => write!(f, "failed to start input stream: {m}"),
            AudioError::Resample(m) => write!(f, "resampling failed: {m}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// A finished recording, ready for STT.
#[derive(Debug, Clone)]
pub struct Recording {
    /// Mono f32 samples at 16 kHz, normalised to roughly [-1.0, 1.0].
    pub samples: Vec<f32>,
    /// Wall-clock length of the clip, in seconds.
    pub duration_secs: f32,
}

/// An in-progress capture session. Created (and started) by [`AudioCapture::start`];
/// call [`AudioCapture::stop`] to end it and get the processed [`Recording`].
pub struct AudioCapture {
    // Kept alive for the session; dropping it stops the OS audio callback.
    stream: cpal::Stream,
    // The callback (on a high-priority audio thread) appends normalised f32
    // samples here, interleaved at the source channel count.
    buffer: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    channels: u16,
}

impl AudioCapture {
    /// Open the default input device and begin capturing immediately.
    pub fn start() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoInputDevice)?;

        let supported = device
            .default_input_config()
            .map_err(|e| AudioError::Config(e.to_string()))?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.config();
        let sample_rate = config.sample_rate;
        let channels = config.channels;

        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let sink = Arc::clone(&buffer);

        // cpal's typed `build_input_stream` is monomorphic over the sample
        // type, so we dispatch on the negotiated format. `f32::from_sample`
        // (via dasp) normalises every integer/float format into f32 uniformly.
        let stream = match sample_format {
            SampleFormat::F32 => build_input_stream::<f32>(&device, &config, sink),
            SampleFormat::F64 => build_input_stream::<f64>(&device, &config, sink),
            SampleFormat::I16 => build_input_stream::<i16>(&device, &config, sink),
            SampleFormat::I32 => build_input_stream::<i32>(&device, &config, sink),
            SampleFormat::I8 => build_input_stream::<i8>(&device, &config, sink),
            SampleFormat::U16 => build_input_stream::<u16>(&device, &config, sink),
            SampleFormat::U8 => build_input_stream::<u8>(&device, &config, sink),
            other => {
                return Err(AudioError::Config(format!(
                    "unsupported sample format: {other:?}"
                )))
            }
        }
        .map_err(|e| AudioError::BuildStream(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::PlayStream(e.to_string()))?;

        Ok(Self {
            stream,
            buffer,
            sample_rate,
            channels,
        })
    }

    /// Stop capturing and return the clip as 16 kHz mono f32.
    pub fn stop(self) -> Result<Recording, AudioError> {
        let Self {
            stream,
            buffer,
            sample_rate,
            channels,
        } = self;

        // Dropping the stream stops the OS callback synchronously, so after
        // this no further samples can be appended and the lock is contention-free.
        drop(stream);

        let raw = match buffer.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => return Err(AudioError::Resample("sample buffer lock poisoned".into())),
        };

        let mono = downmix_to_mono(&raw, channels);
        let samples = resample_to_16k(&mono, sample_rate)?;
        let duration_secs = samples.len() as f32 / TARGET_RATE as f32;

        Ok(Recording {
            samples,
            duration_secs,
        })
    }
}

/// Build an input stream whose callback normalises `T` samples to f32 and
/// appends them to the shared buffer. Uses `try_lock` so the realtime audio
/// thread never blocks — a dropped frame is preferable to a glitch.
fn build_input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    sink: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if let Ok(mut buf) = sink.try_lock() {
                buf.extend(data.iter().map(|&s| f32::from_sample(s)));
            }
        },
        |err| eprintln!("[holler-audio] stream error: {err}"),
        None,
    )
}

/// Average interleaved channels down to a single mono track. (Done before
/// resampling: fewer samples to resample, and Whisper wants mono anyway.)
fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Resample mono f32 from `src_rate` to 16 kHz using rubato's anti-aliased sinc
/// resampler. Returns the input untouched when it is already at 16 kHz.
fn resample_to_16k(mono: &[f32], src_rate: u32) -> Result<Vec<f32>, AudioError> {
    if mono.is_empty() {
        return Ok(Vec::new());
    }
    if src_rate as usize == TARGET_RATE {
        return Ok(mono.to_vec());
    }

    let channels = 1usize;
    let f_ratio = TARGET_RATE as f64 / src_rate as f64;

    // Sinc with a Blackman window: anti-aliased, speech-grade quality. (Faster
    // polynomial resamplers add high-frequency artifacts that hurt STT accuracy.)
    let sinc_len = 128;
    let window = WindowFunction::Blackman2;
    let params = SincInterpolationParameters {
        sinc_len,
        f_cutoff: calculate_cutoff(sinc_len, window),
        interpolation: SincInterpolationType::Quadratic,
        oversampling_factor: 256,
        window,
    };
    let mut resampler = Async::<f32>::new_sinc(f_ratio, 1.1, &params, 1024, channels, FixedAsync::Input)
        .map_err(|e| AudioError::Resample(e.to_string()))?;

    let nbr_input_frames = mono.len(); // channels == 1
    let resampler_delay = resampler.output_delay();
    let mut input_frames_next = resampler.input_frames_next();

    // Generous output capacity: expected frames, doubled, plus slack for the
    // resampler's internal delay and the final padded chunk.
    let mut outdata = vec![0.0f32; 2 * (nbr_input_frames as f64 * f_ratio) as usize + 4096];

    let input_adapter = InterleavedSlice::new(mono, channels, nbr_input_frames)
        .map_err(|e| AudioError::Resample(e.to_string()))?;
    let outdata_capacity = outdata.len() / channels;

    let mut indexing = Indexing {
        input_offset: 0,
        output_offset: 0,
        active_channels_mask: None,
        partial_len: None,
    };
    let mut input_frames_left = nbr_input_frames;

    // Scope the output adapter so its `&mut outdata` borrow ends before we read.
    {
        let mut output_adapter =
            InterleavedSlice::new_mut(&mut outdata, channels, outdata_capacity)
                .map_err(|e| AudioError::Resample(e.to_string()))?;

        // Full chunks first.
        while input_frames_left >= input_frames_next {
            let (nbr_in, nbr_out) = resampler
                .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
                .map_err(|e| AudioError::Resample(e.to_string()))?;
            indexing.input_offset += nbr_in;
            indexing.output_offset += nbr_out;
            input_frames_left -= nbr_in;
            input_frames_next = resampler.input_frames_next();
        }

        // Final partial chunk: rubato pads the missing frames with silence.
        indexing.partial_len = Some(input_frames_left);
        resampler
            .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
            .map_err(|e| AudioError::Resample(e.to_string()))?;
    }

    // Trim the resampler's leading delay and keep the expected frame count.
    let expected_frames = (nbr_input_frames as f64 * f_ratio) as usize;
    let start = resampler_delay.min(outdata.len());
    let end = (start + expected_frames).min(outdata.len());
    Ok(outdata[start..end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_stereo_averages_channels() {
        // L/R interleaved: (0.0,1.0), (1.0,1.0) -> 0.5, 1.0
        let stereo = [0.0, 1.0, 1.0, 1.0];
        assert_eq!(downmix_to_mono(&stereo, 2), vec![0.5, 1.0]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let mono = [0.1, 0.2, 0.3];
        assert_eq!(downmix_to_mono(&mono, 1), mono.to_vec());
    }

    #[test]
    fn resample_passthrough_at_16k() {
        let samples = vec![0.1, -0.2, 0.3];
        assert_eq!(resample_to_16k(&samples, 16_000).unwrap(), samples);
    }

    #[test]
    fn resample_48k_to_16k_thirds_the_length() {
        // 48 kHz -> 16 kHz is a 1:3 ratio, so ~1/3 the samples (allow slack
        // for the resampler's edge handling).
        let one_sec_48k = vec![0.0f32; 48_000];
        let out = resample_to_16k(&one_sec_48k, 48_000).unwrap();
        let expected = 16_000;
        let diff = (out.len() as i64 - expected as i64).unsigned_abs() as usize;
        assert!(
            diff <= 256,
            "expected ~{expected} samples, got {} (diff {diff})",
            out.len()
        );
    }
}
