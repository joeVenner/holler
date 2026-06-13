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

    // `NSData::with_bytes` copies the slice into an owned NSData. SAFETY (below):
    // `AVAudioPlayer::alloc` + `initWithData:error:` is the documented designated
    // initialiser; the player and its run-loop pumping stay on this one thread.
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
                player.stop();
                break;
            }
            // Drain per slice: this worker thread has no ambient autorelease pool
            // (it isn't AppKit's main thread), so the `NSDate` we create each
            // iteration plus anything the run loop autoreleases would otherwise
            // accumulate for the whole utterance. Pumping inside a pool keeps the
            // long-playback path leak-free and stable.
            autoreleasepool(|_| {
                let until = NSDate::dateWithTimeIntervalSinceNow(POLL_SLICE_SECS);
                run_loop.runUntilDate(&until);
            });
        }
    }
    Ok(())
}
