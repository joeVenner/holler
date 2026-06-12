//! Platform-specific permission checks.

/// Returns true if Accessibility (assistive technology) access is granted.
/// Used to decide whether auto-paste via key simulation will work.
pub fn accessibility_granted() -> bool {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows: enigo generally works without special permission grants.
        true
    }
}

/// True when **Secure Keyboard Entry** is active anywhere on the system.
///
/// macOS lets an app (Terminal/iTerm via their "Secure Keyboard Entry" menu
/// item, and any password field) lock keyboard input so no other process can
/// observe *or inject* events while that app is frontmost. Holler's auto-paste
/// fires a synthetic Cmd+V (and Type mode synthesises keystrokes) — both are
/// silently swallowed under secure input, which is exactly why dictation pastes
/// fine into most apps but does nothing in a terminal that has it enabled.
///
/// The flag is global but effectively tracks the frontmost secure app, so it
/// reads true at the moment we'd inject into such an app and false otherwise —
/// good enough to decide whether to fall back to a manual-paste toast.
pub fn secure_keyboard_entry_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            // Carbon HIToolbox: returns a `Boolean` (unsigned char). Linked via
            // build.rs (`framework=Carbon`).
            fn IsSecureEventInputEnabled() -> u8;
        }
        unsafe { IsSecureEventInputEnabled() != 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Microphone privacy authorization. macOS gates microphone access per app
/// (System Settings → Privacy & Security → Microphone). Other platforms have
/// no equivalent per-app prompt, so the status there is always [`Granted`].
///
/// [`Granted`]: MicStatus::Granted
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicStatus {
    /// The app may capture audio.
    Granted,
    /// The user has denied access; recordings stay silent until re-enabled.
    Denied,
    /// Not asked yet — the first recording shows the OS prompt (capture works).
    NotDetermined,
    /// Blocked by MDM / parental controls; the user cannot change it.
    Restricted,
}

/// Current microphone authorization. This is a pure query — only an actual
/// capture (or `requestAccessForMediaType:`) ever shows a prompt — so it is
/// safe to call on a poll while the settings window is open.
pub fn microphone_status() -> MicStatus {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::AnyClass;
        use objc2_foundation::NSString;

        // [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio].
        // AVMediaTypeAudio is the constant NSString @"soun".
        let Some(cls) = AnyClass::get(c"AVCaptureDevice") else {
            // AVFoundation somehow not loaded — assume usable rather than
            // wrongly reporting a permission problem we couldn't read.
            return MicStatus::Granted;
        };
        let media_type = NSString::from_str("soun");
        // SAFETY: documented class method `(AVMediaType) -> AVAuthorizationStatus`
        // (an NSInteger). The NSString argument outlives the synchronous call.
        let raw: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: &*media_type] };
        match raw {
            3 => MicStatus::Granted,
            2 => MicStatus::Denied,
            1 => MicStatus::Restricted,
            // 0 = NotDetermined; treat any unexpected value the same (capture
            // still prompts, so this is the safe default).
            _ => MicStatus::NotDetermined,
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        MicStatus::Granted
    }
}

/// Open the OS panel where the user can grant Accessibility access.
pub fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        // Windows has no Accessibility-style grant for enigo: input injection
        // works without one and can only fail against windows running as
        // Administrator (UIPI). `accessibility_granted()` returns true here, so
        // the tray's grant item is disabled and this is effectively unreachable
        // — just log instead of opening an unrelated Settings panel.
        println!("[holler] Windows: auto-paste needs no permission; it can only fail against apps run as Administrator.");
    }
}

/// Open System Settings/Preferences at the microphone privacy panel.
pub fn open_mic_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        // `ms-settings:` is a URI protocol, not an executable — launch it via
        // the shell handler (explorer.exe). Passing it straight to Command::new
        // fails with ERROR_FILE_NOT_FOUND, so the tray action did nothing.
        let _ = std::process::Command::new("explorer.exe")
            .arg("ms-settings:privacy-microphone")
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: the live status queries must return without panicking. On
    /// macOS this exercises the AVCaptureDevice Objective-C messaging path
    /// (class lookup + `msg_send`), catching a broken AVFoundation link or a
    /// wrong selector signature in CI before it ships.
    #[test]
    fn status_queries_do_not_panic() {
        let _ = accessibility_granted();
        let _ = microphone_status();
        // Exercises the Carbon `IsSecureEventInputEnabled` link on macOS.
        let _ = secure_keyboard_entry_enabled();
    }
}
