//! Floating recording indicator — a borderless, always-on-top window shown at
//! the bottom-centre of the primary monitor while a dictation is in progress.
//!
//! Rendering is `softbuffer` (CPU pixels, no GPU context): the overlay is a
//! tiny, purely-ornamental, non-activating window, so a second GL/egui stack
//! would cost idle GPU memory and main-thread contention for no benefit — see
//! docs/DISCOVERIES.md (2026-06-12). The look is a modern dark pill with an
//! anti-aliased outline (signed-distance coverage), a pulsing record dot, and a
//! live, scrolling level meter fed by the real microphone amplitude
//! (`AudioCapture::level`). During transcription the meter is replaced by an
//! indeterminate sweep so the two phases read differently at a glance.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use winit::{
    dpi::{LogicalSize, PhysicalPosition},
    event_loop::ActiveEventLoop,
    window::{Window, WindowAttributes, WindowLevel},
};

pub const WIDTH: u32 = 340;
pub const HEIGHT: u32 = 72;

/// Which phase of a dictation the overlay is depicting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Mic is open; show the live level meter.
    Recording,
    /// Audio captured, transcription in flight; show an indeterminate sweep.
    Processing,
}

/// An RGB colour, kept as components so we can alpha-composite with coverage.
/// Shared with the toast renderer (`toast.rs`), which reuses the paint helpers.
pub(crate) type Rgb = (u8, u8, u8);

// Palette tuned to macOS dark-mode materials + system colours, shared in spirit
// with the toast and status popup so the three overlays read as one surface.
const BG: Rgb = (18, 18, 20); // window backdrop / pill corners (near-black)
const PILL: Rgb = (44, 44, 46); // the rounded card — macOS dark popover material
const RING: Rgb = (72, 72, 76); // its subtle outline
const REC: Rgb = (255, 69, 58); // systemRed, recording dot
const REC_DIM: Rgb = (140, 38, 32);
const WAVE_HI: Rgb = (64, 156, 255); // loud sample — toward systemBlue
const WAVE_LO: Rgb = (74, 96, 128); // quiet sample
const PROC: Rgb = (255, 159, 10); // systemOrange dot while transcribing
const PROC_SWEEP: Rgb = (150, 188, 235);

/// Geometry shared by the dot and meter (logical px, origin top-left).
const DOT_CX: f32 = 30.0;
const METER_X0: f32 = 56.0;
const METER_X1: f32 = WIDTH as f32 - 18.0;
const BAR_W: f32 = 3.0;
const BAR_GAP: f32 = 2.0;
/// One history sample per visible bar; older samples scroll off the left.
const NUM_BARS: usize = ((METER_X1 - METER_X0) / (BAR_W + BAR_GAP)) as usize;

/// Owns the overlay window, its softbuffer surface, and the rolling level
/// history that drives the scrolling meter.
pub struct Overlay {
    window: Arc<Window>,
    _ctx: Context<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
    /// Recent levels in `[0, 1]`, newest at the back; one per rendered frame.
    levels: VecDeque<f32>,
    /// Exponentially-smoothed level so the meter glides instead of jittering.
    level_smooth: f32,
}

impl Overlay {
    /// Create the overlay window. Hidden by default — call `show()` to reveal it.
    pub fn create(event_loop: &ActiveEventLoop) -> Option<Self> {
        let monitor = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())?;

        // Anchor to the monitor's own desktop-space origin (not an assumed
        // (0,0)) so multi-monitor layouts — where the primary can sit at a
        // positive/negative offset — still place the pill on the right screen.
        // Work in physical pixels to match the scaled inner size.
        let monitor_size = monitor.size();
        let scale = monitor.scale_factor();
        let origin = monitor.position();
        let win_w = (WIDTH as f64 * scale) as i32;
        let win_h = (HEIGHT as f64 * scale) as i32;
        let margin = (40.0 * scale) as i32; // 40 px from the bottom edge

        // Bottom-centre of this monitor.
        let x = origin.x + (monitor_size.width as i32 - win_w) / 2;
        let y = origin.y + monitor_size.height as i32 - win_h - margin;

        let attrs = WindowAttributes::default()
            .with_title("Holler Overlay")
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_position(PhysicalPosition::new(x, y))
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false);

        // Windows-only parity with the macOS ornamental overlay: keep it out of
        // the taskbar and don't let it steal focus from the field about to be
        // pasted into. (macOS doesn't show a taskbar button or take key focus
        // for a borderless always-on-top window, so this is gated to Windows.)
        #[cfg(target_os = "windows")]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_skip_taskbar(true).with_active(false)
        };

        let window = Arc::new(event_loop.create_window(attrs).ok()?);
        let ctx = Context::new(window.clone()).ok()?;
        let surface = Surface::new(&ctx, window.clone()).ok()?;

        Some(Self {
            window,
            _ctx: ctx,
            surface,
            levels: VecDeque::with_capacity(NUM_BARS),
            level_smooth: 0.0,
        })
    }

    pub fn show(&self) {
        self.window.set_visible(true);
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    /// Render one frame for `phase`. `frame` drives the periodic animation
    /// (pulse / sweep); `level` is the live mic amplitude in `[0, 1]` (ignored
    /// when processing). Smoothing and the scrolling history live here so the
    /// caller just forwards the raw reading each tick.
    pub fn render(&mut self, phase: Phase, frame: usize, level: f32) {
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

        // Advance the smoothed level + history. `level_smooth`/`levels` and
        // `surface` (held by `buf`) are disjoint fields, so both borrows coexist.
        let target = if phase == Phase::Recording { level } else { 0.0 };
        self.level_smooth += (target - self.level_smooth) * 0.45;
        self.levels.push_back(self.level_smooth.clamp(0.0, 1.0));
        while self.levels.len() > NUM_BARS {
            self.levels.pop_front();
        }

        paint(&mut buf, phase, frame, &self.levels);
        buf.present().ok();
    }
}

/// Paint a whole frame into the softbuffer (XRGB, native-endian).
fn paint(buf: &mut [u32], phase: Phase, frame: usize, levels: &VecDeque<f32>) {
    let w = WIDTH as i32;
    let h = HEIGHT as i32;
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let half_w = w as f32 / 2.0 - 1.5;
    let half_h = h as f32 / 2.0 - 1.5;
    let radius = half_h; // full-height pill

    // Backdrop, then the rounded card with an AA outline, computed per pixel
    // from a signed distance field (24k px — trivial at the overlay's framerate).
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) as usize;
            buf[idx] = pack(BG);
            let sd = sd_round_rect(x as f32 + 0.5 - cx, y as f32 + 0.5 - cy, half_w, half_h, radius);
            // Fill: coverage = how far inside the edge this pixel sits.
            let fill = (0.5 - sd).clamp(0.0, 1.0);
            if fill > 0.0 {
                blend(buf, idx, PILL, fill);
            }
            // Outline: a ~1 px band hugging the edge from the inside.
            let ring = (1.0 - (sd + 1.2).abs()).clamp(0.0, 1.0);
            if ring > 0.0 {
                blend(buf, idx, RING, ring * 0.9);
            }
        }
    }

    match phase {
        Phase::Recording => {
            let pulse = pulse(frame);
            draw_dot(buf, DOT_CX, cy, 9.0, lerp_rgb(REC_DIM, REC, pulse));
            draw_meter(buf, cy, levels);
        }
        Phase::Processing => {
            // Dot holds steady amber; an indeterminate sweep replaces the meter.
            draw_dot(buf, DOT_CX, cy, 9.0, PROC);
            draw_sweep(buf, cy, frame);
        }
    }
}

/// Scrolling level meter: one mirrored vertical bar per history sample, newest
/// at the right so the trace flows leftward as time passes.
fn draw_meter(buf: &mut [u32], cy: f32, levels: &VecDeque<f32>) {
    let max_h = HEIGHT as f32 * 0.30; // half-height at full scale
    let n = levels.len();
    for (i, &lvl) in levels.iter().enumerate() {
        // Right-align: the last sample sits at the right edge of the meter.
        let x = METER_X1 - BAR_W - (n - 1 - i) as f32 * (BAR_W + BAR_GAP);
        if x < METER_X0 {
            continue;
        }
        let bar_h = (lvl * max_h).max(1.0); // a thin idle line when silent
        let col = lerp_rgb(WAVE_LO, WAVE_HI, lvl);
        fill_bar(buf, x, cy - bar_h, x + BAR_W, cy + bar_h, col);
    }
}

/// Indeterminate "working" animation: a soft Gaussian highlight that sweeps
/// back and forth across the meter region over flat baseline bars.
fn draw_sweep(buf: &mut [u32], cy: f32, frame: usize) {
    let span = METER_X1 - METER_X0 - BAR_W;
    // Triangle wave in [0,1] over one animation period (the caller's frame
    // counter wraps at `icons::FRAMES`) for a smooth there-and-back sweep.
    let period = crate::icons::FRAMES as f32;
    let t = (frame as f32 % period) / period;
    let tri = 1.0 - (2.0 * t - 1.0).abs();
    let head = METER_X0 + tri * span;

    let mut x = METER_X0;
    while x <= METER_X1 - BAR_W {
        // Distance of this bar from the sweep head → brightness falloff.
        let d = (x - head).abs() / 26.0;
        let glow = (-(d * d)).exp(); // Gaussian
        let bar_h = 2.0 + glow * (HEIGHT as f32 * 0.22);
        let col = lerp_rgb(WAVE_LO, PROC_SWEEP, glow);
        fill_bar(buf, x, cy - bar_h, x + BAR_W, cy + bar_h, col);
        x += BAR_W + BAR_GAP;
    }
}

/// A filled vertical bar with horizontal-edge anti-aliasing (fractional column
/// coverage); vertical extents are clamped into the pill interior.
fn fill_bar(buf: &mut [u32], x0: f32, y0: f32, x1: f32, y1: f32, col: Rgb) {
    let y_lo = y0.max(6.0).floor() as i32;
    let y_hi = y1.min(HEIGHT as f32 - 6.0).ceil() as i32;
    let xi0 = x0.floor() as i32;
    let xi1 = x1.ceil() as i32;
    for x in xi0..xi1 {
        // Coverage of this pixel column by [x0, x1].
        let cov = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
        if cov <= 0.0 {
            continue;
        }
        for y in y_lo..y_hi {
            blend_xy(buf, x, y, col, cov);
        }
    }
}

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

/// Signed distance from a point to a rounded rectangle centred at the origin
/// (negative inside). Standard rounded-box SDF.
pub(crate) fn sd_round_rect(px: f32, py: f32, half_w: f32, half_h: f32, r: f32) -> f32 {
    let qx = px.abs() - (half_w - r);
    let qy = py.abs() - (half_h - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Sine pulse in `[0, 1]`; one cycle per `icons::FRAMES` frames.
fn pulse(frame: usize) -> f32 {
    let t = frame as f32 / crate::icons::FRAMES as f32;
    ((t * std::f32::consts::TAU).sin() + 1.0) / 2.0
}

/// Pack an RGB triple into softbuffer's native XRGB word.
pub(crate) fn pack((r, g, b): Rgb) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Linear interpolate between two colours; `t` clamped to `[0, 1]`.
fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Alpha-composite `col` over the pixel at `idx` with coverage `a` in `[0, 1]`.
pub(crate) fn blend(buf: &mut [u32], idx: usize, col: Rgb, a: f32) {
    let a = a.clamp(0.0, 1.0);
    let dst = buf[idx];
    let dr = ((dst >> 16) & 0xFF) as f32;
    let dg = ((dst >> 8) & 0xFF) as f32;
    let db = (dst & 0xFF) as f32;
    let r = (col.0 as f32 * a + dr * (1.0 - a)).round() as u32;
    let g = (col.1 as f32 * a + dg * (1.0 - a)).round() as u32;
    let b = (col.2 as f32 * a + db * (1.0 - a)).round() as u32;
    buf[idx] = (r << 16) | (g << 8) | b;
}

/// `blend` addressed by pixel coordinate, with bounds checking.
fn blend_xy(buf: &mut [u32], x: i32, y: i32, col: Rgb, a: f32) {
    if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
        return;
    }
    blend(buf, (y * WIDTH as i32 + x) as usize, col, a);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rect_sdf_sign_matches_inside_outside() {
        // Centre is well inside (negative); a point far past the corner is
        // outside (positive); the mid-edge sits ~on the boundary.
        assert!(sd_round_rect(0.0, 0.0, 50.0, 20.0, 10.0) < 0.0);
        assert!(sd_round_rect(100.0, 100.0, 50.0, 20.0, 10.0) > 0.0);
        assert!(sd_round_rect(50.0, 0.0, 50.0, 20.0, 10.0).abs() < 0.001);
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let a = (0, 0, 0);
        let b = (100, 200, 50);
        assert_eq!(lerp_rgb(a, b, 0.0), a);
        assert_eq!(lerp_rgb(a, b, 1.0), b);
        assert_eq!(lerp_rgb(a, b, 0.5), (50, 100, 25));
        // t is clamped, not extrapolated.
        assert_eq!(lerp_rgb(a, b, 2.0), b);
    }

    #[test]
    fn full_coverage_blend_replaces_destination() {
        let mut buf = [pack(BG)];
        blend(&mut buf, 0, REC, 1.0);
        assert_eq!(buf[0], pack(REC));
        // Zero coverage leaves the destination untouched.
        blend(&mut buf, 0, WAVE_HI, 0.0);
        assert_eq!(buf[0], pack(REC));
    }

    #[test]
    fn meter_shows_at_least_one_bar_of_history() {
        // NUM_BARS is derived from the geometry; guard against a zero/oops.
        const { assert!(NUM_BARS > 10) };
    }
}
