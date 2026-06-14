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

use holler_tts::{PreparedAudio, SpeakPhase, TtsError, TtsProvider};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

/// Text shorter than this is spoken as a single utterance, so the native macOS
/// voice keeps one smooth, gapless phrase. Only longer passages are batched.
const BATCH_MIN_TOTAL: usize = 600;
/// Soft upper bound (in chars) for one synthesis batch. Long text is split on
/// sentence boundaries into chunks no larger than this, so the first audio
/// starts quickly and a huge payload never hits the provider as one slow,
/// failure-prone request — the "read-aloud is slow and breaks" complaint.
const BATCH_MAX_CHARS: usize = 400;

/// Normalize text for speech synthesis. Collapses every run of whitespace —
/// including the double spaces and stray tabs/newlines you get pasting from a
/// terminal — down to a single space, and trims the ends. The result reads as
/// continuous prose, which is exactly what a TTS engine wants and which keeps a
/// messy clipboard from tripping up the provider.
fn clean_for_speech(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split cleaned text into synthesis batches on sentence boundaries.
///
/// Short text (`<= BATCH_MIN_TOTAL`) returns as a single batch so the native
/// voice stays gapless. Longer text is broken at `.`/`!`/`?` boundaries and the
/// resulting sentences greedily packed into batches of at most `BATCH_MAX_CHARS`
/// chars. A lone sentence longer than the cap is hard-split on word boundaries
/// so no single batch is unbounded. Whitespace-only input yields no batches.
fn split_into_batches(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if text.len() <= BATCH_MIN_TOTAL {
        return vec![text.to_string()];
    }

    let mut batches = Vec::new();
    let mut current = String::new();
    for sentence in sentences(text) {
        // A single oversized sentence: flush what we have, then hard-split it.
        if sentence.len() > BATCH_MAX_CHARS {
            if !current.is_empty() {
                batches.push(std::mem::take(&mut current));
            }
            batches.extend(hard_split(sentence, BATCH_MAX_CHARS));
            continue;
        }
        // Packing this sentence would overflow the batch — start a new one.
        if !current.is_empty() && current.len() + 1 + sentence.len() > BATCH_MAX_CHARS {
            batches.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(sentence);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Split prose into sentences, keeping each sentence's trailing terminator. A
/// boundary is `.`/`!`/`?` followed by whitespace; the input is assumed already
/// whitespace-collapsed by [`clean_for_speech`].
fn sentences(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if (c == b'.' || c == b'!' || c == b'?') && bytes.get(i + 1) == Some(&b' ') {
            out.push(text[start..=i].trim());
            start = i + 1;
        }
        i += 1;
    }
    if start < text.len() {
        out.push(text[start..].trim());
    }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// Hard-split an oversized sentence into chunks of at most `max` chars, breaking
/// on word boundaries (a single word longer than `max` is split mid-word as a
/// last resort so the cap is never exceeded).
fn hard_split(sentence: &str, max: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for word in sentence.split_whitespace() {
        if word.len() > max {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            // Break the over-long word into max-sized slices on char boundaries.
            let mut buf = String::new();
            for ch in word.chars() {
                if buf.len() + ch.len_utf8() > max {
                    chunks.push(std::mem::take(&mut buf));
                }
                buf.push(ch);
            }
            if !buf.is_empty() {
                current = buf;
            }
            continue;
        }
        if !current.is_empty() && current.len() + 1 + word.len() > max {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

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

                    // Clean the text once, then speak it batch-by-batch. Long
                    // passages start playing sooner (the first batch is small)
                    // and never hit the provider as one giant request. Cloud
                    // backends fetch the next batch(es) while the current one
                    // plays (no inter-batch silence); the native voice plays each
                    // batch in turn. Both re-check the epoch between batches so a
                    // newer request or a stop ends the run promptly instead of
                    // finishing the whole text.
                    let batches = split_into_batches(&clean_for_speech(&job.text));
                    let result = if job.provider.can_prefetch() {
                        speak_prefetched(&job, &batches, &worker_latest, &on_phase)
                    } else {
                        speak_sequential(&job, &batches, &worker_latest, &on_phase)
                    };
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

/// How many batches to keep synthesized ahead of playback. Two means the first
/// two batches are fetched up front and the next is fetched as soon as an
/// earlier one finishes playing — so playback rarely waits on the network, while
/// we never race far ahead of a listener who may stop partway through.
const PREFETCH_AHEAD: usize = 2;

/// Speak `batches` in order, hiding each cloud synthesis round-trip behind the
/// previous batch's playback. A detached background thread synthesizes ahead
/// (bounded to `PREFETCH_AHEAD` clips by the channel); this thread plays them in
/// order. The synthesizer stops as soon as a newer request or a stop bumps the
/// epoch, so we never fetch audio the listener will not hear. The native voice
/// never takes this path (it can't pre-synthesize); only cloud backends do.
fn speak_prefetched(
    job: &Job,
    batches: &[String],
    latest: &Arc<AtomicU64>,
    on_phase: &dyn Fn(SpeakPhase),
) -> Result<(), TtsError> {
    let (tx, rx) = mpsc::sync_channel::<Result<PreparedAudio, TtsError>>(PREFETCH_AHEAD);

    let synth_provider = Arc::clone(&job.provider);
    let synth_batches = batches.to_vec();
    let synth_latest = Arc::clone(latest);
    let synth_epoch = job.epoch;
    // Detached on purpose: if the worker stops early (epoch bumped), `rx` drops
    // when this fn returns, the synthesizer's next `send` fails, and the thread
    // exits. An in-flight synthesize finishes and is discarded rather than
    // blocking the worker from moving on to the next utterance.
    let _synth = std::thread::Builder::new()
        .name("holler-tts-prefetch".into())
        .spawn(move || {
            for batch in &synth_batches {
                if synth_epoch != synth_latest.load(Ordering::SeqCst) {
                    break;
                }
                let item = synth_provider.prepare(batch);
                let failed = item.is_err();
                // Stop on a closed channel (worker moved on) or after an error.
                if tx.send(item).is_err() || failed {
                    break;
                }
            }
        });

    // The first clip is on its way — surface "generating" once. Later clips are
    // already prefetched, so playback stays continuous from here on.
    on_phase(SpeakPhase::Synthesizing);
    let mut result = Ok(());
    for _ in 0..batches.len() {
        if job.epoch != latest.load(Ordering::SeqCst) {
            break;
        }
        match rx.recv() {
            Ok(Ok(audio)) => {
                result = job.provider.play_prepared(audio, on_phase);
                if result.is_err() {
                    break;
                }
            }
            Ok(Err(e)) => {
                result = Err(e);
                break;
            }
            // Synthesizer ended: epoch bumped, or every batch was produced.
            Err(_) => break,
        }
    }
    result
}

/// Speak `batches` one at a time with the blocking [`TtsProvider::speak`] — the
/// path for backends that can't pre-synthesize (the native voice, which is local
/// so inter-batch gaps are negligible). Cancels between batches on the epoch.
fn speak_sequential(
    job: &Job,
    batches: &[String],
    latest: &Arc<AtomicU64>,
    on_phase: &dyn Fn(SpeakPhase),
) -> Result<(), TtsError> {
    let mut result = Ok(());
    for batch in batches {
        if job.epoch != latest.load(Ordering::SeqCst) {
            break;
        }
        result = job.provider.speak(batch, on_phase);
        if result.is_err() {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_collapses_terminal_whitespace() {
        // The headline case: double spaces from a terminal copy.
        assert_eq!(clean_for_speech("hello   world"), "hello world");
        // Tabs, newlines and leading/trailing space all normalize away.
        assert_eq!(clean_for_speech("  a\tb\n\nc  "), "a b c");
        assert_eq!(clean_for_speech(""), "");
        assert_eq!(clean_for_speech("   \n\t "), "");
    }

    #[test]
    fn short_text_is_one_batch() {
        let s = "Just a short sentence.";
        assert_eq!(split_into_batches(s), vec![s.to_string()]);
        // Whitespace-only yields nothing to speak.
        assert!(split_into_batches("   ").is_empty());
    }

    #[test]
    fn long_text_splits_on_sentences_within_cap() {
        let sentence = "This is a sentence of some length to pad things out. ";
        let text = sentence.repeat(40); // ~2000 chars, well past BATCH_MIN_TOTAL
        let batches = split_into_batches(&clean_for_speech(&text));
        assert!(batches.len() > 1, "long text should split into many batches");
        for b in &batches {
            assert!(b.len() <= BATCH_MAX_CHARS, "batch over cap: {} chars", b.len());
            assert!(!b.is_empty());
        }
        // No content is lost: every sentence's words survive the split.
        let rejoined = batches.join(" ");
        assert_eq!(rejoined.matches("This is a sentence").count(), 40);
    }

    #[test]
    fn oversized_token_is_hard_split() {
        // A single "word" longer than the cap must still be bounded.
        let long_word = "x".repeat(BATCH_MAX_CHARS * 2 + 50);
        let text = format!("{long_word} and then some trailing words to exceed the minimum total length so batching actually kicks in for this case here now.");
        let padded = format!("{text} {}", "more padding words ".repeat(30));
        let batches = split_into_batches(&clean_for_speech(&padded));
        assert!(batches.iter().all(|b| b.len() <= BATCH_MAX_CHARS));
    }

    fn batches(of: &[&str]) -> Vec<String> {
        of.iter().map(|s| s.to_string()).collect()
    }

    fn job_with(provider: Arc<dyn TtsProvider>, epoch: u64) -> Job {
        Job { epoch, text: String::new(), provider }
    }

    /// A cloud-like backend: synthesizes audio from text (the bytes ARE the text,
    /// so playback can record what it played) and supports prefetch.
    struct FakeCloud {
        prepared: Mutex<Vec<String>>,
        played: Mutex<Vec<String>>,
    }
    impl FakeCloud {
        fn new() -> Arc<Self> {
            Arc::new(Self { prepared: Mutex::new(Vec::new()), played: Mutex::new(Vec::new()) })
        }
    }
    impl TtsProvider for FakeCloud {
        fn speak(&self, _t: &str, _p: &dyn Fn(SpeakPhase)) -> Result<(), TtsError> {
            panic!("prefetch backends must go through prepare/play_prepared, not speak");
        }
        fn stop(&self) -> Result<(), TtsError> {
            Ok(())
        }
        fn name(&self) -> &str {
            "fake-cloud"
        }
        fn can_prefetch(&self) -> bool {
            true
        }
        fn prepare(&self, text: &str) -> Result<PreparedAudio, TtsError> {
            lock(&self.prepared).push(text.to_string());
            Ok(PreparedAudio::new(text.as_bytes().to_vec()))
        }
        fn play_prepared(&self, audio: PreparedAudio, on_phase: &dyn Fn(SpeakPhase)) -> Result<(), TtsError> {
            on_phase(SpeakPhase::Playing);
            lock(&self.played).push(String::from_utf8_lossy(audio.as_bytes()).into_owned());
            Ok(())
        }
    }

    /// A native-like backend: blocking speak only, no prefetch capability.
    struct FakeNative {
        spoken: Mutex<Vec<String>>,
    }
    impl TtsProvider for FakeNative {
        fn speak(&self, text: &str, on_phase: &dyn Fn(SpeakPhase)) -> Result<(), TtsError> {
            on_phase(SpeakPhase::Playing);
            lock(&self.spoken).push(text.to_string());
            Ok(())
        }
        fn stop(&self) -> Result<(), TtsError> {
            Ok(())
        }
        fn name(&self) -> &str {
            "fake-native"
        }
    }

    #[test]
    fn prefetch_plays_and_synthesizes_every_batch_in_order() {
        let provider = FakeCloud::new();
        let latest = Arc::new(AtomicU64::new(7));
        let job = job_with(provider.clone(), 7);
        let bs = batches(&["one.", "two.", "three.", "four.", "five."]);
        assert!(speak_prefetched(&job, &bs, &latest, &|_| {}).is_ok());
        // Both synthesis and playback preserve batch order, and nothing is lost.
        assert_eq!(*lock(&provider.played), bs);
        assert_eq!(*lock(&provider.prepared), bs);
    }

    #[test]
    fn prefetch_cancelled_before_start_plays_nothing() {
        let provider = FakeCloud::new();
        // `latest` already past the job's epoch — the request was superseded.
        let latest = Arc::new(AtomicU64::new(9));
        let job = job_with(provider.clone(), 7);
        let bs = batches(&["a.", "b.", "c."]);
        assert!(speak_prefetched(&job, &bs, &latest, &|_| {}).is_ok());
        assert!(lock(&provider.played).is_empty(), "a stale job must not play");
    }

    #[test]
    fn sequential_speaks_each_batch_in_order() {
        let provider = Arc::new(FakeNative { spoken: Mutex::new(Vec::new()) });
        let latest = Arc::new(AtomicU64::new(3));
        let job = job_with(provider.clone(), 3);
        let bs = batches(&["x.", "y.", "z."]);
        assert!(speak_sequential(&job, &bs, &latest, &|_| {}).is_ok());
        assert_eq!(*lock(&provider.spoken), bs);
    }
}
