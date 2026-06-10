//! PTT key combo parser: config string → `HotKey` + human-readable label.
//!
//! The `global-hotkey` crate already handles `ctrl/alt/shift/cmd/super`
//! case-insensitively. This wrapper adds the two extra aliases Holler
//! documents (`meta` = `super`, `opt` = `alt`) and provides a fallback.

use global_hotkey::hotkey::{HotKey, Modifiers};

const DEFAULT_LABEL: &str = "Ctrl+Alt+Space";

fn default_hotkey() -> HotKey {
    HotKey::new(
        Some(Modifiers::CONTROL | Modifiers::ALT),
        global_hotkey::hotkey::Code::Space,
    )
}

/// Parse `raw` (e.g. `"ctrl+alt+space"`) into a `(HotKey, label)` pair.
///
/// Accepted modifiers: `ctrl`/`control`, `alt`/`option`/`opt`,
/// `cmd`/`command`/`super`/`meta`, `shift`.
/// On any parse error, logs a warning and returns the default `Ctrl+Alt+Space`.
pub fn parse_ptt_key(raw: &str) -> (HotKey, String) {
    match try_parse_ptt_key(raw) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("[holler] invalid ptt_key {raw:?}: {e}; falling back to {DEFAULT_LABEL}");
            (default_hotkey(), DEFAULT_LABEL.to_string())
        }
    }
}

/// Strict variant of [`parse_ptt_key`]: returns the parse error instead of
/// falling back, so interactive callers (the settings UI) can surface it.
pub fn try_parse_ptt_key(raw: &str) -> Result<(HotKey, String), String> {
    let normalized = raw
        .split('+')
        .map(|tok| match tok.trim().to_uppercase().as_str() {
            "META" => "SUPER".to_string(),
            "OPT" => "ALT".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+");

    normalized
        .parse::<HotKey>()
        .map(|hk| {
            let label = hotkey_label(&hk);
            (hk, label)
        })
        .map_err(|e| e.to_string())
}

/// Build a human-readable label from a parsed hotkey, e.g. `"Ctrl+Alt+Space"`.
fn hotkey_label(hk: &HotKey) -> String {
    let mut parts: Vec<String> = Vec::new();
    if hk.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if hk.mods.contains(Modifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if hk.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    if hk.mods.contains(Modifiers::SUPER) {
        parts.push("Cmd".to_string());
    }
    let key_raw = hk.key.to_string();
    let key_display = key_raw.strip_prefix("Key").unwrap_or(&key_raw);
    parts.push(key_display.to_string());
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use global_hotkey::hotkey::Code;

    #[test]
    fn default_combo_parses() {
        let (hk, label) = parse_ptt_key("ctrl+alt+space");
        assert_eq!(hk.mods, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(hk.key, Code::Space);
        assert_eq!(label, "Ctrl+Alt+Space");
    }

    #[test]
    fn case_insensitive() {
        let (hk, _) = parse_ptt_key("CTRL+ALT+SPACE");
        assert_eq!(hk.mods, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(hk.key, Code::Space);
    }

    #[test]
    fn alias_meta_maps_to_super() {
        let (hk, label) = parse_ptt_key("meta+alt+d");
        assert!(hk.mods.contains(Modifiers::SUPER));
        assert!(hk.mods.contains(Modifiers::ALT));
        assert_eq!(hk.key, Code::KeyD);
        assert!(label.contains("Cmd"), "label was: {label}");
    }

    #[test]
    fn alias_opt_maps_to_alt() {
        let (hk, _) = parse_ptt_key("ctrl+opt+space");
        assert!(hk.mods.contains(Modifiers::CONTROL));
        assert!(hk.mods.contains(Modifiers::ALT));
        assert_eq!(hk.key, Code::Space);
    }

    #[test]
    fn single_key_no_mods() {
        let (hk, label) = parse_ptt_key("f8");
        assert_eq!(hk.mods, Modifiers::empty());
        assert_eq!(hk.key, Code::F8);
        assert_eq!(label, "F8");
    }

    #[test]
    fn invalid_input_falls_back_to_default() {
        let (hk, label) = parse_ptt_key("notakey");
        assert_eq!(hk.mods, Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(hk.key, Code::Space);
        assert_eq!(label, DEFAULT_LABEL);
    }

    #[test]
    fn empty_string_falls_back_to_default() {
        let (hk, label) = parse_ptt_key("");
        assert_eq!(hk.key, Code::Space);
        assert_eq!(label, DEFAULT_LABEL);
    }

    #[test]
    fn try_parse_rejects_invalid_input() {
        assert!(try_parse_ptt_key("notakey").is_err());
        assert!(try_parse_ptt_key("").is_err());
        assert!(try_parse_ptt_key("ctrl+alt").is_err()); // modifiers only
    }

    #[test]
    fn try_parse_accepts_valid_input() {
        let (hk, label) = try_parse_ptt_key("cmd+shift+h").unwrap();
        assert!(hk.mods.contains(Modifiers::SUPER));
        assert!(hk.mods.contains(Modifiers::SHIFT));
        assert_eq!(hk.key, Code::KeyH);
        assert_eq!(label, "Shift+Cmd+H");
    }
}
