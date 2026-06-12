fn main() {
    // AVFoundation provides `AVCaptureDevice`, which `permissions.rs` messages
    // for the live microphone authorization status. winit/AppKit don't load
    // it, so the class wouldn't be registered in the Objective-C runtime
    // unless we link the framework explicitly. macOS-only; a no-op elsewhere.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }
    // Windows: embed the .exe icon + version resource so Explorer, the taskbar
    // and file properties show Holler's branding. Gated to a Windows host — the
    // `winres` build-dep is only present there, and Windows builds run on a
    // Windows CI runner. A no-op on macOS/Linux.
    #[cfg(windows)]
    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    println!("cargo:rerun-if-changed=../../assets/holler.ico");
    let mut res = winres::WindowsResource::new();
    res.set_icon("../../assets/holler.ico");
    res.set("ProductName", "Holler");
    res.set("FileDescription", "Holler — push-to-talk dictation");
    if let Err(e) = res.compile() {
        // Don't fail the build over branding; surface it as a warning instead.
        println!("cargo:warning=winres failed to embed resources: {e}");
    }
}
