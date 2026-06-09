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
