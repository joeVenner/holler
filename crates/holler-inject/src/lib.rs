//! Holler text injection (Phase 1).
//!
//! Delivers transcribed text at the active cursor via [`enigo`]. Two modes:
//!
//! - **Paste** (default): the caller has already put the text on the clipboard
//!   (Holler does this as its "copy memory" feature), so we just fire the OS
//!   paste chord — Cmd+V on macOS, Ctrl+V elsewhere. Fast, layout-independent,
//!   handles long text. The primary path.
//! - **Type**: simulate the keystrokes directly (`enigo.text`). Fallback for
//!   terminals/RDP that block synthetic paste; slower but needs no clipboard.
//!
//! macOS note: `enigo` uses CGEvent/TIS, which are **main-thread only** and
//! require Accessibility permission — so [`Injector`] must live and be used on
//! the main thread.

#[cfg(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "macos"))))]
use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// How long to wait between clipboard polls after firing the synthetic copy.
const COPY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);
/// How many times to poll the clipboard before giving up on the copy landing.
/// 25 × 20ms = 500ms total budget — generous for a local Cmd+C, never a busy spin.
const COPY_POLL_ATTEMPTS: u32 = 25;

/// How to deliver text to the focused app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InjectMode {
    /// Fire the OS paste chord (clipboard must already hold the text).
    #[default]
    Paste,
    /// Type the text as keystrokes.
    Type,
}

impl InjectMode {
    /// Parse a config string; unknown values fall back to the default (`Paste`).
    pub fn from_config(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "type" | "typing" | "keystroke" => InjectMode::Type,
            _ => InjectMode::Paste,
        }
    }
}

#[derive(Debug)]
pub enum InjectError {
    /// Could not initialise the input backend (e.g. Accessibility not granted).
    Init(String),
    /// A simulated keystroke/paste failed.
    Inject(String),
    /// The system clipboard could not be accessed.
    Clipboard(String),
}

impl std::fmt::Display for InjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InjectError::Init(m) => write!(f, "input backend init failed (grant Accessibility?): {m}"),
            InjectError::Inject(m) => write!(f, "injection failed: {m}"),
            InjectError::Clipboard(m) => write!(f, "clipboard access failed: {m}"),
        }
    }
}

impl std::error::Error for InjectError {}

/// Owns the `enigo` handle. Create and use on the main thread (macOS).
pub struct Injector {
    enigo: Enigo,
}

impl Injector {
    pub fn new() -> Result<Self, InjectError> {
        let enigo =
            Enigo::new(&Settings::default()).map_err(|e| InjectError::Init(e.to_string()))?;
        Ok(Self { enigo })
    }

    /// Deliver `text` using `mode`. In `Paste` mode the clipboard must already
    /// contain the text (`text` is then only used by the `Type` path).
    pub fn deliver(&mut self, text: &str, mode: InjectMode) -> Result<(), InjectError> {
        match mode {
            InjectMode::Paste => self.paste(),
            InjectMode::Type => self.type_text(text),
        }
    }

    /// Fire the platform paste chord (Cmd+V / Ctrl+V).
    pub fn paste(&mut self) -> Result<(), InjectError> {
        #[cfg(target_os = "macos")]
        let modifier = Key::Meta;
        #[cfg(not(target_os = "macos"))]
        let modifier = Key::Control;

        self.press(modifier, Direction::Press)?;
        self.press(Key::Unicode('v'), Direction::Click)?;
        self.press(modifier, Direction::Release)?;
        Ok(())
    }

    /// Type `text` directly as keystrokes (Unicode, layout-independent).
    pub fn type_text(&mut self, text: &str) -> Result<(), InjectError> {
        self.enigo
            .text(text)
            .map_err(|e| InjectError::Inject(e.to_string()))
    }

    fn press(&mut self, key: Key, direction: Direction) -> Result<(), InjectError> {
        self.enigo
            .key(key, direction)
            .map_err(|e| InjectError::Inject(e.to_string()))
    }

    /// Capture the user's current selection in the focused app by firing the OS
    /// copy chord (Cmd+C on macOS, Ctrl+C elsewhere) and reading what lands on
    /// the clipboard.
    ///
    /// Returns `Some(text)` with the selection, or `None` when nothing usable was
    /// captured (no selection, copy didn't land within the budget, or the
    /// clipboard was unavailable).
    ///
    /// Unlike the paste path — which *deliberately* leaves the transcript on the
    /// clipboard as Holler's "copy memory" — this reads the user's *own*
    /// selection on their behalf, so clobbering their clipboard would be
    /// surprising. We therefore **save the prior clipboard text and restore it**
    /// after capturing (best effort). The synthetic copy is asynchronous, so we
    /// poll with a bounded retry loop (see [`COPY_POLL_ATTEMPTS`]) until the new
    /// value differs from the saved one, rather than reading once and racing it.
    ///
    /// macOS / Windows / X11 only (anything `arboard` + `enigo` support).
    #[cfg(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "macos"))))]
    pub fn copy_selection(&mut self) -> Option<String> {
        let mut clipboard = match Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[holler-inject] {}", InjectError::Clipboard(e.to_string()));
                return None;
            }
        };

        // Snapshot the prior clipboard so we can (a) detect when the synthetic
        // copy lands by comparing against it and (b) restore it afterwards.
        // A missing/non-text clipboard reads as `None` — treated as "empty".
        let prior = clipboard.get_text().ok();

        if let Err(e) = self.copy() {
            eprintln!("[holler-inject] {e}");
            return None;
        }

        let captured = poll_clipboard(prior.as_deref(), COPY_POLL_ATTEMPTS, || {
            std::thread::sleep(COPY_POLL_INTERVAL);
            clipboard.get_text().ok()
        });

        // Restore the user's original clipboard (best effort — a restore failure
        // must not lose the captured selection).
        if let Some(prev) = prior {
            let _ = clipboard.set_text(prev);
        }

        captured
    }

    /// Non-macOS/Windows/Unix fallback: no input-synthesis backend, so selection
    /// capture is unsupported. TODO: wire up if a new target gains `enigo`/
    /// `arboard` support.
    #[cfg(not(any(target_os = "macos", target_os = "windows", all(unix, not(target_os = "macos")))))]
    pub fn copy_selection(&mut self) -> Option<String> {
        None
    }

    /// Fire the platform copy chord (Cmd+C / Ctrl+C).
    fn copy(&mut self) -> Result<(), InjectError> {
        #[cfg(target_os = "macos")]
        let modifier = Key::Meta;
        #[cfg(not(target_os = "macos"))]
        let modifier = Key::Control;

        self.press(modifier, Direction::Press)?;
        self.press(Key::Unicode('c'), Direction::Click)?;
        self.press(modifier, Direction::Release)?;
        Ok(())
    }
}

/// Pure clipboard-poll loop: call `read` up to `attempts` times, returning the
/// first **non-empty** value that differs from `prior` (the pre-copy contents).
/// Returns `None` if no fresh, non-empty value appears within the budget.
///
/// Factored out (with `read` injected) so the retry/timeout/empty-selection
/// logic is unit-testable without a live pasteboard or real key synthesis.
fn poll_clipboard<F>(prior: Option<&str>, attempts: u32, mut read: F) -> Option<String>
where
    F: FnMut() -> Option<String>,
{
    for _ in 0..attempts {
        if let Some(text) = read() {
            // A blank/whitespace-only result means nothing was selected, OR the
            // copy hasn't landed yet — either way, keep polling.
            if text.trim().is_empty() {
                continue;
            }
            // If the value still equals what was there before the copy, the
            // synthetic Cmd+C hasn't propagated yet — wait for it to change.
            // (When the user genuinely re-selected identical text this costs us
            // the full budget then returns None; acceptable — the caller treats
            // None as "nothing to read".)
            if prior == Some(text.as_str()) {
                continue;
            }
            return Some(text);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_mode_parses_case_insensitively() {
        assert_eq!(InjectMode::from_config("Type"), InjectMode::Type);
        assert_eq!(InjectMode::from_config("TYPING"), InjectMode::Type);
        assert_eq!(InjectMode::from_config("paste"), InjectMode::Paste);
        // Unknown -> default.
        assert_eq!(InjectMode::from_config("wat"), InjectMode::Paste);
        assert_eq!(InjectMode::default(), InjectMode::Paste);
    }

    /// Build a fake clipboard `read` closure that yields each item in `seq` on
    /// successive calls, then keeps repeating the last one (mimics a clipboard
    /// that eventually settles on a value).
    fn reader(seq: Vec<Option<&'static str>>) -> impl FnMut() -> Option<String> {
        let mut i = 0;
        move || {
            let v = seq.get(i).cloned().unwrap_or_else(|| seq.last().cloned().flatten().map(Some).unwrap_or(None));
            i += 1;
            v.map(|s| s.to_string())
        }
    }

    #[test]
    fn poll_returns_first_fresh_nonempty_value() {
        // Prior was "old"; copy lands "selected text" on the 3rd poll.
        let read = reader(vec![Some("old"), Some("old"), Some("selected text")]);
        assert_eq!(
            poll_clipboard(Some("old"), 25, read),
            Some("selected text".to_string())
        );
    }

    #[test]
    fn poll_skips_unchanged_clipboard() {
        // The synthetic copy never changes the clipboard (nothing was selected,
        // or it didn't land) — value stays equal to `prior` for all attempts.
        let read = reader(vec![Some("unchanged")]);
        assert_eq!(poll_clipboard(Some("unchanged"), 5, read), None);
    }

    #[test]
    fn poll_skips_blank_and_whitespace() {
        // Empty and whitespace-only reads are treated as "no selection yet".
        let read = reader(vec![Some(""), Some("   "), Some("\n\t")]);
        assert_eq!(poll_clipboard(None, 3, read), None);
    }

    #[test]
    fn poll_accepts_value_when_prior_was_empty() {
        // Empty clipboard before copy, then a real selection lands.
        let read = reader(vec![None, Some("hello")]);
        assert_eq!(poll_clipboard(None, 5, read), Some("hello".to_string()));
    }

    #[test]
    fn poll_gives_up_after_budget() {
        // Clipboard read always fails (None) — bounded, returns None, no spin.
        let read = reader(vec![None]);
        assert_eq!(poll_clipboard(Some("x"), 10, read), None);
    }

    #[test]
    fn poll_zero_attempts_is_none() {
        let read = reader(vec![Some("anything")]);
        assert_eq!(poll_clipboard(None, 0, read), None);
    }
}
