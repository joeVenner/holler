//! Interactive read-aloud status popup — a borderless, always-on-top softbuffer
//! pill at the bottom-centre of the primary monitor, sitting above the recording
//! overlay's slot. It shows the live read-aloud phase (Triggered → Generating →
//! Speaking → Done/Stopped/Error) with an animated status dot, plus two
//! **clickable** controls: ⟲ Replay (re-read the last utterance) and ◼ Stop.
//!
//! Unlike the recording overlay and the clipboard toast — which are purely
//! ornamental and never routed input — this window's pointer events ARE routed
//! (see `main.rs::window_event`): hover lightens a button, a left click hit-tests
//! the two control circles and returns a [`PopupAction`]. Rendering stays
//! softbuffer (CPU pixels, no GL context) like its siblings, reusing the shared
//! pill paint helpers and bitmap font.

use std::num::NonZeroU32;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use winit::{
    dpi::{LogicalSize, PhysicalPosition},
    event_loop::ActiveEventLoop,
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};

use crate::font;
use crate::overlay::{blend, pack, sd_round_rect, Rgb};

pub const WIDTH: u32 = 380;
pub const HEIGHT: u32 = 66;
/// Seconds the popup lingers after a terminal phase before auto-dismissing.
pub const DONE_DWELL_SECS: u64 = 2;
pub const ERROR_DWELL_SECS: u64 = 4;

// Palette — shares the overlay's dark-pill look.
const BG: Rgb = (18, 18, 20);
const PILL: Rgb = (34, 34, 39);
const RING: Rgb = (66, 66, 74);
const TEXT: Rgb = (224, 224, 230);
const AMBER: Rgb = (255, 179, 64);
const BLUE: Rgb = (120, 180, 255);
const GREEN: Rgb = (120, 210, 140);
const GREY: Rgb = (170, 170, 178);
const RED: Rgb = (255, 99, 90);
const BTN_BG: Rgb = (52, 52, 60);
const BTN_BG_HOVER: Rgb = (78, 78, 92);
const BTN_BG_OFF: Rgb = (28, 28, 32);
const ICON_OFF: Rgb = (96, 96, 104);

// Geometry (logical px, origin top-left).
const CY: f32 = HEIGHT as f32 / 2.0;
const DOT_CX: f32 = 26.0;
const TEXT_X: i32 = 46;
const BTN_R: f32 = 15.0;
const STOP_CX: f32 = WIDTH as f32 - 28.0;
const REPLAY_CX: f32 = STOP_CX - 40.0;

/// The read-aloud phase the popup depicts. Mirrors the subset of
/// `speech::SpeechStatus` that has a visual; the app maps between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Triggered,
    Generating,
    Speaking,
    Finished,
    Stopped,
    Error,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Triggered => "Starting…",
            Phase::Generating => "Generating…",
            Phase::Speaking => "Speaking",
            Phase::Finished => "Done",
            Phase::Stopped => "Stopped",
            Phase::Error => "Error",
        }
    }

    fn accent(self) -> Rgb {
        match self {
            Phase::Triggered => AMBER,
            Phase::Generating => AMBER,
            Phase::Speaking => BLUE,
            Phase::Finished => GREEN,
            Phase::Stopped => GREY,
            Phase::Error => RED,
        }
    }

    /// Active phases pulse the status dot; terminal ones hold steady.
    fn animates(self) -> bool {
        matches!(self, Phase::Generating | Phase::Speaking)
    }
}

/// Which control the user clicked, returned by [`StatusPopup::on_click`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupAction {
    Replay,
    Stop,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Button {
    Replay,
    Stop,
}

/// Owns the popup window, its softbuffer surface, and the current visual state.
pub struct StatusPopup {
    window: Arc<Window>,
    _ctx: Context<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    /// Window scale factor, to map physical pointer coords to the logical layout.
    scale: f64,
    phase: Phase,
    frame: usize,
    /// Pointer position in logical px (popup-local), or off-window.
    cursor: Option<(f32, f32)>,
    hover: Option<Button>,
    /// Replay is offered only once something has been read this session; Stop
    /// only while a read-aloud is actually in flight.
    can_replay: bool,
    can_stop: bool,
    visible: bool,
}

impl StatusPopup {
    /// Build the popup window (hidden). `None` on failure — a missing popup must
    /// never take down the tray/PTT loop.
    pub fn create(event_loop: &ActiveEventLoop) -> Option<Self> {
        let monitor = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())?;
        let monitor_size = monitor.size();
        let scale = monitor.scale_factor();
        let origin = monitor.position();
        let win_w = (WIDTH as f64 * scale) as i32;
        let win_h = (HEIGHT as f64 * scale) as i32;
        // Sit just above the recording overlay's bottom slot.
        let margin = (124.0 * scale) as i32;
        let x = origin.x + (monitor_size.width as i32 - win_w) / 2;
        let y = origin.y + monitor_size.height as i32 - win_h - margin;

        let attrs = WindowAttributes::default()
            .with_title("Holler Read-Aloud")
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_position(PhysicalPosition::new(x, y))
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false);

        #[cfg(target_os = "windows")]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_skip_taskbar(true)
        };

        let window = Arc::new(event_loop.create_window(attrs).ok()?);
        let ctx = Context::new(window.clone()).ok()?;
        let surface = Surface::new(&ctx, window.clone()).ok()?;

        Some(Self {
            window,
            _ctx: ctx,
            surface,
            scale,
            phase: Phase::Triggered,
            frame: 0,
            cursor: None,
            hover: None,
            can_replay: false,
            can_stop: false,
            visible: false,
        })
    }

    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    /// True while the popup is visible AND its phase animates — the caller wakes
    /// the loop each frame only then (idle popups cost nothing).
    pub fn is_animating(&self) -> bool {
        self.visible && self.phase.animates()
    }

    /// Set the phase + control availability and (re)reveal the popup, redrawing.
    pub fn show(&mut self, phase: Phase, can_replay: bool, can_stop: bool) {
        let fresh = !self.visible;
        self.phase = phase;
        self.can_replay = can_replay;
        self.can_stop = can_stop;
        if fresh {
            self.frame = 0;
        }
        // A control may have vanished from under the pointer.
        self.recompute_hover();
        self.render();
        if fresh {
            self.window.set_visible(true);
            self.visible = true;
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.cursor = None;
        self.hover = None;
        self.window.set_visible(false);
    }

    /// Advance one animation frame; redraw if the phase animates. No-op (returns
    /// false) when hidden or steady, so the caller can stop waking the loop.
    pub fn tick(&mut self) -> bool {
        if !self.is_animating() {
            return false;
        }
        self.frame = (self.frame + 1) % crate::icons::FRAMES;
        self.render();
        true
    }

    /// Update hover from a pointer move (physical coords). Returns true if the
    /// hovered control changed (the caller need not care — we redraw internally).
    pub fn on_cursor_moved(&mut self, physical_x: f64, physical_y: f64) -> bool {
        let x = (physical_x / self.scale) as f32;
        let y = (physical_y / self.scale) as f32;
        self.cursor = Some((x, y));
        let before = self.hover;
        self.recompute_hover();
        if self.hover != before {
            self.render();
            true
        } else {
            false
        }
    }

    /// The pointer left the window — clear hover.
    pub fn on_cursor_left(&mut self) {
        if self.cursor.is_some() || self.hover.is_some() {
            self.cursor = None;
            self.hover = None;
            self.render();
        }
    }

    /// Hit-test a left click against the enabled controls.
    pub fn on_click(&self) -> Option<PopupAction> {
        match self.button_at_cursor() {
            Some(Button::Replay) if self.can_replay => Some(PopupAction::Replay),
            Some(Button::Stop) if self.can_stop => Some(PopupAction::Stop),
            _ => None,
        }
    }

    fn recompute_hover(&mut self) {
        self.hover = self.button_at_cursor().filter(|b| match b {
            Button::Replay => self.can_replay,
            Button::Stop => self.can_stop,
        });
    }

    /// Which button circle the pointer is within (ignoring enabled-state), if any.
    fn button_at_cursor(&self) -> Option<Button> {
        let (x, y) = self.cursor?;
        let hit = |cx: f32| {
            let dx = x - cx;
            let dy = y - CY;
            (dx * dx + dy * dy).sqrt() <= BTN_R + 3.0
        };
        if hit(STOP_CX) {
            Some(Button::Stop)
        } else if hit(REPLAY_CX) {
            Some(Button::Replay)
        } else {
            None
        }
    }

    fn render(&mut self) {
        if self
            .surface
            .resize(
                NonZeroU32::new(WIDTH).unwrap(),
                NonZeroU32::new(HEIGHT).unwrap(),
            )
            .is_err()
        {
            return;
        }
        let Ok(mut buf) = self.surface.buffer_mut() else {
            return;
        };
        paint(
            &mut buf,
            self.phase,
            self.frame,
            self.hover,
            self.can_replay,
            self.can_stop,
        );
        buf.present().ok();
    }
}

/// Paint a whole frame: pill, status dot + label, and the two control buttons.
fn paint(
    buf: &mut [u32],
    phase: Phase,
    frame: usize,
    hover: Option<Button>,
    can_replay: bool,
    can_stop: bool,
) {
    let w = WIDTH as i32;
    let h = HEIGHT as i32;
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let half_w = w as f32 / 2.0 - 1.5;
    let half_h = h as f32 / 2.0 - 1.5;
    let radius = half_h;

    // Backdrop + rounded card with an AA outline (signed-distance coverage).
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            buf[idx] = pack(BG);
            let sd = sd_round_rect(x as f32 + 0.5 - cx, y as f32 + 0.5 - cy, half_w, half_h, radius);
            let fill = (0.5 - sd).clamp(0.0, 1.0);
            if fill > 0.0 {
                blend(buf, idx, PILL, fill);
            }
            let ring = (1.0 - (sd + 1.2).abs()).clamp(0.0, 1.0);
            if ring > 0.0 {
                blend(buf, idx, RING, ring * 0.9);
            }
        }
    }

    // Status dot — pulses while the phase animates, steady otherwise.
    let accent = phase.accent();
    let dot = if phase.animates() {
        let p = pulse(frame);
        lerp_rgb(dim(accent), accent, p)
    } else {
        accent
    };
    draw_dot(buf, DOT_CX, CY, 6.0, dot);

    // Status label, vertically centred.
    let y0 = (h - font::text_height()) / 2;
    font::draw_text(buf, w, h, TEXT_X, y0, phase.label(), TEXT);

    // Controls.
    draw_replay_button(buf, REPLAY_CX, CY, hover == Some(Button::Replay), can_replay);
    draw_stop_button(buf, STOP_CX, CY, hover == Some(Button::Stop), can_stop);
}

/// A circular control background; colour reflects hover/enabled state.
fn draw_button_bg(buf: &mut [u32], cx: f32, hover: bool, enabled: bool) {
    let bg = if !enabled {
        BTN_BG_OFF
    } else if hover {
        BTN_BG_HOVER
    } else {
        BTN_BG
    };
    draw_dot(buf, cx, CY, BTN_R, bg);
}

/// ◼ Stop: a filled rounded square centred in the button.
fn draw_stop_button(buf: &mut [u32], cx: f32, cy: f32, hover: bool, enabled: bool) {
    draw_button_bg(buf, cx, hover, enabled);
    let col = if enabled { RED } else { ICON_OFF };
    let s = 5.0; // half-side
    fill_round_rect(buf, cx - s, cy - s, cx + s, cy + s, 1.5, col);
}

/// ⟲ Replay: a ~300° ring with an arrowhead at its tip — a circular arrow.
fn draw_replay_button(buf: &mut [u32], cx: f32, cy: f32, hover: bool, enabled: bool) {
    draw_button_bg(buf, cx, hover, enabled);
    let col = if enabled { BLUE } else { ICON_OFF };
    let r = 6.0;
    // Open ring: sweep from ~35° to ~320° (gap at the top-right where the head sits).
    draw_arc(buf, cx, cy, r, 2.0, 35.0_f32.to_radians(), 320.0_f32.to_radians(), col);
    // Arrowhead at the start of the sweep (~35°), pointing along the tangent
    // (counter-clockwise) so it reads as "go around again".
    let a = 35.0_f32.to_radians();
    let tip = (cx + r * a.cos(), cy - r * a.sin());
    // Tangent direction (clockwise screen-space) and its normal, for a small head.
    let tang = (a.sin(), a.cos());
    let norm = (a.cos(), -a.sin());
    let hl = 4.0; // head length
    let hw = 3.0; // head half-width
    let base = (tip.0 - tang.0 * hl, tip.1 - tang.1 * hl);
    let p1 = (base.0 + norm.0 * hw, base.1 + norm.1 * hw);
    let p2 = (base.0 - norm.0 * hw, base.1 - norm.1 * hw);
    fill_triangle(buf, tip, p1, p2, col);
}

// ---- low-level paint primitives (bounds-checked to the popup buffer) ----------

/// Anti-aliased filled circle via signed-distance coverage.
fn draw_dot(buf: &mut [u32], cx: f32, cy: f32, r: f32, col: Rgb) {
    let x0 = (cx - r - 1.0).floor() as i32;
    let x1 = (cx + r + 1.0).ceil() as i32;
    let y0 = (cy - r - 1.0).floor() as i32;
    let y1 = (cy + r + 1.0).ceil() as i32;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let cov = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_xy(buf, x, y, col, cov);
            }
        }
    }
}

/// Filled rounded rectangle (axis-aligned) via the shared rounded-box SDF.
fn fill_round_rect(buf: &mut [u32], x0: f32, y0: f32, x1: f32, y1: f32, r: f32, col: Rgb) {
    let cx = (x0 + x1) / 2.0;
    let cy = (y0 + y1) / 2.0;
    let hw = (x1 - x0) / 2.0;
    let hh = (y1 - y0) / 2.0;
    let ix0 = (x0 - 1.0).floor() as i32;
    let ix1 = (x1 + 1.0).ceil() as i32;
    let iy0 = (y0 - 1.0).floor() as i32;
    let iy1 = (y1 + 1.0).ceil() as i32;
    for y in iy0..iy1 {
        for x in ix0..ix1 {
            let sd = sd_round_rect(x as f32 + 0.5 - cx, y as f32 + 0.5 - cy, hw, hh, r);
            let cov = (0.5 - sd).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_xy(buf, x, y, col, cov);
            }
        }
    }
}

/// Anti-aliased circular arc (a radial band over an angular range). Angles are in
/// radians, measured counter-clockwise from the +x axis in screen space (y down
/// is handled by the callers' sign choices).
fn draw_arc(buf: &mut [u32], cx: f32, cy: f32, r: f32, thickness: f32, start: f32, end: f32, col: Rgb) {
    let outer = r + thickness;
    let x0 = (cx - outer - 1.0).floor() as i32;
    let x1 = (cx + outer + 1.0).ceil() as i32;
    let y0 = (cy - outer - 1.0).floor() as i32;
    let y1 = (cy + outer + 1.0).ceil() as i32;
    let half = thickness / 2.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = cy - (y as f32 + 0.5); // flip so +angle is up
            let dist = (dx * dx + dy * dy).sqrt();
            // Coverage of the radial band centred on r.
            let band = (half + 0.5 - (dist - r).abs()).clamp(0.0, 1.0);
            if band <= 0.0 {
                continue;
            }
            let mut ang = dy.atan2(dx);
            if ang < 0.0 {
                ang += std::f32::consts::TAU;
            }
            if ang >= start && ang <= end {
                blend_xy(buf, x, y, col, band);
            }
        }
    }
}

/// Filled triangle via half-plane sign tests, with 1-px edge anti-aliasing.
fn fill_triangle(buf: &mut [u32], a: (f32, f32), b: (f32, f32), c: (f32, f32), col: Rgb) {
    let min_x = a.0.min(b.0).min(c.0).floor() as i32 - 1;
    let max_x = a.0.max(b.0).max(c.0).ceil() as i32 + 1;
    let min_y = a.1.min(b.1).min(c.1).floor() as i32 - 1;
    let max_y = a.1.max(b.1).max(c.1).ceil() as i32 + 1;
    let edge = |p: (f32, f32), q: (f32, f32), x: f32, y: f32| {
        (x - p.0) * (q.1 - p.1) - (y - p.1) * (q.0 - p.0)
    };
    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = edge(a, b, px, py);
            let w1 = edge(b, c, px, py);
            let w2 = edge(c, a, px, py);
            let inside = (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0)
                || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
            if inside {
                blend_xy(buf, x, y, col, 1.0);
            }
        }
    }
}

/// `blend` addressed by pixel coordinate, bounds-checked to the popup buffer.
fn blend_xy(buf: &mut [u32], x: i32, y: i32, col: Rgb, a: f32) {
    if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
        return;
    }
    blend(buf, (y * WIDTH as i32 + x) as usize, col, a);
}

/// Sine pulse in `[0, 1]`; one cycle per `icons::FRAMES` frames.
fn pulse(frame: usize) -> f32 {
    let t = frame as f32 / crate::icons::FRAMES as f32;
    ((t * std::f32::consts::TAU).sin() + 1.0) / 2.0
}

/// A dimmed version of a colour (for the low end of the pulse).
fn dim((r, g, b): Rgb) -> Rgb {
    ((r as f32 * 0.45) as u8, (g as f32 * 0.45) as u8, (b as f32 * 0.45) as u8)
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_for_speaking_fits_left_of_the_controls() {
        // The widest active label must not run under the replay button.
        let max_right = (REPLAY_CX - BTN_R - 6.0) as i32;
        for p in [Phase::Generating, Phase::Speaking, Phase::Triggered] {
            assert!(
                TEXT_X + font::text_width(p.label()) <= max_right,
                "label {:?} overflows into the controls",
                p
            );
        }
    }

    #[test]
    fn only_active_phases_animate() {
        assert!(Phase::Generating.animates());
        assert!(Phase::Speaking.animates());
        assert!(!Phase::Triggered.animates());
        assert!(!Phase::Finished.animates());
        assert!(!Phase::Stopped.animates());
        assert!(!Phase::Error.animates());
    }
}
