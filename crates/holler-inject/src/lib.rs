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

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

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
}

impl std::fmt::Display for InjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InjectError::Init(m) => write!(f, "input backend init failed (grant Accessibility?): {m}"),
            InjectError::Inject(m) => write!(f, "injection failed: {m}"),
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
}
