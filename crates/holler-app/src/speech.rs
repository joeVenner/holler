//! Serialized read-aloud pipeline.
//!
//! Replaces the previous "spawn a fresh worker thread per utterance" model. That
//! model let cloud backends overlap: a new `speak()` reset the provider's shared
//! stop flag while the *old* worker was still mid-network, so two threads ended
//! up each driving their own `AVAudioPlayer` + `NSRunLoop` at once — unsynchronised
//! AVFoundation playback from N detached threads, which crashes hard with no
//! Rust-side trace (release builds are `panic = "abort"`). That is the "speak
//! aloud sometimes makes the app exit" bug.
//!
//! Here ONE long-lived worker thread processes requests strictly one at a time.
//! A newer request bumps a shared epoch and interrupts the in-flight provider via
//! `stop()`; the worker then only speaks (and only emits a terminal status for) a
//! job that is still the latest by the time it dequeues it. So rapid re-triggers
//! and any hotkey auto-repeat collapse to the last utterance instead of stacking
//! concurrent players. Progress is posted back to the winit loop via the proxy as
//! [`UserEvent::Speech`], the single source the status popup renders from.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use holler_tts::{SpeakPhase, TtsProvider};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Lifecycle of one read-aloud request, posted to the main loop as it advances.
/// `Triggered` is set synchronously by the app the instant a trigger fires (the
/// brief window before the worker dequeues); the rest are emitted by the worker,
/// except `Stopped`, which the app sets when the user cancels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechStatus {
    /// A read-aloud was just requested; nothing is generating yet.
    Triggered,
    /// Audio is being generated (cloud network request in flight).
    Generating,
    /// Audio is playing.
    Speaking,
    /// Playback completed on its own.
    Finished,
    /// The user stopped playback.
    Stopped,
    /// Synthesis or playback failed; carries a short message.
    Error(String),
}

/// A unit of work for the speech worker. Carries the provider so a settings
/// change (which rebuilds the provider) is naturally picked up next utterance.
struct Job {
    epoch: u64,
    text: String,
    provider: Arc<dyn TtsProvider>,
}

/// Owns the speech worker thread and the handles used to steer it from the main
/// loop. Cheap to hold; the worker simply parks on `recv()` when idle.
pub struct SpeechController {
    tx: Sender<Job>,
    /// The most recently requested epoch. The worker skips any job whose epoch is
    /// stale by the time it dequeues it, and suppresses its terminal status.
    latest: Arc<AtomicU64>,
    /// The provider currently mid-`speak()`, published by the worker so [`stop`]
    /// can interrupt it immediately regardless of which instance is playing.
    ///
    /// [`stop`]: Self::stop
    active: Arc<Mutex<Option<Arc<dyn TtsProvider>>>>,
    /// Monotonic epoch source (main-thread only — no atomic needed here).
    next_epoch: u64,
}

impl SpeechController {
    /// Spawn the worker thread. The thread lives for the process; dropping the
    /// controller closes the channel, which ends the worker's `recv()` loop.
    pub fn spawn(proxy: EventLoopProxy<UserEvent>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let latest = Arc::new(AtomicU64::new(0));
        let active: Arc<Mutex<Option<Arc<dyn TtsProvider>>>> = Arc::new(Mutex::new(None));

        let worker_latest = Arc::clone(&latest);
        let worker_active = Arc::clone(&active);
        std::thread::Builder::new()
            .name("holler-speech".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    // Superseded before we even started — drop it silently.
                    if job.epoch != worker_latest.load(Ordering::SeqCst) {
                        continue;
                    }

                    // Publish the active provider so stop() can interrupt us.
                    set_active(&worker_active, Some(Arc::clone(&job.provider)));

                    // Forward synthesis/playback transitions to the UI.
                    let phase_proxy = proxy.clone();
                    let on_phase = move |phase: SpeakPhase| {
                        let status = match phase {
                            SpeakPhase::Synthesizing => SpeechStatus::Generating,
                            SpeakPhase::Playing => SpeechStatus::Speaking,
                        };
                        let _ = phase_proxy.send_event(UserEvent::Speech(status));
                    };

                    let result = job.provider.speak(&job.text, &on_phase);
                    set_active(&worker_active, None);

                    // Only the still-current job owns the popup: if a newer
                    // utterance (or a stop) has bumped the epoch, stay quiet so we
                    // don't overwrite its status with our terminal one.
                    if job.epoch == worker_latest.load(Ordering::SeqCst) {
                        let status = match result {
                            Ok(()) => SpeechStatus::Finished,
                            Err(e) => SpeechStatus::Error(e.to_string()),
                        };
                        let _ = proxy.send_event(UserEvent::Speech(status));
                    } else if let Err(e) = result {
                        // Superseded jobs don't touch the UI, but a real error is
                        // still worth a log line for the diagnostics file.
                        eprintln!("[holler] read-aloud (superseded) failed: {e}");
                    }
                }
            })
            .expect("spawn speech worker thread");

        Self {
            tx,
            latest,
            active,
            next_epoch: 1,
        }
    }

    /// Queue `text` to be spoken with `provider`, cancelling anything in flight.
    /// Returns immediately; progress arrives as [`UserEvent::Speech`].
    pub fn speak(&mut self, text: String, provider: Arc<dyn TtsProvider>) {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        self.latest.store(epoch, Ordering::SeqCst);
        // Interrupt the in-flight utterance so the worker returns promptly and
        // moves on to this job. stop() is Send+Sync and returns at once.
        self.interrupt();
        if self.tx.send(Job { epoch, text, provider }).is_err() {
            eprintln!("[holler] speech worker is gone; read-aloud unavailable");
        }
    }

    /// Stop any in-flight speech. Advances the epoch with no job queued, so the
    /// current job becomes stale and the worker suppresses its terminal status —
    /// the caller is expected to surface [`SpeechStatus::Stopped`] itself.
    pub fn stop(&mut self) {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        self.latest.store(epoch, Ordering::SeqCst);
        self.interrupt();
    }

    /// Tell the provider currently mid-`speak()` to halt, if any.
    fn interrupt(&self) {
        if let Some(p) = lock(&self.active).as_ref() {
            let _ = p.stop();
        }
    }
}

/// Replace the published active provider. Factored out so the poison handling is
/// written once (we never hold the lock across a panic-prone call, so recovering
/// the guard is always safe).
fn set_active(slot: &Mutex<Option<Arc<dyn TtsProvider>>>, value: Option<Arc<dyn TtsProvider>>) {
    *lock(slot) = value;
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
