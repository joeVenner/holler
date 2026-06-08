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
        // Windows doesn't have an equivalent grant flow for enigo.
        let _ = std::process::Command::new("ms-settings:privacy-general").spawn();
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
        let _ = std::process::Command::new("ms-settings:privacy-microphone").spawn();
    }
}
