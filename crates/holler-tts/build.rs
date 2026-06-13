fn main() {
    // `AVSpeechSynthesizer` / `AVSpeechUtterance` live in the AVFAudio framework
    // (re-exported through the AVFoundation umbrella). objc2-avf-audio only
    // declares the bindings — the symbols still have to be linked in, and
    // winit/AppKit don't pull AVFoundation, so the classes wouldn't be
    // registered in the Objective-C runtime without this. macOS-only; a no-op
    // on every other host (the objc2 deps are cfg-gated to macOS too).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=AVFoundation");
    }
}
