fn main() {
    // AVFoundation provides `AVCaptureDevice`, which `permissions.rs` messages
    // for the live microphone authorization status. winit/AppKit don't load
    // it, so the class wouldn't be registered in the Objective-C runtime
    // unless we link the framework explicitly. macOS-only; a no-op elsewhere.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }
}
