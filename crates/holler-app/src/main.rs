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
//! CLI: `holler set-key openai <KEY>` stores an API key in the OS keychain.

mod icons;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use holler_audio::AudioCapture;
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

/// How often the tray animation advances a frame.
const FRAME_INTERVAL: Duration = Duration::from_millis(90);

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

/// PTT trigger. `Ctrl+Alt+Space` has no OS conflicts (unlike the function
/// keys, which macOS hijacks for media), works on every keyboard, and is
/// comfortable to hold. Becomes user-configurable in Phase 1 (DECISIONS #2).
const PTT_MODS: Modifiers = Modifiers::CONTROL.union(Modifiers::ALT);
const PTT_CODE: Code = Code::Space;
const PTT_LABEL: &str = "Ctrl+Alt+Space";

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
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        // NB: no keychain access here. Reading a key can trigger a (blocking)
        // OS keychain prompt; doing it on the launch path would freeze startup.
        // The provider is resolved lazily on the worker thread at PTT-release.
        // Config (filesystem) and history (SQLite) are prompt-free, so they're
        // fine to load eagerly.
        let config = holler_config::load_or_create().unwrap_or_else(|e| {
            eprintln!("[holler] config load failed ({e}); using defaults.");
            Config::default()
        });
        let history = History::open_default()
            .map_err(|e| eprintln!("[holler] history db unavailable ({e}); not recording."))
            .ok();

        Self {
            proxy,
            hotkeys: None,
            tray: None,
            ptt_hotkey_id: 0,
            quit_item_id: None,
            config_item_id: None,
            history_item_id: None,
            tray_state: TrayState::Idle,
            anim_frame: 0,
            ptt_held: false,
            capture: None,
            config,
            history,
            clipboard: None,
            injector: None,
        }
    }

    /// Build everything that must live on the main thread once the loop runs.
    /// Idempotent: `resumed` can fire more than once on some platforms.
    fn init(&mut self) {
        if self.hotkeys.is_some() {
            return;
        }

        // --- Tray icon + menu ---
        // A lightweight settings entry point until the Phase-2 egui window:
        // open the config file / history folder, plus Quit.
        let menu = Menu::new();
        let config_item = MenuItem::new("Edit Settings (config.toml)…", true, None);
        let history_item = MenuItem::new("Open History Folder…", true, None);
        let quit_item = MenuItem::new("Quit Holler", true, None);
        menu.append(&config_item).expect("append config item");
        menu.append(&history_item).expect("append history item");
        menu.append(&PredefinedMenuItem::separator())
            .expect("append separator");
        menu.append(&quit_item).expect("append Quit menu item");
        self.config_item_id = Some(config_item.id().clone());
        self.history_item_id = Some(history_item.id().clone());
        self.quit_item_id = Some(quit_item.id().clone());

        let tray = TrayIconBuilder::new()
            .with_tooltip(format!("Holler — hold {PTT_LABEL} to talk"))
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
        let manager = GlobalHotKeyManager::new().expect("create global-hotkey manager");
        let ptt = HotKey::new(Some(PTT_MODS), PTT_CODE);
        self.ptt_hotkey_id = ptt.id();
        manager.register(ptt).expect("register PTT hotkey");
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

        println!("[holler] ready — hold {PTT_LABEL} to talk; tray menu → Quit to exit.");
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

    /// Transcribe a finished recording on a worker thread (never block the
    /// event loop on a network call or a keychain prompt) and post the result
    /// back via the proxy. Provider/model come from config; resolution (which
    /// reads the keychain) happens off the main thread.
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
        if mode == InjectMode::Paste {
            std::thread::sleep(Duration::from_millis(60));
        }
        match self.ensure_injector() {
            Some(injector) => {
                if let Err(e) = injector.deliver(&t.text, mode) {
                    eprintln!("[holler] injection failed: {e} (text is on the clipboard — paste manually)");
                }
            }
            None => eprintln!("[holler] no injector; text is on the clipboard — paste manually"),
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
        if self.injector.is_none() {
            match Injector::new() {
                Ok(i) => self.injector = Some(i),
                Err(e) => eprintln!("[holler] {e}"),
            }
        }
        self.injector.as_mut()
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        self.init();
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // Advance the tray animation when our frame timer fires.
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && self.tray_state != TrayState::Idle
        {
            self.anim_frame = (self.anim_frame + 1) % icons::FRAMES;
            self.render_tray();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Schedule the next frame while animating; otherwise sleep until an
        // event wakes us (no polling — PLAN.md §6).
        if self.tray_state == TrayState::Idle {
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_INTERVAL));
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

    // Holler is windowless (tray only), so this never fires — but the trait
    // requires it.
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
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
    // `holler set-key <provider> <KEY>` stores an API key in the OS keychain
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

/// Build the configured STT provider (reading its key from the keychain).
/// `model` overrides the provider default when `Some`. Returns `None` if the
/// provider is unknown or has no stored key. Reads the keychain — call OFF the
/// main thread.
fn build_provider(provider: &str, model: Option<String>) -> Option<Arc<dyn SttProvider>> {
    match provider {
        "deepgram" => {
            let m = model.unwrap_or_else(|| DeepgramStt::DEFAULT_MODEL.to_string());
            DeepgramStt::from_keychain(m)
                .ok()
                .map(|p| Arc::new(p) as Arc<dyn SttProvider>)
        }
        "openai" => {
            let m = model.unwrap_or_else(|| OpenAiStt::DEFAULT_MODEL.to_string());
            OpenAiStt::from_keychain(m)
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
        Ok(()) => println!("[holler] stored {provider} API key in the OS keychain."),
        Err(e) => {
            eprintln!("[holler] failed to store key: {e}");
            std::process::exit(1);
        }
    }
}
