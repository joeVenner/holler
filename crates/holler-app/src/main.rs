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

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    ptt_hotkey_id: u32,
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
    /// Retained menu items so we can update their text/enabled state at runtime.
    ax_menu_item: Option<MenuItem>,
    grant_access_menu_item: Option<MenuItem>,
    /// Timestamp of the last AXIsProcessTrusted() check (rate-limits polling).
    last_ax_check: Instant,
    /// Desktop recording indicator shown during PTT hold.
    overlay: Option<Overlay>,
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

        Self {
            proxy,
            hotkeys: None,
            tray: None,
            ptt_hotkey_id: 0,
            quit_item_id: None,
            config_item_id: None,
            history_item_id: None,
            grant_access_item_id: None,
            grant_mic_item_id: None,
            ax_menu_item: None,
            grant_access_menu_item: None,
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
        let menu = Menu::new();

        // Permissions section.
        let mic_label = "✓  Microphone";
        let mic_item = MenuItem::new(mic_label, false, None); // always shown; disabled label
        let ax_label = if self.accessibility_ok {
            "✓  Accessibility (auto-paste active)".to_string()
        } else {
            "✗  Accessibility (auto-paste disabled)".to_string()
        };
        let ax_item = MenuItem::new(ax_label, false, None);
        let grant_access_item = MenuItem::new("   Grant Accessibility Access…", !self.accessibility_ok, None);
        let grant_mic_item = MenuItem::new("   Grant Microphone Access…", true, None);
        menu.append(&mic_item).expect("append mic item");
        menu.append(&ax_item).expect("append ax item");
        menu.append(&grant_access_item).expect("append grant-access item");
        menu.append(&grant_mic_item).expect("append grant-mic item");
        self.grant_access_item_id = Some(grant_access_item.id().clone());
        self.grant_mic_item_id = Some(grant_mic_item.id().clone());
        // Retain items so refresh_ax_status() can update their labels live.
        self.ax_menu_item = Some(ax_item);
        self.grant_access_menu_item = Some(grant_access_item);

        menu.append(&PredefinedMenuItem::separator()).expect("separator");

        // Settings section.
        let config_item = MenuItem::new("Edit Settings (config.toml)…", true, None);
        let history_item = MenuItem::new("Open History Folder…", true, None);
        menu.append(&config_item).expect("append config item");
        menu.append(&history_item).expect("append history item");
        menu.append(&PredefinedMenuItem::separator()).expect("separator");

        let quit_item = MenuItem::new("Quit Holler", true, None);
        menu.append(&quit_item).expect("append Quit menu item");
        self.config_item_id = Some(config_item.id().clone());
        self.history_item_id = Some(history_item.id().clone());
        self.quit_item_id = Some(quit_item.id().clone());

        // Parse the PTT combo from config; falls back to Ctrl+Alt+Space on error.
        let (ptt_hotkey, ptt_label) = holler_config::parse_ptt_key(&self.config.ptt_key);

        let tray = TrayIconBuilder::new()
            .with_tooltip(format!("Holler — hold {ptt_label} to talk"))
            .with_icon(state_icon(TrayState::Idle, 0))
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
        // tray app at launch with no visible message. Degrade gracefully:
        // leave hotkeys = None, keep the tray/menu alive, and tell the user how
        // to recover (change ptt_key via the tray's Edit Settings, then relaunch).
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[holler] could not initialise global hotkeys ({e}); push-to-talk disabled.");
                return;
            }
        };
        self.ptt_hotkey_id = ptt_hotkey.id();
        if let Err(e) = manager.register(ptt_hotkey) {
            eprintln!(
                "[holler] could not register PTT key {ptt_label} — it may already be in use by \
                 another app. Change ptt_key in config.toml and relaunch ({e})."
            );
            return;
        }
        self.hotkeys = Some(manager);

        // `global-hotkey` has no callback API — only a static channel. Drain it
        // on a dedicated thread that BLOCKS on recv() and forwards via the
        // proxy, so the main loop can stay in ControlFlow::Wait (event-driven,
        // no polling — PLAN.md §6) yet still wake instantly on a key event.
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

        // Desktop recording indicator — created lazily here so we have an event loop.
        self.overlay = Overlay::create(event_loop);
        if self.overlay.is_none() {
            eprintln!("[holler] overlay window unavailable (non-fatal)");
        }

        println!("[holler] ready — hold {ptt_label} to talk; tray menu → Quit to exit.");
    }

    fn on_hotkey(&mut self, event: GlobalHotKeyEvent) {
        if event.id != self.ptt_hotkey_id {
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
                            self.set_tray_state(TrayState::Recording);
                            // Pre-render frame 0 before showing so the window
                            // has content the moment it becomes visible.
                            if let Some(ov) = &mut self.overlay {
                                ov.render(0);
                                ov.show();
                            }
                            println!("[holler] PTT DOWN — recording…");
                        }
                        Err(e) => eprintln!("[holler] could not start capture: {e}"),
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
                                self.set_tray_state(TrayState::Processing);
                                if let Some(ov) = &self.overlay { ov.hide(); }
                                let rec = self.maybe_vad_trim(rec);
                                self.transcribe(rec);
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
        println!("[holler] transcript: {}", t.text);

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
            }
        }

        // 3. Inject at the active cursor. Paste reads the clipboard we just set;
        //    give it a moment to propagate (clipboard set is racy).
        let mode = InjectMode::from_config(&self.config.injection_mode);

        // Re-check Accessibility in case the user granted it while running.
        self.accessibility_ok = permissions::accessibility_granted();

        if mode == InjectMode::Paste && !self.accessibility_ok {
            // Text is already on clipboard — silent fallback, no error spam.
            if !self.accessibility_warned {
                self.accessibility_warned = true;
                println!("[holler] auto-paste skipped — Accessibility not granted. Text is on clipboard; use tray menu → Grant Accessibility Access…");
            }
        } else {
            if mode == InjectMode::Paste {
                std::thread::sleep(Duration::from_millis(60));
            }
            match self.ensure_injector() {
                Some(injector) => {
                    if let Err(e) = injector.deliver(&t.text, mode) {
                        eprintln!("[holler] injection failed: {e} (text is on clipboard — paste manually)");
                    }
                }
                None => {
                    if !self.accessibility_warned {
                        self.accessibility_warned = true;
                        eprintln!("[holler] injector unavailable — text is on clipboard");
                    }
                }
            }
        }

        self.set_tray_state(TrayState::Idle);
    }

    /// Switch the tray to a new state, resetting the animation and redrawing.
    fn set_tray_state(&mut self, state: TrayState) {
        self.tray_state = state;
        self.anim_frame = 0;
        self.render_tray();
    }

    /// Draw the current state/frame into the tray icon.
    fn render_tray(&self) {
        if let Some(tray) = &self.tray {
            let _ = tray.set_icon(Some(state_icon(self.tray_state, self.anim_frame)));
        }
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

    /// Re-query AXIsProcessTrusted() and update the tray menu labels in-place
    /// so the user sees the real status without restarting the app.
    fn refresh_ax_status(&mut self) {
        self.last_ax_check = Instant::now();
        let granted = permissions::accessibility_granted();
        if granted == self.accessibility_ok {
            return;
        }
        self.accessibility_ok = granted;
        self.accessibility_warned = false; // allow fresh log on next delivery
        if let Some(item) = &self.ax_menu_item {
            if granted {
                item.set_text("✓  Accessibility (auto-paste active)");
            } else {
                item.set_text("✗  Accessibility (auto-paste disabled)");
            }
        }
        if let Some(item) = &self.grant_access_menu_item {
            item.set_enabled(!granted);
        }
        println!(
            "[holler] Accessibility {}",
            if granted { "granted — auto-paste active" } else { "revoked — clipboard fallback active" }
        );
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        self.init(event_loop);
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // Advance the tray + overlay animation when our frame timer fires.
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && self.tray_state != TrayState::Idle
        {
            self.anim_frame = (self.anim_frame + 1) % icons::FRAMES;
            self.render_tray();
            if self.tray_state == TrayState::Recording {
                if let Some(ov) = &mut self.overlay {
                    ov.render(self.anim_frame);
                }
            }
        }
        // Re-check Accessibility on every loop wake, rate-limited. Guarded by
        // hotkeys.is_some() so we don't poll before init() has run.
        if self.hotkeys.is_some()
            && Instant::now().duration_since(self.last_ax_check) >= AX_POLL_INTERVAL
        {
            self.refresh_ax_status();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray_state != TrayState::Idle {
            // Animating — wake every frame.
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_INTERVAL));
        } else if !self.accessibility_ok {
            // Waiting for the user to grant Accessibility — wake periodically
            // so the tray label updates the moment permission is granted.
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + AX_POLL_INTERVAL));
        } else {
            // Fully idle — sleep until an event arrives (no CPU burn).
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey(e) => self.on_hotkey(e),
            UserEvent::Menu(e) => {
                if self.quit_item_id.as_ref() == Some(&e.id) {
                    println!("[holler] quit requested — exiting.");
                    event_loop.exit();
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
                self.set_tray_state(TrayState::Idle);
            }
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // Only the overlay window generates window events; close requests are ignored
        // (the overlay is controlled by PTT state, not the user closing it).
        let _ = (id, event);
    }
}

/// Build the tray `Icon` for a state + animation frame (see `icons.rs`).
fn state_icon(state: TrayState, frame: usize) -> Icon {
    let rgba = match state {
        TrayState::Idle => icons::idle(),
        TrayState::Recording => icons::recording(frame),
        TrayState::Processing => icons::processing(frame),
    };
    Icon::from_rgba(rgba, icons::SIZE, icons::SIZE).expect("valid RGBA icon")
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
