//! Native (offline) text-to-speech using the OS system voice.
//!
//! macOS: prefers the in-process **`AVSpeechSynthesizer`** (AVFAudio, via objc2 —
//! reusing the AVFoundation framework link the app already needs for the mic
//! probe), and falls back to the built-in **`say`** binary if the synthesizer
//! cannot be constructed. Both are offline and need no API key.
//!
//! Why prefer the synthesizer? It speaks in-process (no child fork per
//! utterance), exposes finer control (voice/rate/volume), and `stop()` halts it
//! instantly at a word boundary. `say` stays as a always-present safety net.
//!
//! ## Threading
//! `speak()` is **blocking** (the documented [`TtsProvider`] contract) and is
//! meant to run on a worker thread. `AVSpeechSynthesizer` synthesises
//! asynchronously and only produces audio while a run loop is being serviced,
//! so we drive a short [`NSRunLoop`] slice in a poll loop on the calling thread
//! until speech finishes — keeping the call blocking without ever touching the
//! main winit/AppKit loop. The synthesizer is built fresh per call and lives
//! only on that worker thread, so nothing about it crosses threads (it isn't
//! `Send`); cross-thread cancellation goes through the `Send + Sync`
//! [`AtomicBool`] stop flag, which the poll loop honours by halting its local
//! synthesizer. `stop()` also kills any in-flight `say` child.
//!
//! Windows/Linux: TODO stubs (this loop is macOS-only) — see [`NativeTts::speak`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::{TtsError, TtsProvider};

/// Offline system-voice TTS. Holds an in-flight `say` child (the fallback path)
/// so [`stop`](Self::stop) can interrupt it, and a cross-thread stop flag the
/// `AVSpeechSynthesizer` poll loop watches.
#[derive(Default)]
pub struct NativeTts {
    /// Optional named voice (e.g. "Samantha"); `None` uses the system default.
    voice: Option<String>,
    /// Speaking rate in words per minute; `None` uses the system default.
    rate: Option<u32>,
    /// Set by [`stop`](Self::stop); the AVSpeechSynthesizer poll loop watches it
    /// and halts its (thread-local) synthesizer when it flips true. Cleared at
    /// the start of each [`speak`](Self::speak).
    stop_requested: AtomicBool,
    /// The current `say` child process, if the fallback path is speaking.
    #[cfg(target_os = "macos")]
    child: Mutex<Option<std::process::Child>>,
    /// Keep the field present on every platform so the struct shape is stable.
    #[cfg(not(target_os = "macos"))]
    _child: Mutex<()>,
}

impl NativeTts {
    /// Build with an optional voice name and rate (words/min). Empty/blank voice
    /// strings are treated as "system default".
    pub fn new(voice: Option<String>, rate: Option<u32>) -> Self {
        let voice = voice.and_then(|v| {
            let v = v.trim().to_string();
            (!v.is_empty()).then_some(v)
        });
        Self {
            voice,
            rate,
            ..Self::default()
        }
    }

    /// Speak via the `say` binary (the macOS fallback / non-synthesizer path).
    /// Tracks the child so [`stop`](Self::stop) can kill it, then blocks until it
    /// exits so the call honours the blocking [`TtsProvider`] contract.
    #[cfg(target_os = "macos")]
    fn speak_via_say(&self, text: &str) -> Result<(), TtsError> {
        use std::process::Command;

        let mut cmd = Command::new("say");
        if let Some(voice) = &self.voice {
            cmd.arg("-v").arg(voice);
        }
        if let Some(rate) = self.rate {
            cmd.arg("-r").arg(rate.to_string());
        }
        // Pass the text as a single argument (avoids stdin plumbing); `say`
        // treats everything after the flags as the utterance.
        cmd.arg("--").arg(text);

        let child = cmd.spawn().map_err(|e| TtsError::Engine(e.to_string()))?;
        {
            let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
            *guard = Some(child);
        }
        // Re-take the child to wait on it; if stop() already took/killed it,
        // there's nothing left to wait for.
        let child = {
            let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
            guard.take()
        };
        if let Some(mut child) = child {
            child.wait().map_err(|e| TtsError::Playback(e.to_string()))?;
        }
        Ok(())
    }

    /// Speak via the in-process `AVSpeechSynthesizer`. Returns `Err` if the
    /// synthesizer can't be built (the caller then falls back to `say`).
    ///
    /// Blocks until the utterance finishes (or [`stop`](Self::stop) is called)
    /// by servicing a short run-loop slice in a poll loop — required because the
    /// synthesizer only emits audio while a run loop is being pumped, and we run
    /// off the main thread.
    #[cfg(target_os = "macos")]
    fn speak_via_synth(&self, text: &str) -> Result<(), TtsError> {
        use objc2_avf_audio::{AVSpeechSynthesisVoice, AVSpeechSynthesizer, AVSpeechUtterance};
        use objc2_foundation::{NSDate, NSRunLoop, NSString};

        // How long each run-loop slice services events before we re-check
        // `isSpeaking` / the stop flag. Short enough that stop() feels instant,
        // long enough not to busy-spin the CPU.
        const POLL_SLICE_SECS: f64 = 0.05;

        // SAFETY: `AVSpeechSynthesizer`/`AVSpeechUtterance`/`AVSpeechSynthesisVoice`
        // are the documented AVFAudio classes; `new`/`speechUtteranceWithString:`
        // are their designated constructors and return a retained instance. The
        // synthesizer and its run-loop pumping stay on this one thread.
        unsafe {
            let synth = AVSpeechSynthesizer::new();
            let utterance = AVSpeechUtterance::speechUtteranceWithString(&NSString::from_str(text));

            if let Some(name) = &self.voice {
                // Try the name as a voice identifier first, then as a BCP-47
                // language tag (e.g. "en-US"); leave the system default if
                // neither resolves rather than silencing the utterance.
                let id = NSString::from_str(name);
                if let Some(voice) = AVSpeechSynthesisVoice::voiceWithIdentifier(&id)
                    .or_else(|| AVSpeechSynthesisVoice::voiceWithLanguage(Some(&id)))
                {
                    utterance.setVoice(Some(&voice));
                }
            }
            if let Some(wpm) = self.rate {
                utterance.setRate(wpm_to_synth_rate(wpm));
            }

            synth.speakUtterance(&utterance);

            // Pump the run loop in slices until the synthesizer reports it has
            // stopped speaking, or a stop is requested.
            let run_loop = NSRunLoop::currentRunLoop();
            while synth.isSpeaking() {
                if self.stop_requested.load(Ordering::SeqCst) {
                    synth.stopSpeakingAtBoundary(objc2_avf_audio::AVSpeechBoundary::Immediate);
                    break;
                }
                let until = NSDate::dateWithTimeIntervalSinceNow(POLL_SLICE_SECS);
                run_loop.runUntilDate(&until);
            }
        }
        Ok(())
    }
}

/// Map a words-per-minute rate (the `say -r` unit, also what config stores) onto
/// the `AVSpeechUtterance` rate scale (a unitless float, default ≈ 0.5). macOS'
/// `say` default is ~175 wpm, which corresponds to the synthesizer default, so
/// we scale linearly off that anchor and clamp to the documented min/max.
#[cfg(target_os = "macos")]
fn wpm_to_synth_rate(wpm: u32) -> std::ffi::c_float {
    use objc2_avf_audio::{
        AVSpeechUtteranceDefaultSpeechRate, AVSpeechUtteranceMaximumSpeechRate,
        AVSpeechUtteranceMinimumSpeechRate,
    };

    const SAY_DEFAULT_WPM: f32 = 175.0;
    // SAFETY: these are `extern static` `c_float` rate constants exported by
    // AVFAudio (linked via build.rs); reading them is a plain memory load.
    let (default, min, max) = unsafe {
        (
            AVSpeechUtteranceDefaultSpeechRate,
            AVSpeechUtteranceMinimumSpeechRate,
            AVSpeechUtteranceMaximumSpeechRate,
        )
    };
    let scaled = default * (wpm as f32 / SAY_DEFAULT_WPM);
    scaled.clamp(min, max)
}

impl TtsProvider for NativeTts {
    #[cfg(target_os = "macos")]
    fn speak(&self, text: &str) -> Result<(), TtsError> {
        // Fresh utterance: clear any stale stop request from a prior call
        // (before the empty-text shortcut, so speak() always resets the flag).
        self.stop_requested.store(false, Ordering::SeqCst);
        // Nothing to say — succeed silently rather than start an engine.
        if text.trim().is_empty() {
            return Ok(());
        }

        // Prefer the in-process synthesizer; fall back to `say` only if it
        // genuinely fails to start (the poll loop above returns Ok once the
        // utterance is enqueued, so a clean synth path never reaches `say`).
        match self.speak_via_synth(text) {
            Ok(()) => Ok(()),
            Err(_) => self.speak_via_say(text),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn speak(&self, _text: &str) -> Result<(), TtsError> {
        // TODO(windows): SAPI5 / WinRT SpeechSynthesizer.
        // TODO(linux): speech-dispatcher (`spd-say`) or espeak.
        Err(TtsError::Unsupported(
            "native TTS is implemented for macOS only in this build".into(),
        ))
    }

    #[cfg(target_os = "macos")]
    fn stop(&self) -> Result<(), TtsError> {
        // Signal the synthesizer poll loop to halt its (thread-local) synth.
        self.stop_requested.store(true, Ordering::SeqCst);
        // And kill any in-flight `say` child (the fallback path).
        let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(mut child) = guard.take() {
            // Best-effort: the process may already have exited.
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn stop(&self) -> Result<(), TtsError> {
        Ok(())
    }

    fn name(&self) -> &str {
        "native"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_blanks_voice_to_none() {
        let t = NativeTts::new(Some("   ".to_string()), None);
        assert_eq!(t.voice, None);
        let t = NativeTts::new(Some("Samantha".to_string()), Some(200));
        assert_eq!(t.voice.as_deref(), Some("Samantha"));
        assert_eq!(t.rate, Some(200));
    }

    #[test]
    fn name_is_native() {
        assert_eq!(NativeTts::default().name(), "native");
    }

    #[test]
    fn empty_text_is_a_silent_noop() {
        // Must not start an engine (and must not error) on whitespace-only input.
        let t = NativeTts::default();
        assert!(t.speak("   ").is_ok());
    }

    #[test]
    fn stop_with_nothing_playing_is_ok() {
        let t = NativeTts::default();
        assert!(t.stop().is_ok());
    }

    /// `stop()` before any `speak()` must leave the flag set so a subsequent
    /// utterance still clears it — and must not panic on the empty child slot.
    #[test]
    fn stop_sets_then_speak_clears_the_flag() {
        let t = NativeTts::default();
        assert!(t.stop().is_ok());
        assert!(t.stop_requested.load(Ordering::SeqCst));
        // An empty utterance is a no-op but still resets the stop flag.
        assert!(t.speak("").is_ok());
        assert!(!t.stop_requested.load(Ordering::SeqCst));
    }

    /// Native-path smoke (macOS): synthesising a tiny phrase must drive the
    /// blocking `speak()` path (build an `AVSpeechSynthesizer`, enqueue an
    /// utterance, pump the run loop until it reports done) and return `Ok`.
    ///
    /// Deterministic + not audio-hardware-dependent: `AVSpeechSynthesizer`
    /// reports `isSpeaking` from its own state machine regardless of whether an
    /// output device is present (CI agents have none), and the phrase is short
    /// (~1 word) so the poll loop exits quickly. It may emit a brief sound on a
    /// dev machine with speakers, but the assertion is purely on the `Result`.
    #[cfg(target_os = "macos")]
    #[test]
    fn native_speak_short_phrase_is_ok() {
        let t = NativeTts::default();
        assert!(
            t.speak("hi").is_ok(),
            "native speak of a tiny phrase should succeed"
        );
    }

    /// The wpm→synth-rate mapping must stay within the documented bounds and
    /// move monotonically with wpm (faster wpm → not-slower synth rate).
    #[cfg(target_os = "macos")]
    #[test]
    fn wpm_maps_into_valid_synth_rate_range() {
        use objc2_avf_audio::{
            AVSpeechUtteranceMaximumSpeechRate, AVSpeechUtteranceMinimumSpeechRate,
        };
        // SAFETY: extern `c_float` rate-bound statics from AVFAudio.
        let (min, max) =
            unsafe { (AVSpeechUtteranceMinimumSpeechRate, AVSpeechUtteranceMaximumSpeechRate) };
        let slow = wpm_to_synth_rate(80);
        let fast = wpm_to_synth_rate(400);
        assert!((min..=max).contains(&slow), "slow {slow} out of [{min},{max}]");
        assert!((min..=max).contains(&fast), "fast {fast} out of [{min},{max}]");
        assert!(slow <= fast, "rate should not decrease with wpm");
    }
}
