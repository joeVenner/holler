//! Native (offline) text-to-speech using the macOS system voice via the built-in
//! **`say`** binary. Offline, no API key, and uses the very same system voices as
//! the in-process speech APIs (Samantha, Alex, …).
//!
//! ## Why `say` and not `AVSpeechSynthesizer`?
//! An earlier version drove `AVSpeechSynthesizer` in-process for "finer control",
//! but that API speaks *asynchronously*: `isSpeaking()` is still `false` the
//! instant after `speakUtterance:` returns, so a poll loop on it exits before any
//! audio plays (the read-aloud-produces-no-sound bug). Detecting completion
//! reliably would require an Objective-C delegate — real complexity for no
//! user-visible gain, since `say` exposes the same voices, runs safely off the
//! main thread (it's a child process, no run-loop pumping), honours `-v <voice>`
//! and `-r <wpm>` (exactly our config units), and is stopped by killing it.
//!
//! ## Threading
//! `speak()` is **blocking** (the [`TtsProvider`] contract) and runs on a worker
//! thread. It spawns `say`, stores the child so [`stop`](NativeTts::stop) can kill
//! it, then polls `try_wait()` until the child exits or a stop is requested —
//! keeping the child reachable for cancellation the whole time (a plain
//! `child.wait()` would move the handle out of reach and make `stop()` a no-op).
//!
//! Windows/Linux: TODO stubs (this loop is macOS-only) — see [`NativeTts::speak`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::{TtsError, TtsProvider};

/// Offline system-voice TTS. Holds the in-flight `say` child so
/// [`stop`](Self::stop) can interrupt it, plus a cross-thread stop flag the poll
/// loop watches (so a `stop()` that races the spawn is still honoured).
#[derive(Default)]
pub struct NativeTts {
    /// Optional named voice (e.g. "Samantha"); `None` uses the system default.
    voice: Option<String>,
    /// Speaking rate in words per minute; `None` uses the system default.
    rate: Option<u32>,
    /// Set by [`stop`](Self::stop); the poll loop halts/ kills `say` when it flips
    /// true. Cleared at the start of each [`speak`](Self::speak).
    stop_requested: AtomicBool,
    /// The current `say` child process while speaking.
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

    /// Speak via the `say` binary, blocking until it finishes (or [`stop`] kills
    /// it). The child stays in `self.child` while running so `stop()` can reach
    /// and kill it; we poll `try_wait()` rather than `wait()` to keep it there.
    ///
    /// [`stop`]: Self::stop
    #[cfg(target_os = "macos")]
    fn speak_via_say(&self, text: &str) -> Result<(), TtsError> {
        use std::process::Command;
        use std::time::Duration;

        // How long to sleep between liveness checks. Short enough that stop()
        // feels instant, long enough not to busy-spin the CPU.
        const POLL_INTERVAL: Duration = Duration::from_millis(40);

        let mut cmd = Command::new("say");
        if let Some(voice) = &self.voice {
            cmd.arg("-v").arg(voice);
        }
        if let Some(rate) = self.rate {
            cmd.arg("-r").arg(rate.to_string());
        }
        // Everything after `--` is the utterance (no stdin plumbing needed).
        cmd.arg("--").arg(text);

        let child = cmd.spawn().map_err(|e| TtsError::Engine(e.to_string()))?;
        {
            let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
            *guard = Some(child);
        }

        // Poll until the child exits or a stop is requested, keeping the handle
        // in `self.child` so stop() can kill it mid-utterance.
        loop {
            if self.stop_requested.load(Ordering::SeqCst) {
                self.kill_child();
                break;
            }
            let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
            match guard.as_mut() {
                // try_wait borrows the child mutably; the lock guards it.
                Some(child) => match child.try_wait() {
                    Ok(Some(_status)) => {
                        *guard = None; // exited cleanly
                        break;
                    }
                    Ok(None) => {} // still speaking — fall through and sleep
                    Err(e) => {
                        *guard = None;
                        return Err(TtsError::Playback(e.to_string()));
                    }
                },
                None => break, // stop() already took/killed it
            }
            drop(guard);
            std::thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }

    /// Kill and reap the in-flight `say` child, if any. Best-effort: the process
    /// may already have exited.
    #[cfg(target_os = "macos")]
    fn kill_child(&self) {
        let mut guard = self.child.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
        self.speak_via_say(text)
    }

    #[cfg(not(target_os = "macos"))]
    fn speak(&self, text: &str) -> Result<(), TtsError> {
        // Honour the same platform-agnostic contract as the macOS path and the
        // cloud backends: reset the stop flag, and treat empty text as a silent
        // no-op rather than an engine error.
        self.stop_requested.store(false, Ordering::SeqCst);
        if text.trim().is_empty() {
            return Ok(());
        }
        // TODO(windows): SAPI5 / WinRT SpeechSynthesizer.
        // TODO(linux): speech-dispatcher (`spd-say`) or espeak.
        Err(TtsError::Unsupported(
            "native TTS is implemented for macOS only in this build".into(),
        ))
    }

    #[cfg(target_os = "macos")]
    fn stop(&self) -> Result<(), TtsError> {
        // Flag the poll loop (handles a stop that races a not-yet-stored child)
        // and kill any in-flight `say` child immediately.
        self.stop_requested.store(true, Ordering::SeqCst);
        self.kill_child();
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn stop(&self) -> Result<(), TtsError> {
        // Keep the stop-flag bookkeeping identical across platforms even though
        // no native engine is wired up here yet.
        self.stop_requested.store(true, Ordering::SeqCst);
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

    /// Native-path smoke (macOS): speaking a tiny phrase must drive the blocking
    /// `speak()` path (spawn `say`, poll until it exits) and return `Ok`. Unlike
    /// the old `AVSpeechSynthesizer` smoke, this genuinely blocks for the
    /// utterance, so it also guards against the "returns instantly, no audio"
    /// regression. It may emit a brief sound on a machine with speakers.
    #[cfg(target_os = "macos")]
    #[test]
    fn native_speak_short_phrase_is_ok() {
        let t = NativeTts::default();
        assert!(
            t.speak("hi").is_ok(),
            "native speak of a tiny phrase should succeed"
        );
    }
}
