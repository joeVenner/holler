//! Native (offline) text-to-speech using the OS system voice.
//!
//! macOS: shells out to the built-in `say` binary — always present, offline,
//! and needs no API key. A future unit will prefer `AVSpeechSynthesizer` via
//! objc2 FFI (reusing the AVFoundation link added for the mic-permission probe)
//! for finer in-process control; `say` stays the documented fallback.
//!
//! Windows/Linux: TODO stubs (this loop is macOS-only) — see [`NativeTts::speak`].
//!
//! `stop()` interrupts in-progress speech by killing the tracked child process.
//! Each [`NativeTts`] keeps at most one live child (the most recent utterance);
//! `speak` is blocking, so the child is normally already reaped by the time a
//! caller on another thread calls `stop`.

use std::sync::Mutex;

use crate::{TtsError, TtsProvider};

/// Offline system-voice TTS. Holds the in-flight child so [`stop`](Self::stop)
/// can interrupt it from another thread.
#[derive(Default)]
pub struct NativeTts {
    /// Optional named voice (e.g. "Samantha"); `None` uses the system default.
    voice: Option<String>,
    /// Speaking rate in words per minute; `None` uses the system default.
    rate: Option<u32>,
    /// The current `say` child process, if one is speaking.
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
}

impl TtsProvider for NativeTts {
    #[cfg(target_os = "macos")]
    fn speak(&self, text: &str) -> Result<(), TtsError> {
        use std::process::Command;

        // Nothing to say — succeed silently rather than spawn an empty process.
        if text.trim().is_empty() {
            return Ok(());
        }

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

        // Track the child so stop() can kill it, then wait for it to finish so
        // speak() stays blocking (the documented contract).
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
        // Must not spawn a process (and must not error) on whitespace-only input.
        let t = NativeTts::default();
        assert!(t.speak("   ").is_ok());
    }

    #[test]
    fn stop_with_nothing_playing_is_ok() {
        let t = NativeTts::default();
        assert!(t.stop().is_ok());
    }
}
