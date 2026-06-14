//! In-process audio playback for the cloud backends (macOS only).
//!
//! Both [`crate::OpenAiTts`] and [`crate::DeepgramTts`] fetch encoded audio
//! bytes over HTTP and play them through `AVAudioPlayer` — the AVFoundation link
//! the app already needs, so no extra audio crate. Factored out here so the two
//! providers share one tested playback path instead of duplicating the unsafe
//! Objective-C glue.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::TtsError;

/// Play encoded audio `bytes` (WAV in our case) in-process, blocking until
/// playback finishes or `stop_requested` flips true. Pumps a short `NSRunLoop`
/// slice per iteration (`AVAudioPlayer` callbacks are async) so the player runs
/// without ever touching the main winit/AppKit loop — this is called only on a
/// worker thread.
pub(crate) fn play_audio(bytes: &[u8], stop_requested: &AtomicBool) -> Result<(), TtsError> {
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::AnyThread;
    use objc2_avf_audio::AVAudioPlayer;
    use objc2_foundation::{NSData, NSDate, NSRunLoop};

    // How long each run-loop slice services events before re-checking playback
    // state / the stop flag.
    const POLL_SLICE_SECS: f64 = 0.05;

    // Wrap the whole player lifetime in an autorelease pool. This worker thread
    // has no ambient pool (it isn't AppKit's main thread), so the NSData, the
    // run-loop reference, and anything AVFoundation autoreleases during setup
    // and teardown would otherwise leak for the life of the thread. Draining
    // them here also guarantees teardown completes before we return to the
    // worker and start the next utterance.
    autoreleasepool(|_| {
        // `NSData::with_bytes` copies the slice into an owned NSData. SAFETY:
        // `AVAudioPlayer::alloc` + `initWithData:error:` is the documented
        // designated initialiser; the player and its run-loop pumping stay on
        // this one thread.
        let data = NSData::with_bytes(bytes);
        unsafe {
            let player: Retained<AVAudioPlayer> =
                AVAudioPlayer::initWithData_error(AVAudioPlayer::alloc(), &data)
                    .map_err(|e| TtsError::Playback(e.localizedDescription().to_string()))?;

            if !player.play() {
                return Err(TtsError::Playback("AVAudioPlayer refused to start".into()));
            }

            let run_loop = NSRunLoop::currentRunLoop();
            while player.isPlaying() {
                if stop_requested.load(Ordering::SeqCst) {
                    break;
                }
                // Pump the run loop a slice at a time inside its own pool so the
                // per-iteration NSDate doesn't accumulate over a long utterance.
                autoreleasepool(|_| {
                    let until = NSDate::dateWithTimeIntervalSinceNow(POLL_SLICE_SECS);
                    run_loop.runUntilDate(&until);
                });
            }

            // ALWAYS stop before the player is released — including on natural
            // completion. AVAudioPlayer posts its finish handling to this
            // thread's run loop; dropping (releasing) the player while that
            // callback is still pending is a hard crash with no Rust trace
            // (release builds are panic=abort). `stop()` detaches the player
            // from the run loop and cancels the pending callback synchronously,
            // so the release at the end of this scope is safe. This is the
            // "app quits when read-aloud finishes (cloud backend)" bug — the
            // Stop path already called stop(), which is why only natural
            // completion crashed.
            player.stop();
        }
        Ok(())
    })
}
