// On Windows, build the RELEASE binary as a GUI-subsystem app so launching it
// from Explorer doesn't pop a persistent black console window — Holler is a
// tray agent with no console (the macOS analog is Info.plist LSUIElement). Left
// off in debug builds so `cargo run` keeps println!/eprintln! diagnostics.
// Trade-off: under the windows subsystem, `holler.exe set-key …` run from a
// terminal won't print to it (it still stores the key and exits correctly).
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

//! Holler — push-to-talk dictation.
//!
//! A SINGLE main-thread `winit` event loop owns the `tray-icon` and the
//! `global-hotkey` push-to-talk receiver (CLAUDE.md / docs/PLAN.md §0 & §34).
//! Holding the PTT key records the mic; releasing it transcribes the clip.
//!
//! The loop must never block: capture runs on cpal's own audio thread, and
//! transcription (a network call) runs on a spawned worker thread that posts
//! the result back as a `UserEvent` via the `EventLoopProxy`.
//!
//! CLI: `holler set-key openai <KEY>` stores an API key in `secrets.toml`.

mod icons;
mod overlay;
mod permissions;
mod settings;
mod toast;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arboard::Clipboard;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use holler_audio::{AudioCapture, Recording};
use holler_config::Config;
use holler_inject::{InjectMode, Injector};
use holler_stt::{DeepgramStt, OpenAiStt, SttProvider};
use holler_store::History;
use mimalloc::MiMalloc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use winit::{
    application::ApplicationHandler,
    event::{StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::WindowId,
};
use overlay::Overlay;
use settings::{SettingsAction, SettingsWindow};
use toast::Toast;

/// How often the tray animation advances a frame.
const FRAME_INTERVAL: Duration = Duration::from_millis(90);
/// How often to re-check Accessibility permission so the tray reflects reality
/// without requiring a restart.
const AX_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Visible tray state. `Idle` is static; the others animate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayState {
    Idle,
    Recording,
    Processing,
}

/// Lower idle RSS — the whole point of a tray-resident app (PLAN.md §6).
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Everything that can wake the loop, funnelled through one channel. Tray and
/// hotkey callbacks fire on OS background threads, so they forward into the
/// winit loop via `EventLoopProxy` rather than touching state directly.
#[derive(Debug)]
enum UserEvent {
    Hotkey(GlobalHotKeyEvent),
    Tray(TrayIconEvent),
    Menu(MenuEvent),
    /// A finished transcription (or a rendered error) from the worker thread.
    /// Carried back into the loop so delivery stays on the main thread.
    Transcript(Result<Transcription, String>),
    /// egui asked for a repaint of the settings window after this delay
    /// (zero = now). Sent from the repaint callback installed on its Context.
    SettingsRepaint(Duration),
}

/// A successful transcription plus which provider produced it (for history).
#[derive(Debug)]
struct Transcription {
    text: String,
    provider: String,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    // Both are created on the main thread *after* the loop starts (macOS
    // requirement) and kept alive for the process: dropping the manager
    // unregisters the hotkey, dropping the tray removes the menu-bar icon.
    hotkeys: Option<GlobalHotKeyManager>,
    tray: Option<TrayIcon>,
    /// Id of the currently registered PTT hotkey; `None` while registration
    /// has failed (combo taken) — recoverable live via Settings → Hotkey.
    ptt_hotkey_id: Option<u32>,
    /// Human-readable PTT combo (e.g. "Ctrl+Alt+Space"), used to rebuild the
    /// default tray tooltip after a transient error message.
    ptt_label: String,
    quit_item_id: Option<MenuId>,
    config_item_id: Option<MenuId>,
    history_item_id: Option<MenuId>,
    /// Current visible tray state and the animation frame within it.
    tray_state: TrayState,
    anim_frame: usize,
    /// True while the PTT key is physically held. The edge detector that makes
    /// this the source of truth is what debounces OS key auto-repeat.
    ptt_held: bool,
    /// The live mic capture, present only between PTT down and up. cpal's
    /// `Stream` is `!Send`, so this (and `App`) stay on the main thread.
    capture: Option<AudioCapture>,
    /// Non-secret settings (provider, model, injection mode). Loaded at startup.
    config: Config,
    /// Transcript history (SQLite). Opened at startup; `None` if it failed.
    history: Option<History>,
    /// Clipboard + input simulation are lazily created on first use — both are
    /// main-thread/`!Send` and the injector can trigger an Accessibility prompt,
    /// so we keep them off the launch path.
    clipboard: Option<Clipboard>,
    injector: Option<Injector>,
    /// Cached result of AXIsProcessTrusted() / equivalent. Re-checked each time
    /// `ensure_injector` is called in case the user granted it while running.
    accessibility_ok: bool,
    /// Avoid flooding the log with the same "grant Accessibility" note.
    accessibility_warned: bool,
    /// Tray menu item IDs (for click routing).
    grant_access_item_id: Option<MenuId>,
    grant_mic_item_id: Option<MenuId>,
    /// The tray menu (retained clone — muda's `Menu` is a shared handle, so
    /// mutating this reflects in the live NSMenu) plus the two Grant items and
    /// their group separator. The permission poll shows/hides each Grant entry
    /// so it appears ONLY while that permission is actually missing — no
    /// permanent disabled status rows cluttering the menu.
    tray_menu: Option<Menu>,
    grant_access_menu_item: Option<MenuItem>,
    grant_mic_menu_item: Option<MenuItem>,
    grant_block_sep: Option<PredefinedMenuItem>,
    /// Last microphone status seen by the poll — drives live tray/panel updates.
    last_mic_status: permissions::MicStatus,
    /// Timestamp of the last permission poll (rate-limits AXIsProcessTrusted /
    /// microphone-status checks).
    last_ax_check: Instant,
    /// Desktop recording indicator shown during PTT hold.
    overlay: Option<Overlay>,
    /// "Copied to clipboard — paste it" toast, shown when auto-paste can't run.
    toast: Option<Toast>,
    /// OS text-to-speech engine for the History "Read aloud" action. Lazily
    /// constructed on first use (nothing on the launch path); kept alive after
    /// because the engine speaks asynchronously and dropping it cuts the audio.
    tts: Option<tts::Tts>,
    /// When the toast should auto-dismiss; drives a `WaitUntil` wake like the
    /// settings repaint. `None` when no toast is showing.
    toast_hide_at: Option<Instant>,
    /// The egui settings window — exists only while open (PLAN.md §6).
    settings: Option<SettingsWindow>,
    /// When egui asked to be repainted (cursor blink, animations). Drives a
    /// `ControlFlow::WaitUntil` wake; cleared once the redraw is requested.
    settings_repaint_at: Option<Instant>,
    settings_item_id: Option<MenuId>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        // Keys are resolved lazily on the worker thread at PTT-release, not
        // here — so the launch path touches no secrets and stays snappy. Config
        // (filesystem) and history (SQLite) are cheap, so they load eagerly.
        let config = holler_config::load_or_create().unwrap_or_else(|e| {
            eprintln!("[holler] config load failed ({e}); using defaults.");
            Config::default()
        });
        let history = History::open_default()
            .map_err(|e| eprintln!("[holler] history db unavailable ({e}); not recording."))
            .ok();

        let accessibility_ok = permissions::accessibility_granted();
        if !accessibility_ok {
            println!("[holler] Accessibility not granted — auto-paste disabled. Grant via tray menu.");
        }

        // Raise the microphone prompt now (main thread) if undecided. cpal's
        // CoreAudio capture never triggers it, so without this the first
        // recording would be silent and the grant would never be requested.
        permissions::request_microphone_access();

        Self {
            proxy,
            hotkeys: None,
            tray: None,
            ptt_hotkey_id: None,
            ptt_label: String::new(),
            quit_item_id: None,
            config_item_id: None,
            history_item_id: None,
            grant_access_item_id: None,
            grant_mic_item_id: None,
            tray_menu: None,
            grant_access_menu_item: None,
            grant_mic_menu_item: None,
            grant_block_sep: None,
            last_mic_status: permissions::microphone_status(),
            last_ax_check: Instant::now(),
            tray_state: TrayState::Idle,
            anim_frame: 0,
            ptt_held: false,
            capture: None,
            config,
            history,
            clipboard: None,
            injector: None,
            accessibility_ok,
            accessibility_warned: false,
            overlay: None,
            toast: None,
            tts: None,
            toast_hide_at: None,
            settings: None,
            settings_repaint_at: None,
            settings_item_id: None,
        }
    }

    /// Build everything that must live on the main thread once the loop runs.
    /// Idempotent: `resumed` can fire more than once on some platforms. Guarded
    /// on the tray (not the hotkey) so a failed PTT registration still can't
    /// rebuild the tray on re-entry.
    fn init(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray.is_some() {
            return;
        }

        // --- Tray icon + menu ---
        // The menu stays native (NSMenu on macOS) and lean: the always-actions
        // (Settings / history / Quit) are appended once here; the two "Grant …"
        // prompts are inserted at the top only while a permission is missing
        // (see `rebuild_grant_block`, driven by the permission poll).
        let menu = Menu::new();

        // Always-present actions.
        let settings_item = MenuItem::new("Settings…", true, None);
        let config_item = MenuItem::new("Edit Settings (config.toml)…", true, None);
        let history_item = MenuItem::new("Open History Folder…", true, None);
        menu.append(&settings_item).expect("append settings item");
        menu.append(&config_item).expect("append config item");
        menu.append(&history_item).expect("append history item");
        menu.append(&PredefinedMenuItem::separator()).expect("separator");
        let quit_item = MenuItem::new("Quit Holler", true, None);
        menu.append(&quit_item).expect("append Quit menu item");
        self.settings_item_id = Some(settings_item.id().clone());
        self.config_item_id = Some(config_item.id().clone());
        self.history_item_id = Some(history_item.id().clone());
        self.quit_item_id = Some(quit_item.id().clone());

        // Grant prompts — built once, shown on demand. Clean labels, no leading
        // spaces or ✓/✗ glyphs; they only ever appear when actionable.
        let grant_access_item = MenuItem::new("Grant Accessibility Access…", true, None);
        let grant_mic_item = MenuItem::new("Grant Microphone Access…", true, None);
        self.grant_access_item_id = Some(grant_access_item.id().clone());
        self.grant_mic_item_id = Some(grant_mic_item.id().clone());
        self.grant_access_menu_item = Some(grant_access_item);
        self.grant_mic_menu_item = Some(grant_mic_item);
        self.grant_block_sep = Some(PredefinedMenuItem::separator());
        // Retain a handle to the live menu, then reconcile the Grant block to
        // the current permission state before the tray is shown.
        self.tray_menu = Some(menu.clone());
        self.rebuild_grant_block();

        // Parse the PTT combo from config; falls back to Ctrl+Alt+Space on error.
        let (ptt_hotkey, ptt_label) = holler_config::parse_ptt_key(&self.config.ptt_key);
        self.ptt_label = ptt_label.clone();

        let tray = TrayIconBuilder::new()
            .with_tooltip(format!("Holler — hold {ptt_label} to talk"))
            .with_icon(state_icon(TrayState::Idle, 0).expect("build initial tray icon"))
            .with_menu(Box::new(menu))
            .build()
            .expect("build tray icon");
        self.tray = Some(tray);

        // Forward tray + menu events (OS threads) into the loop via the proxy.
        let proxy = self.proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |e| {
            let _ = proxy.send_event(UserEvent::Tray(e));
        }));
        let proxy = self.proxy.clone();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = proxy.send_event(UserEvent::Menu(e));
        }));

        // --- Global hotkey (PTT) ---
        // Registration can fail at runtime — most commonly on Windows when the
        // combo is already owned by another app/IME, but the manager itself can
        // fail too. Under panic="abort" an .expect() here hard-kills the whole
        // tray app at launch with no visible message. Degrade gracefully: keep
        // the manager (and the tray/menu/overlay) alive with PTT disabled —
        // the user can pick a free combo live via Settings → Hotkey.
        match GlobalHotKeyManager::new() {
            Ok(manager) => {
                match manager.register(ptt_hotkey) {
                    Ok(()) => self.ptt_hotkey_id = Some(ptt_hotkey.id()),
                    Err(e) => {
                        eprintln!(
                            "[holler] could not register PTT key {ptt_label} — it may already be \
                             in use by another app. Pick another combo in Settings → Hotkey ({e})."
                        );
                        self.set_tray_tooltip(
                            "Holler — PTT key unavailable; pick another in Settings → Hotkey",
                        );
                    }
                }
                self.hotkeys = Some(manager);

                // `global-hotkey` has no callback API — only a static channel.
                // Drain it on a dedicated thread that BLOCKS on recv() and
                // forwards via the proxy, so the main loop can stay in
                // ControlFlow::Wait (event-driven, no polling — PLAN.md §6)
                // yet still wake instantly on a key event.
                let proxy = self.proxy.clone();
                std::thread::Builder::new()
                    .name("holler-hotkey-rx".into())
                    .spawn(move || {
                        let rx = GlobalHotKeyEvent::receiver();
                        while let Ok(event) = rx.recv() {
                            if proxy.send_event(UserEvent::Hotkey(event)).is_err() {
                                break; // loop is gone — stop forwarding.
                            }
                        }
                    })
                    .expect("spawn hotkey forwarder thread");
            }
            Err(e) => {
                eprintln!("[holler] could not initialise global hotkeys ({e}); push-to-talk disabled.");
            }
        }

        // Desktop recording indicator — created lazily here so we have an event loop.
        self.overlay = Overlay::create(event_loop);
        if self.overlay.is_none() {
            eprintln!("[holler] overlay window unavailable (non-fatal)");
        }
        // Clipboard-fallback toast — created hidden, shown on demand from deliver().
        self.toast = Toast::create(event_loop);
        if self.toast.is_none() {
            eprintln!("[holler] toast window unavailable (non-fatal)");
        }

        println!("[holler] ready — hold {ptt_label} to talk; tray menu → Quit to exit.");
    }

    fn on_hotkey(&mut self, event: GlobalHotKeyEvent) {
        if Some(event.id) != self.ptt_hotkey_id {
            return;
        }
        match event.state {
            HotKeyState::Pressed => {
                if !self.ptt_held {
                    self.ptt_held = true;
                    // Open the mic only for the duration of the hold (PLAN.md §6).
                    match AudioCapture::start() {
                        Ok(capture) => {
                            self.capture = Some(capture);
                            self.reset_tray_tooltip(); // clear any prior error hint
                            self.set_tray_state(TrayState::Recording); // also shows the overlay
                            println!("[holler] PTT DOWN — recording…");
                        }
                        Err(e) => {
                            eprintln!("[holler] could not start capture: {e}");
                            self.set_tray_tooltip(
                                "Holler — microphone unavailable (tray → Grant Microphone Access)",
                            );
                        }
                    }
                }
                // else: OS key auto-repeat while held — debounced (ignored).
            }
            HotKeyState::Released => {
                if self.ptt_held {
                    self.ptt_held = false;
                    match self.capture.take() {
                        Some(capture) => match capture.stop() {
                            Ok(rec) => {
                                // Overlay visibility is owned by set_tray_state:
                                // it stays up through Processing and hides on Idle.
                                let rec = self.maybe_vad_trim(rec);
                                // Ignore accidental taps / silence: a clip too
                                // short to hold speech would only waste an API
                                // request and surface a confusing API error.
                                const MIN_SAMPLES: usize = 3_200; // 0.2 s @ 16 kHz
                                if rec.samples.len() < MIN_SAMPLES {
                                    println!(
                                        "[holler] clip too short ({} samples) — ignored",
                                        rec.samples.len()
                                    );
                                    self.set_tray_state(TrayState::Idle);
                                } else {
                                    self.set_tray_state(TrayState::Processing);
                                    self.transcribe(rec);
                                }
                            }
                            Err(e) => {
                                eprintln!("[holler] capture failed: {e}");
                                self.set_tray_state(TrayState::Idle);
                            }
                        },
                        None => println!("[holler] PTT UP"),
                    }
                }
            }
        }
    }

    /// Optionally trim leading/trailing silence via WebRTC VAD when `config.vad`
    /// is enabled. Logs the pre/post sample counts for diagnostics.
    fn maybe_vad_trim(&self, rec: Recording) -> Recording {
        if !self.config.vad {
            return rec;
        }
        let before = rec.samples.len();
        let samples = holler_audio::vad_trim(&rec.samples);
        let after = samples.len();
        if after < before {
            let trimmed_secs = (before - after) as f32 / 16_000.0;
            println!("[holler] VAD: trimmed {trimmed_secs:.2}s of silence");
        }
        let duration_secs = samples.len() as f32 / 16_000.0;
        Recording { samples, duration_secs }
    }

    /// Transcribe a finished recording on a worker thread (never block the
    /// event loop on a network call or a keychain prompt) and post the result
    /// back via the proxy. Provider/model come from config; resolution (which
    /// reads the stored key) happens off the main thread.
    fn transcribe(&self, rec: holler_audio::Recording) {
        println!(
            "[holler] PTT UP — captured {:.2}s, transcribing…",
            rec.duration_secs
        );

        let proxy = self.proxy.clone();
        let provider = self.config.stt_provider.clone();
        let model = self.config.model_override().map(str::to_string);

        std::thread::Builder::new()
            .name("holler-stt".into())
            .spawn(move || {
                let result = match build_provider(&provider, model) {
                    Some(stt) => stt
                        .transcribe(&rec.samples, 16_000)
                        .map(|text| Transcription {
                            text,
                            provider: stt.name().to_string(),
                        })
                        .map_err(|e| e.to_string()),
                    None => Err(format!(
                        "no API key for '{provider}' — run: holler set-key {provider} <KEY>"
                    )),
                };
                let _ = proxy.send_event(UserEvent::Transcript(result));
            })
            .expect("spawn transcription thread");
    }

    /// Deliver a transcript on the main thread: copy to clipboard ("copy
    /// memory"), record to history, then inject at the cursor.
    fn deliver(&mut self, t: Transcription) {
        // Empty/whitespace transcript (silence, no speech detected): don't
        // clobber the user's clipboard, write an empty history row, or inject
        // nothing — just go back to Idle.
        if t.text.trim().is_empty() {
            println!("[holler] empty transcript — ignored");
            self.set_tray_state(TrayState::Idle);
            return;
        }

        println!("[holler] transcript: {}", t.text);
        self.reset_tray_tooltip(); // a successful transcript clears any error hint

        // 1. Copy to the system clipboard (also primes the paste injection).
        if let Some(clipboard) = self.ensure_clipboard() {
            if let Err(e) = clipboard.set_text(t.text.clone()) {
                eprintln!("[holler] clipboard set failed: {e}");
            }
        }

        // 2. Record to searchable history.
        if let Some(history) = &self.history {
            if let Err(e) = history.record(&t.text, &t.provider) {
                eprintln!("[holler] history record failed: {e}");
                self.set_tray_tooltip("Holler — history not saved (see logs); text still delivered");
            }
        }

        // 3. Inject at the active cursor. Paste reads the clipboard we just set;
        //    give it a moment to propagate (clipboard set is racy).
        let mode = InjectMode::from_config(&self.config.injection_mode);

        // Re-check Accessibility in case the user granted it while running.
        self.accessibility_ok = permissions::accessibility_granted();

        // Whether auto-paste/auto-type couldn't run and the text is left on the
        // clipboard for a manual paste — drives the fallback toast below.
        let mut fell_back = false;
        if mode == InjectMode::Paste && !self.accessibility_ok {
            // Text is already on clipboard — silent fallback, no error spam.
            fell_back = true;
            if !self.accessibility_warned {
                self.accessibility_warned = true;
                println!("[holler] auto-paste skipped — Accessibility not granted. Text is on clipboard; use tray menu → Grant Accessibility Access…");
            }
        } else {
            if mode == InjectMode::Paste {
                // Let the clipboard propagate to the target app before firing
                // the paste chord. Windows delivers its clipboard-update
                // notification to the target later than macOS, so it needs more
                // headroom or the paste can land stale/empty.
                #[cfg(target_os = "windows")]
                const SETTLE_MS: u64 = 100;
                #[cfg(not(target_os = "windows"))]
                const SETTLE_MS: u64 = 60;
                std::thread::sleep(Duration::from_millis(SETTLE_MS));
            }
            match self.ensure_injector() {
                Some(injector) => {
                    if let Err(e) = injector.deliver(&t.text, mode) {
                        eprintln!("[holler] injection failed: {e} (text is on clipboard — paste manually)");
                        fell_back = true;
                    }
                }
                None => {
                    fell_back = true;
                    if !self.accessibility_warned {
                        self.accessibility_warned = true;
                        eprintln!("[holler] injector unavailable — text is on clipboard");
                    }
                }
            }
        }

        if fell_back {
            self.show_clipboard_toast();
        }
        self.set_tray_state(TrayState::Idle);
    }

    /// Show the "copied to clipboard — paste it" toast (when enabled in config)
    /// and arm its auto-dismiss timer. Called from `deliver` whenever auto-paste
    /// or auto-type couldn't run, so the user knows the transcript is waiting on
    /// the clipboard. The toast window is non-activating — it never steals the
    /// focus from the app the user is about to paste into.
    fn show_clipboard_toast(&mut self) {
        if !self.config.clipboard_toast {
            return;
        }
        if let Some(toast) = &mut self.toast {
            toast.show_message("COPIED TO CLIPBOARD — PASTE IT");
            self.toast_hide_at = Some(Instant::now() + Duration::from_secs(toast::VISIBLE_SECS));
        }
    }

    /// Switch the tray to a new state, resetting the animation and redrawing.
    fn set_tray_state(&mut self, state: TrayState) {
        self.tray_state = state;
        self.anim_frame = 0;
        self.render_tray();
        // One place owns overlay visibility (avoids show/hide scattered across
        // the PTT handlers): it's up — with the matching animation — while
        // Recording or Processing, and hidden when Idle. Render a first frame
        // before showing so it appears with content, not an empty flash.
        let level = self.capture.as_ref().map_or(0.0, |c| c.level());
        if let Some(ov) = &mut self.overlay {
            match state {
                TrayState::Recording => {
                    ov.render(overlay::Phase::Recording, 0, level);
                    ov.show();
                }
                TrayState::Processing => {
                    ov.render(overlay::Phase::Processing, 0, 0.0);
                    ov.show();
                }
                TrayState::Idle => ov.hide(),
            }
        }
    }

    /// Draw the current state/frame into the tray icon.
    fn render_tray(&self) {
        if let (Some(tray), Some(icon)) = (&self.tray, state_icon(self.tray_state, self.anim_frame))
        {
            let _ = tray.set_icon(Some(icon));
        }
    }

    /// Show a short message on the tray tooltip — the only feedback channel a
    /// tray agent has (stderr is invisible with no console). Errors are
    /// non-fatal: a failed tooltip update is ignored.
    fn set_tray_tooltip(&self, text: &str) {
        if let Some(tray) = &self.tray {
            let _ = tray.set_tooltip(Some(text));
        }
    }

    /// Restore the default "hold <combo> to talk" tooltip after an error hint.
    fn reset_tray_tooltip(&self) {
        self.set_tray_tooltip(&format!("Holler — hold {} to talk", self.ptt_label));
    }

    fn ensure_clipboard(&mut self) -> Option<&mut Clipboard> {
        if self.clipboard.is_none() {
            match Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => eprintln!("[holler] clipboard unavailable: {e}"),
            }
        }
        self.clipboard.as_mut()
    }

    fn ensure_injector(&mut self) -> Option<&mut Injector> {
        // Re-try if Accessibility was just granted (accessibility_ok flipped
        // to true in `deliver` before this call), otherwise skip to avoid
        // repeated error logging on every transcription.
        let should_try = self.injector.is_none() && self.accessibility_ok;
        if should_try {
            match Injector::new() {
                Ok(i) => {
                    self.injector = Some(i);
                    println!("[holler] injector ready (Accessibility granted)");
                }
                Err(e) => eprintln!("[holler] {e}"),
            }
        }
        self.injector.as_mut()
    }

    /// Re-query the live OS permission status (Accessibility + microphone) and,
    /// when something changed, update the tray labels in-place and refresh the
    /// open settings panel — so the user sees reality without a restart.
    fn refresh_permissions(&mut self) {
        self.last_ax_check = Instant::now();
        let mut changed = false;

        let granted = permissions::accessibility_granted();
        if granted != self.accessibility_ok {
            changed = true;
            self.accessibility_ok = granted;
            self.accessibility_warned = false; // allow fresh log on next delivery
            println!(
                "[holler] Accessibility {}",
                if granted { "granted — auto-paste active" } else { "revoked — clipboard fallback active" }
            );
        }

        let mic = permissions::microphone_status();
        if mic != self.last_mic_status {
            changed = true;
            self.last_mic_status = mic;
            println!("[holler] Microphone status: {mic:?}");
        }

        if changed {
            // Show/hide the Grant prompts to match the new state, and push the
            // fresh status into the panel if it's open and showing it.
            self.rebuild_grant_block();
            if let Some(sw) = &mut self.settings {
                sw.refresh_permissions();
            }
        }
    }

    /// Reconcile the tray's "Grant …" prompts with the current permission state:
    /// each entry is present at the top of the menu only while its permission is
    /// missing, with a trailing separator off the rest of the menu. Idempotent —
    /// safe to call whether or not the items are currently in the menu.
    fn rebuild_grant_block(&self) {
        let Some(menu) = &self.tray_menu else { return };

        // Detach any existing block elements first (remove() errs harmlessly if
        // the item isn't currently a child — we ignore that).
        if let Some(item) = &self.grant_access_menu_item {
            let _ = menu.remove(item);
        }
        if let Some(item) = &self.grant_mic_menu_item {
            let _ = menu.remove(item);
        }
        if let Some(sep) = &self.grant_block_sep {
            let _ = menu.remove(sep);
        }

        // Re-insert at the very top, in order, only the prompts that apply.
        let mut pos = 0;
        if !self.accessibility_ok {
            if let Some(item) = &self.grant_access_menu_item {
                let _ = menu.insert(item, pos);
                pos += 1;
            }
        }
        if !matches!(self.last_mic_status, permissions::MicStatus::Granted) {
            if let Some(item) = &self.grant_mic_menu_item {
                let _ = menu.insert(item, pos);
                pos += 1;
            }
        }
        if pos > 0 {
            if let Some(sep) = &self.grant_block_sep {
                let _ = menu.insert(sep, pos);
            }
        }
    }

    /// Apply the edits the settings UI confirmed this frame, then report each
    /// outcome back to the panel that requested it.
    fn handle_settings_actions(&mut self, actions: Vec<SettingsAction>) {
        for action in actions {
            match action {
                SettingsAction::SaveGeneral {
                    injection_mode,
                    vad,
                    clipboard_toast,
                } => {
                    self.config.injection_mode = injection_mode;
                    self.config.vad = vad;
                    self.config.clipboard_toast = clipboard_toast;
                    let res = holler_config::save(&self.config).map_err(|e| e.to_string());
                    match &res {
                        Ok(()) => println!("[holler] config saved (general)"),
                        Err(e) => eprintln!("[holler] config save failed: {e}"),
                    }
                    if let Some(sw) = &mut self.settings {
                        sw.general_feedback(res);
                    }
                }
                SettingsAction::ApplyPttKey(raw) => {
                    let res = self.apply_ptt_key(&raw);
                    if let Some(sw) = &mut self.settings {
                        sw.hotkey_feedback(res);
                    }
                }
                SettingsAction::SaveProviders { provider, model } => {
                    self.config.stt_provider = provider;
                    self.config.stt_model = model;
                    let res = holler_config::save(&self.config).map_err(|e| e.to_string());
                    match &res {
                        Ok(()) => println!(
                            "[holler] config saved — provider {} ({})",
                            self.config.stt_provider,
                            self.config.model_override().unwrap_or("default model")
                        ),
                        Err(e) => eprintln!("[holler] config save failed: {e}"),
                    }
                    if let Some(sw) = &mut self.settings {
                        sw.provider_feedback(res);
                    }
                }
                SettingsAction::SetKey { provider, key } => {
                    let res =
                        holler_config::store_secret(&provider, &key).map_err(|e| e.to_string());
                    match &res {
                        Ok(()) => println!("[holler] stored {provider} API key"),
                        Err(e) => eprintln!("[holler] storing {provider} key failed: {e}"),
                    }
                    if let Some(sw) = &mut self.settings {
                        sw.key_feedback(&provider, res);
                    }
                }
                SettingsAction::ClearKey { provider } => {
                    let res = holler_config::remove_secret(&provider).map_err(|e| e.to_string());
                    match &res {
                        Ok(()) => println!("[holler] cleared {provider} API key"),
                        Err(e) => eprintln!("[holler] clearing {provider} key failed: {e}"),
                    }
                    if let Some(sw) = &mut self.settings {
                        sw.key_feedback(&provider, res);
                    }
                }
                SettingsAction::OpenAccessibilitySettings => {
                    permissions::open_accessibility_settings();
                    println!("[holler] opened Accessibility settings — the panel updates automatically once granted.");
                }
                SettingsAction::OpenMicrophoneSettings => {
                    permissions::open_mic_settings();
                    println!("[holler] opened Microphone settings — the panel updates automatically once changed.");
                }
                SettingsAction::LoadHistory { query } => {
                    let res = self.load_history(&query);
                    if let Some(sw) = &mut self.settings {
                        sw.history_loaded(res);
                    }
                }
                SettingsAction::CopyHistory { text } => {
                    let res = match self.ensure_clipboard() {
                        Some(cb) => cb
                            .set_text(text)
                            .map(|()| "Copied to clipboard ✓".to_string())
                            .map_err(|e| e.to_string()),
                        None => Err("clipboard unavailable".to_string()),
                    };
                    if let Some(sw) = &mut self.settings {
                        sw.history_action_feedback(res);
                    }
                }
                SettingsAction::DeleteHistory { id, query } => {
                    // Delete, then reload the list so the panel shows the new
                    // truth (and the deleted row is gone) in one round-trip.
                    let deleted = match &self.history {
                        Some(h) => h.delete(id).map_err(|e| e.to_string()),
                        None => Err("history database unavailable".to_string()),
                    };
                    match deleted {
                        Ok(()) => println!("[holler] deleted history entry {id}"),
                        Err(ref e) => eprintln!("[holler] delete history failed: {e}"),
                    }
                    let reload = self.load_history(&query);
                    if let Some(sw) = &mut self.settings {
                        sw.history_loaded(reload);
                        sw.history_action_feedback(
                            deleted.map(|()| "Deleted ✓".to_string()),
                        );
                    }
                }
                SettingsAction::LoadStats => {
                    let res = self.load_stats();
                    if let Some(sw) = &mut self.settings {
                        sw.stats_loaded(res);
                    }
                }
                SettingsAction::SpeakHistory { text } => {
                    let res = self.speak(&text);
                    if let Err(e) = &res {
                        eprintln!("[holler] read-aloud failed: {e}");
                    }
                    if let Some(sw) = &mut self.settings {
                        sw.history_action_feedback(res.map(|()| "Reading aloud… ✓".to_string()));
                    }
                }
                SettingsAction::StopSpeaking => {
                    self.stop_speaking();
                    if let Some(sw) = &mut self.settings {
                        sw.history_action_feedback(Ok("Stopped".to_string()));
                    }
                }
            }
        }
    }

    /// Speak `text` via the OS TTS engine, cancelling any in-progress utterance
    /// (`interrupt = true`). The engine is built on first use — nothing touches
    /// the launch path — and speaks asynchronously on its own thread, so this
    /// never blocks the UI or the PTT/tray loop. Cross-platform via the `tts`
    /// crate (AVFoundation on macOS, SAPI/WinRT on Windows) — see DISCOVERIES.
    fn speak(&mut self, text: &str) -> Result<(), String> {
        if self.tts.is_none() {
            self.tts = Some(tts::Tts::default().map_err(|e| e.to_string())?);
        }
        let engine = self.tts.as_mut().expect("tts just initialised");
        engine.speak(text, true).map(|_| ()).map_err(|e| e.to_string())
    }

    /// Halt any in-progress read-aloud. No-op if nothing is speaking / the
    /// engine was never built.
    fn stop_speaking(&mut self) {
        if let Some(engine) = &mut self.tts {
            if let Err(e) = engine.stop() {
                eprintln!("[holler] stop read-aloud failed: {e}");
            }
        }
    }

    /// Compute usage statistics for the Settings → Stats panel. Like the
    /// history queries, this runs on the main thread — a single scan of a
    /// local SQLite table is sub-millisecond.
    fn load_stats(&self) -> Result<holler_store::Stats, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        match &self.history {
            Some(h) => h.stats(now).map_err(|e| e.to_string()),
            None => Err("history database unavailable".to_string()),
        }
    }

    /// Query the transcript history filtered by `query` (empty = all), newest
    /// first, for the Settings → History panel. Runs on the main thread — a
    /// local SQLite `LIKE` scan is sub-millisecond, like the config saves here.
    fn load_history(&self, query: &str) -> Result<Vec<holler_store::Entry>, String> {
        match &self.history {
            Some(h) => h.search(query).map_err(|e| e.to_string()),
            None => Err("history database unavailable".to_string()),
        }
    }

    /// Re-register the global PTT hotkey to `raw` (live, no restart) and
    /// persist it. New combo registers BEFORE the old one is dropped, so a
    /// conflict (combo owned by another app) leaves the current key working.
    fn apply_ptt_key(&mut self, raw: &str) -> Result<String, String> {
        if self.ptt_held {
            return Err("release the push-to-talk key first".to_string());
        }
        let manager = self
            .hotkeys
            .as_ref()
            .ok_or("global hotkeys are unavailable on this system")?;
        let (new_hk, label) = holler_config::try_parse_ptt_key(raw)?;

        if Some(new_hk.id()) != self.ptt_hotkey_id {
            manager.register(new_hk).map_err(|e| {
                format!("could not register {label} — is it taken by another app? ({e})")
            })?;
            if self.ptt_hotkey_id.is_some() {
                // Drop the previous combo (re-derived from config — the same
                // parse the original registration used).
                let (old_hk, _) = holler_config::parse_ptt_key(&self.config.ptt_key);
                let _ = manager.unregister(old_hk);
            }
            self.ptt_hotkey_id = Some(new_hk.id());
        }

        self.ptt_label = label.clone();
        self.config.ptt_key = raw.to_string();
        holler_config::save(&self.config)
            .map_err(|e| format!("hotkey is active, but saving config failed: {e}"))?;
        self.reset_tray_tooltip();
        println!("[holler] PTT key changed — hold {label} to talk");
        Ok(label)
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        self.init(event_loop);
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // Advance the tray + overlay animation when our frame timer fires.
        // (A settings-repaint wake can land between animation frames and
        // advance one a few ms early — cosmetically negligible.)
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && self.tray_state != TrayState::Idle
        {
            self.anim_frame = (self.anim_frame + 1) % icons::FRAMES;
            self.render_tray();
            // Drive the overlay animation while it's visible: a live mic level
            // when recording, an indeterminate sweep while transcribing.
            let phase = match self.tray_state {
                TrayState::Recording => Some(overlay::Phase::Recording),
                TrayState::Processing => Some(overlay::Phase::Processing),
                TrayState::Idle => None,
            };
            if let Some(phase) = phase {
                let level = self.capture.as_ref().map_or(0.0, |c| c.level());
                if let Some(ov) = &mut self.overlay {
                    ov.render(phase, self.anim_frame, level);
                }
            }
        }
        // A deferred egui repaint (cursor blink etc.) has come due.
        if let (Some(at), Some(sw)) = (self.settings_repaint_at, &self.settings) {
            if Instant::now() >= at {
                self.settings_repaint_at = None;
                sw.request_redraw();
            }
        }
        // The clipboard toast's dwell time has elapsed — dismiss it.
        if let Some(at) = self.toast_hide_at {
            if Instant::now() >= at {
                self.toast_hide_at = None;
                if let Some(toast) = &self.toast {
                    toast.hide();
                }
            }
        }
        // Re-check permissions on every loop wake, rate-limited. Guarded by
        // hotkeys.is_some() so we don't poll before init() has run.
        if self.hotkeys.is_some()
            && Instant::now().duration_since(self.last_ax_check) >= AX_POLL_INTERVAL
        {
            self.refresh_permissions();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Wake at the earliest pending deadline; sleep fully when none exists.
        let now = Instant::now();
        let mut wake: Option<Instant> = None;
        if self.tray_state != TrayState::Idle {
            // Animating — wake every frame.
            wake = Some(now + FRAME_INTERVAL);
        }
        if let Some(at) = self.settings_repaint_at {
            // egui asked for a deferred repaint of the settings window.
            wake = Some(wake.map_or(at, |w| w.min(at)));
        }
        if let Some(at) = self.toast_hide_at {
            // Wake to auto-dismiss the clipboard toast.
            wake = Some(wake.map_or(at, |w| w.min(at)));
        }
        if cfg!(target_os = "macos") && self.hotkeys.is_some() {
            // macOS only: poll slowly so the tray (and open Permissions panel)
            // reflect an Accessibility or microphone grant/revoke without a
            // restart. Other OSes have no such per-app prompt, so with nothing
            // else pending they reach the true no-poll idle below.
            let ax = now + AX_POLL_INTERVAL;
            wake = Some(wake.map_or(ax, |w| w.min(ax)));
        }
        event_loop.set_control_flow(match wake {
            Some(at) => ControlFlow::WaitUntil(at),
            // Fully idle — sleep until an event arrives (no CPU burn).
            None => ControlFlow::Wait,
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey(e) => self.on_hotkey(e),
            UserEvent::Menu(e) => {
                if self.quit_item_id.as_ref() == Some(&e.id) {
                    println!("[holler] quit requested — exiting.");
                    event_loop.exit();
                } else if self.settings_item_id.as_ref() == Some(&e.id) {
                    match &self.settings {
                        // Already open — bring it to the front instead.
                        Some(sw) => sw.focus(),
                        None => {
                            self.settings = SettingsWindow::create(
                                event_loop,
                                self.proxy.clone(),
                                &self.config,
                                &self.ptt_label,
                            );
                            if self.settings.is_none() {
                                eprintln!("[holler] settings window unavailable (see logs)");
                                self.set_tray_tooltip("Holler — settings window failed to open");
                            }
                        }
                    }
                } else if self.config_item_id.as_ref() == Some(&e.id) {
                    open_in_os(holler_config::config_path().ok().as_deref());
                } else if self.history_item_id.as_ref() == Some(&e.id) {
                    let folder = holler_store::default_db_path()
                        .ok()
                        .and_then(|p| p.parent().map(Path::to_path_buf));
                    open_in_os(folder.as_deref());
                } else if self.grant_access_item_id.as_ref() == Some(&e.id) {
                    permissions::open_accessibility_settings();
                    println!("[holler] opened Accessibility settings — tray updates automatically once granted.");
                } else if self.grant_mic_item_id.as_ref() == Some(&e.id) {
                    permissions::open_mic_settings();
                }
            }
            // Tray events reach the same loop; the icon's behaviour (overlay,
            // state) is wired up later.
            UserEvent::Tray(e) => println!("[holler] tray event: {e:?}"),
            UserEvent::Transcript(Ok(t)) => self.deliver(t),
            UserEvent::Transcript(Err(e)) => {
                eprintln!("[holler] transcription failed: {e}");
                // Surface it where a tray agent can actually see it.
                self.set_tray_tooltip(&format!("Holler — {e}"));
                self.set_tray_state(TrayState::Idle);
            }
            UserEvent::SettingsRepaint(delay) => {
                let Some(sw) = &self.settings else { return };
                if delay.is_zero() {
                    sw.request_redraw();
                } else if let Some(at) = Instant::now().checked_add(delay) {
                    // Keep the earliest pending deadline.
                    self.settings_repaint_at =
                        Some(self.settings_repaint_at.map_or(at, |cur| cur.min(at)));
                }
                // A delay too large for Instant means "no repaint needed".
            }
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // Route settings-window events to egui; everything else comes from the
        // overlay, whose events are ignored (it is controlled by PTT state,
        // not by the user closing it).
        let Some(sw) = &mut self.settings else { return };
        if sw.window_id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                // Drop the whole window + GL context + egui state — the
                // settings GUI is only resident while open (PLAN.md §6).
                self.settings = None;
                self.settings_repaint_at = None;
                println!("[holler] settings window closed");
            }
            WindowEvent::RedrawRequested => {
                let actions = sw.redraw();
                self.handle_settings_actions(actions);
            }
            event => {
                if let WindowEvent::Resized(size) = &event {
                    sw.resized(*size);
                }
                // Everything else (incl. Resized) feeds egui's input state.
                if sw.on_window_event(&event) {
                    sw.request_redraw();
                }
            }
        }
    }
}

/// Build the tray `Icon` for a state + animation frame (see `icons.rs`).
/// Returns `None` on the (currently unreachable) chance the RGBA buffer is the
/// wrong length — render_tray then skips the frame rather than aborting the
/// process from a timer callback (release builds use panic="abort").
fn state_icon(state: TrayState, frame: usize) -> Option<Icon> {
    let rgba = match state {
        TrayState::Idle => icons::idle(),
        TrayState::Recording => icons::recording(frame),
        TrayState::Processing => icons::processing(frame),
    };
    Icon::from_rgba(rgba, icons::SIZE, icons::SIZE)
        .map_err(|e| eprintln!("[holler] tray icon build failed: {e}"))
        .ok()
}

/// Open a file/folder in the OS default handler (Finder/Explorer/xdg-open).
fn open_in_os(path: Option<&Path>) {
    let Some(path) = path else {
        eprintln!("[holler] could not resolve path to open");
        return;
    };
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        eprintln!("[holler] could not open {}: {e}", path.display());
    }
}

fn main() {
    // `holler set-key <provider> <KEY>` stores an API key in `secrets.toml`
    // and exits — no event loop. (A stopgap until the Phase-2 settings UI.)
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("set-key") {
        run_set_key(&args);
        return;
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("build winit event loop");

    let mut app = App::new(event_loop.create_proxy());

    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("[holler] fatal: event loop error: {err}");
        std::process::exit(1);
    }
}

/// Build the configured STT provider (reading its key from `secrets.toml` or
/// the env var). `model` overrides the provider default when `Some`. Returns
/// `None` if the provider is unknown or has no stored key.
fn build_provider(provider: &str, model: Option<String>) -> Option<Arc<dyn SttProvider>> {
    match provider {
        "deepgram" => {
            let m = model.unwrap_or_else(|| DeepgramStt::DEFAULT_MODEL.to_string());
            DeepgramStt::from_stored_key(m)
                .ok()
                .map(|p| Arc::new(p) as Arc<dyn SttProvider>)
        }
        "openai" => {
            let m = model.unwrap_or_else(|| OpenAiStt::DEFAULT_MODEL.to_string());
            OpenAiStt::from_stored_key(m)
                .ok()
                .map(|p| Arc::new(p) as Arc<dyn SttProvider>)
        }
        other => {
            eprintln!("[holler] unknown stt_provider {other:?} in config; supported: deepgram, openai");
            None
        }
    }
}

/// Handle `holler set-key <provider> <KEY>` for the supported cloud providers.
fn run_set_key(args: &[String]) {
    let (Some(provider), Some(key)) = (args.get(2), args.get(3)) else {
        eprintln!("usage: holler set-key <openai|deepgram> <API_KEY>");
        std::process::exit(2);
    };
    let supported = [OpenAiStt::KEY_ACCOUNT, DeepgramStt::KEY_ACCOUNT];
    if !supported.contains(&provider.as_str()) {
        eprintln!("unknown provider {provider:?}; supported: {}", supported.join(", "));
        std::process::exit(2);
    }
    match holler_stt::store_key(provider, key) {
        Ok(()) => match holler_config::secrets_path() {
            Ok(p) => println!("[holler] stored {provider} API key in {}.", p.display()),
            Err(_) => println!("[holler] stored {provider} API key."),
        },
        Err(e) => {
            eprintln!("[holler] failed to store key: {e}");
            std::process::exit(1);
        }
    }
}
