//! Holler — Phase 0 spike.
//!
//! Proves the one hard integration risk (CLAUDE.md / docs/PLAN.md §0 & §34):
//! a SINGLE main-thread `winit` event loop that owns BOTH the `tray-icon`
//! and the `global-hotkey` push-to-talk receiver, with reliable key
//! down/up edge detection and OS auto-repeat debounced — on macOS and Windows.
//!
//! Exit criteria (PLAN.md §5, Phase 0):
//!   hold the PTT key  -> "PTT DOWN" logged exactly once
//!   release it        -> "PTT UP"   logged exactly once
//!   tray menu "Quit"  -> process exits cleanly.

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use holler_audio::AudioCapture;
use mimalloc::MiMalloc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::WindowId,
};

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
    /// True while the PTT key is physically held. The edge detector that makes
    /// this the source of truth is what debounces OS key auto-repeat.
    ptt_held: bool,
    /// The live mic capture, present only between PTT down and up. cpal's
    /// `Stream` is `!Send`, so this (and `App`) stay on the main thread.
    capture: Option<AudioCapture>,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            proxy,
            hotkeys: None,
            tray: None,
            ptt_hotkey_id: 0,
            quit_item_id: None,
            ptt_held: false,
            capture: None,
        }
    }

    /// Build everything that must live on the main thread once the loop runs.
    /// Idempotent: `resumed` can fire more than once on some platforms.
    fn init(&mut self) {
        if self.hotkeys.is_some() {
            return;
        }

        // --- Tray icon + menu ---
        let menu = Menu::new();
        let quit_item = MenuItem::new("Quit Holler", true, None);
        menu.append(&quit_item).expect("append Quit menu item");
        self.quit_item_id = Some(quit_item.id().clone());

        let tray = TrayIconBuilder::new()
            .with_tooltip(format!("Holler — hold {PTT_LABEL} to talk"))
            .with_icon(placeholder_icon())
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
                            // Phase 1 stops here; STT consumes `rec.samples` next.
                            Ok(rec) => println!(
                                "[holler] PTT UP — captured {:.2}s, {} samples @ 16kHz mono",
                                rec.duration_secs,
                                rec.samples.len()
                            ),
                            Err(e) => eprintln!("[holler] capture failed: {e}"),
                        },
                        None => println!("[holler] PTT UP"),
                    }
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Sleep until something happens — no busy polling.
        event_loop.set_control_flow(ControlFlow::Wait);
        self.init();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey(e) => self.on_hotkey(e),
            UserEvent::Menu(e) => {
                if self.quit_item_id.as_ref() == Some(&e.id) {
                    println!("[holler] quit requested — exiting.");
                    event_loop.exit();
                }
            }
            // Phase 0 just confirms tray events reach the same loop; the
            // icon's behaviour (overlay, state) is wired up later.
            UserEvent::Tray(e) => println!("[holler] tray event: {e:?}"),
        }
    }

    // Holler is windowless (tray only), so this never fires — but the trait
    // requires it.
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

/// A flat accent-blue square so the tray entry is visible on every platform
/// without committing a binary asset. Real artwork lands with the GUI (Phase 2).
fn placeholder_icon() -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..(SIZE * SIZE) {
        rgba.extend_from_slice(&[0x4C, 0x9A, 0xFF, 0xFF]);
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("valid RGBA icon")
}

fn main() {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("build winit event loop");

    let mut app = App::new(event_loop.create_proxy());

    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("[holler] fatal: event loop error: {err}");
        std::process::exit(1);
    }
}
