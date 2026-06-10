//! On-demand egui settings window, rendered inside the single main-thread
//! winit loop with a manual `egui-winit` + `egui_glow` integration — never
//! `eframe::run_native` (PLAN.md §34). The window (and its GL context, egui
//! state and fonts) exists only while open and is dropped on close, so the
//! idle tray process carries no GUI memory (PLAN.md §6).
//!
//! Renderer choice — `egui_glow` over `egui-wgpu` / softbuffer: see
//! docs/DISCOVERIES.md (2026-06-10). In short: glow is eframe's own default
//! renderer, uses the system OpenGL driver (WGL on Windows, CGL on macOS —
//! deprecated but shipping), and costs a fraction of wgpu's dependency tree,
//! compile time and resident memory. The integration below mirrors the
//! crate's own `examples/pure_glow.rs`.

use std::ffi::CString;
use std::num::NonZeroU32;
use std::sync::Arc;

use egui_glow::glow;
use egui_glow::EguiGlow;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::prelude::{GlSurface, NotCurrentGlContext, PossiblyCurrentGlContext};
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{ApiPreference, DisplayBuilder};
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Window, WindowAttributes, WindowId};

use crate::UserEvent;

const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(760.0, 520.0);
const MIN_SIZE: LogicalSize<f64> = LogicalSize::new(640.0, 420.0);

/// The settings sections, in sidebar order. Routing only for now — each
/// placeholder panel is replaced by the real thing in P2–P6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    General,
    Hotkey,
    Providers,
    Permissions,
    History,
    Stats,
    About,
}

impl Panel {
    const ALL: [Self; 7] = [
        Self::General,
        Self::Hotkey,
        Self::Providers,
        Self::Permissions,
        Self::History,
        Self::Stats,
        Self::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Hotkey => "Hotkey",
            Self::Providers => "Providers",
            Self::Permissions => "Permissions",
            Self::History => "History",
            Self::Stats => "Stats",
            Self::About => "About",
        }
    }

    /// One-liner shown under the placeholder heading.
    fn blurb(self) -> &'static str {
        match self {
            Self::General => "Injection mode, VAD and other behaviour.",
            Self::Hotkey => "The push-to-talk combo.",
            Self::Providers => "Speech-to-text providers and API keys.",
            Self::Permissions => "Microphone and Accessibility status.",
            Self::History => "Your transcript history.",
            Self::Stats => "Local usage statistics.",
            Self::About => "About Holler.",
        }
    }
}

/// Pure UI state — kept apart from the GL/egui plumbing so `redraw` can
/// borrow it and `EguiGlow` mutably at the same time.
struct UiState {
    selected: Panel,
}

impl UiState {
    fn draw(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("settings-nav")
            .resizable(false)
            .exact_size(160.0)
            .show_inside(ui, |ui| {
                ui.add_space(8.0);
                for panel in Panel::ALL {
                    if ui
                        .selectable_label(self.selected == panel, panel.label())
                        .clicked()
                    {
                        self.selected = panel;
                    }
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| match self.selected {
            Panel::About => draw_about(ui),
            panel => draw_placeholder(ui, panel),
        });
    }
}

/// The settings window plus everything it needs to paint itself. Dropping
/// this frees the egui state, the GL context and the window in one go.
pub struct SettingsWindow {
    window: Window,
    gl_context: PossiblyCurrentContext,
    gl_surface: Surface<WindowSurface>,
    gl: Arc<glow::Context>,
    egui_glow: EguiGlow,
    ui: UiState,
    /// False until the first frame has been painted — the window is created
    /// hidden and revealed only once it has content (avoids a white flash).
    shown: bool,
}

impl SettingsWindow {
    /// Build the window + GL context + egui state and paint the first frame.
    /// Returns `None` (with a logged reason) on any failure — a broken
    /// settings window must never take down the tray/PTT loop.
    pub fn create(
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Option<Self> {
        let attrs = WindowAttributes::default()
            .with_title("Holler Settings")
            .with_inner_size(INITIAL_SIZE)
            .with_min_inner_size(MIN_SIZE)
            .with_visible(false);
        // Centre on the primary monitor (same desktop-space maths as the
        // overlay — monitor origins are global, not (0,0); DISCOVERIES).
        let attrs = match event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
        {
            Some(monitor) => {
                let size = monitor.size();
                let origin = monitor.position();
                let scale = monitor.scale_factor();
                let win_w = (INITIAL_SIZE.width * scale) as i32;
                let win_h = (INITIAL_SIZE.height * scale) as i32;
                let x = origin.x + (size.width as i32 - win_w) / 2;
                let y = origin.y + (size.height as i32 - win_h) / 2;
                attrs.with_position(PhysicalPosition::new(x, y))
            }
            None => attrs, // OS default placement
        };

        // Let glutin-winit pair a GL config with the winit window. Native API
        // first (WGL / CGL), EGL as the fallback (egui#2520).
        let template = ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);
        let (window, gl_config) = DisplayBuilder::new()
            .with_preference(ApiPreference::FallbackEgl)
            .with_window_attributes(Some(attrs.clone()))
            .build(event_loop, template, |mut configs| {
                configs.next().expect("glutin offered no GL config")
            })
            .map_err(|e| eprintln!("[holler] settings: no GL display/config ({e})"))
            .ok()?;
        let gl_display = gl_config.display();

        // The window is usually created by the DisplayBuilder; finalize covers
        // the platforms where config selection has to happen first.
        let window = match window {
            Some(w) => w,
            None => glutin_winit::finalize_window(event_loop, attrs, &gl_config)
                .map_err(|e| eprintln!("[holler] settings: window creation failed ({e})"))
                .ok()?,
        };
        let raw_handle = window.window_handle().ok().map(|h| h.as_raw());

        // Core GL first, GLES as the fallback (older Windows drivers / ANGLE).
        let context_attrs = ContextAttributesBuilder::new().build(raw_handle);
        let fallback_attrs = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(raw_handle);
        // SAFETY: the raw window handle comes from the live `window` above and
        // outlives the display/context built from it.
        let not_current = unsafe {
            gl_display
                .create_context(&gl_config, &context_attrs)
                .or_else(|_| gl_display.create_context(&gl_config, &fallback_attrs))
                .map_err(|e| eprintln!("[holler] settings: GL context failed ({e})"))
                .ok()?
        };

        let Some(raw_handle) = raw_handle else {
            eprintln!("[holler] settings: window has no raw handle");
            return None;
        };
        let (w, h): (u32, u32) = window.inner_size().into();
        let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            raw_handle,
            NonZeroU32::new(w).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(h).unwrap_or(NonZeroU32::MIN),
        );
        // SAFETY: same handle/lifetime argument as above.
        let gl_surface = unsafe {
            gl_display
                .create_window_surface(&gl_config, &surface_attrs)
                .map_err(|e| eprintln!("[holler] settings: GL surface failed ({e})"))
                .ok()?
        };
        let gl_context = not_current
            .make_current(&gl_surface)
            .map_err(|e| eprintln!("[holler] settings: make_current failed ({e})"))
            .ok()?;
        // Vsync — caps repaints at the refresh rate, no busy redraw loop.
        let _ = gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::MIN));

        // SAFETY: the loader function queries the current GL display, which
        // stays alive as long as `gl_display` (owned via context/surface).
        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function(|s| {
                let Ok(s) = CString::new(s) else {
                    return std::ptr::null();
                };
                gl_display.get_proc_address(&s)
            })
        });

        let egui_glow = EguiGlow::new(
            event_loop,
            Arc::clone(&gl),
            None,
            Some(window.scale_factor() as f32),
            true,
        );

        // egui's repaint requests (cursor blink, animations) become loop
        // wake-ups via the proxy — the same funnel every other subsystem uses.
        egui_glow.egui_ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.send_event(UserEvent::SettingsRepaint(info.delay));
        });

        // App theme: dark, matching the tray/overlay look, independent of the
        // OS theme so the window is consistent across platforms.
        egui_glow.egui_ctx.set_theme(egui::Theme::Dark);

        let mut this = Self {
            window,
            gl_context,
            gl_surface,
            gl,
            egui_glow,
            ui: UiState {
                selected: Panel::General,
            },
            shown: false,
        };
        // Paint before showing so the window appears with content, and don't
        // rely on a RedrawRequested ever being delivered to a hidden window.
        this.redraw();
        Some(this)
    }

    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Bring an already-open window to the front (tray item clicked again).
    /// On macOS this also activates our LSUIElement agent so the window can
    /// actually take key focus.
    pub fn focus(&self) {
        self.window.focus_window();
    }

    /// Feed a winit event to egui. Returns true when egui wants a repaint.
    pub fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        self.egui_glow.on_window_event(&self.window, event).repaint
    }

    /// Keep the GL surface in sync with the window size (zero is clamped —
    /// minimised windows must not poison the surface).
    pub fn resized(&mut self, size: PhysicalSize<u32>) {
        self.gl_surface.resize(
            &self.gl_context,
            NonZeroU32::new(size.width).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(size.height).unwrap_or(NonZeroU32::MIN),
        );
    }

    /// Run the egui frame and paint it.
    pub fn redraw(&mut self) {
        let ui_state = &mut self.ui; // disjoint borrow next to egui_glow
        self.egui_glow
            .run(&self.window, |ui| ui_state.draw(ui));

        use glow::HasContext as _;
        // SAFETY: plain state-set + clear on our own current context.
        unsafe {
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.egui_glow.paint(&self.window);
        if self.gl_surface.swap_buffers(&self.gl_context).is_err() {
            return; // context lost — the user can close/reopen the window.
        }

        if !self.shown {
            self.shown = true;
            self.window.set_visible(true);
            self.focus();
        }
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        // Free the GPU-side egui resources while the context is still alive
        // and current; context, surface and window are dropped right after.
        if self.gl_context.is_current() {
            self.egui_glow.destroy();
        }
    }
}

/// Placeholder panel body — replaced section by section in P2–P6.
fn draw_placeholder(ui: &mut egui::Ui, panel: Panel) {
    ui.heading(panel.label());
    ui.add_space(4.0);
    ui.label(panel.blurb());
    ui.add_space(12.0);
    ui.weak("Coming soon.");
}

/// About — already real: name, version, licence. Cheap and final.
fn draw_about(ui: &mut egui::Ui) {
    ui.heading("Holler");
    ui.add_space(4.0);
    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
    ui.add_space(8.0);
    ui.label("Push-to-talk dictation — a walkie-talkie for your agents.");
    ui.label("Hold the hotkey, speak, release: the transcript lands at your cursor.");
    ui.add_space(12.0);
    ui.weak("© 2026 joeVenner — MIT OR Apache-2.0");
}
