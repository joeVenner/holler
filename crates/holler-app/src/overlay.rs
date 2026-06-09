//! Floating recording indicator — a borderless, always-on-top window shown
//! at the bottom-centre of the primary monitor while PTT is held.
//!
//! Rendering is done with `softbuffer` (CPU pixels, no GPU required). The
//! animation matches the tray icon: a pulsing red dot on a dark pill.

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

/// BGRA colour helpers (softbuffer uses native-endian XRGB on all platforms).
const fn xrgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

const BG: u32 = xrgb(28, 28, 30);       // near-black charcoal
const REC_ON: u32 = xrgb(255, 59, 48);  // Apple red
const REC_DIM: u32 = xrgb(120, 20, 15); // dim red for pulse contrast
const TEXT_GREY: u32 = xrgb(180, 180, 185);

/// Owns the overlay window and its softbuffer surface.
pub struct Overlay {
    window: Arc<Window>,
    _ctx: Context<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
}

impl Overlay {
    /// Create the overlay window. Hidden by default — call `show()` to reveal it.
    pub fn create(event_loop: &ActiveEventLoop) -> Option<Self> {
        let monitor = event_loop.primary_monitor()
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

        Some(Self { window, _ctx: ctx, surface })
    }

    pub fn show(&self) {
        self.window.set_visible(true);
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    /// Render one animation frame. `frame` is 0-based, wrapping at `FRAMES`.
    pub fn render(&mut self, frame: usize) {
        let w = WIDTH as usize;
        let h = HEIGHT as usize;

        if self.surface
            .resize(NonZeroU32::new(WIDTH).unwrap(), NonZeroU32::new(HEIGHT).unwrap())
            .is_err()
        {
            return;
        }

        let Ok(mut buf) = self.surface.buffer_mut() else { return };

        // Fill background.
        buf.fill(BG);

        // Draw rounded-rect border: simple 6 px inset pill outline.
        let r = 14usize;
        draw_rounded_rect(&mut buf, w, h, r, xrgb(55, 55, 58));

        // Pulsing dot — 16 px radius, centred vertically, left-aligned.
        let pulse = pulse_alpha(frame);
        let dot_r = 10usize;
        let dot_cx = 32usize;
        let dot_cy = h / 2;
        let dot_col = blend(REC_ON, REC_DIM, pulse);
        draw_circle(&mut buf, w, dot_cx, dot_cy, dot_r, dot_col);

        // Draw three-bar "sound wave" decoration to the right of the dot.
        draw_bars(&mut buf, w, h, dot_cx + dot_r + 12, frame);

        // Horizontal text stand-in: a bright strip (real text needs a font lib).
        // We draw "REC" as simple pixel blocks — enough to be readable at a glance.
        draw_rec_label(&mut buf, w, h, dot_cx + dot_r + 48, TEXT_GREY);

        buf.present().ok();
    }
}

/// Sine-based pulse: returns a value in [0, 255] where FRAMES gives one cycle.
fn pulse_alpha(frame: usize) -> u8 {
    let t = frame as f32 / crate::icons::FRAMES as f32;
    let s = ((t * std::f32::consts::TAU).sin() + 1.0) / 2.0;
    (s * 255.0) as u8
}

/// Linear blend: 0 = a, 255 = b.
fn blend(a: u32, b: u32, t: u8) -> u32 {
    let t = t as u32;
    let a_r = (a >> 16) & 0xFF;
    let a_g = (a >> 8) & 0xFF;
    let a_b = a & 0xFF;
    let b_r = (b >> 16) & 0xFF;
    let b_g = (b >> 8) & 0xFF;
    let b_b = b & 0xFF;
    let r = (a_r * (255 - t) + b_r * t) / 255;
    let g = (a_g * (255 - t) + b_g * t) / 255;
    let b_ = (a_b * (255 - t) + b_b * t) / 255;
    (r << 16) | (g << 8) | b_
}

fn set_pixel(buf: &mut [u32], w: usize, x: usize, y: usize, col: u32) {
    if x < w && y < buf.len() / w {
        buf[y * w + x] = col;
    }
}

fn draw_circle(buf: &mut [u32], w: usize, cx: usize, cy: usize, r: usize, col: u32) {
    let r2 = (r * r) as i64;
    for dy in -(r as i64)..=(r as i64) {
        for dx in -(r as i64)..=(r as i64) {
            if dx * dx + dy * dy <= r2 {
                let x = cx as i64 + dx;
                let y = cy as i64 + dy;
                if x >= 0 && y >= 0 {
                    set_pixel(buf, w, x as usize, y as usize, col);
                }
            }
        }
    }
}

fn draw_rounded_rect(buf: &mut [u32], w: usize, h: usize, _r: usize, col: u32) {
    // Simple 1-px border for now (full rounded corners need more maths).
    for x in 0..w {
        set_pixel(buf, w, x, 0, col);
        set_pixel(buf, w, x, h - 1, col);
    }
    for y in 0..h {
        set_pixel(buf, w, 0, y, col);
        set_pixel(buf, w, w - 1, y, col);
    }
}

/// Three animated equaliser bars.
fn draw_bars(buf: &mut [u32], w: usize, h: usize, x0: usize, frame: usize) {
    let bar_w = 4usize;
    let gap = 3usize;
    let max_bar_h = (h as f32 * 0.55) as usize;
    let base_y = h / 2;
    let col = xrgb(100, 180, 255);

    for i in 0..3 {
        let phase = (frame + i * 4) % crate::icons::FRAMES;
        let t = phase as f32 / crate::icons::FRAMES as f32;
        let bar_h = (((t * std::f32::consts::TAU).sin() + 1.0) / 2.0 * max_bar_h as f32) as usize + 4;
        let x = x0 + i * (bar_w + gap);
        let y_top = base_y.saturating_sub(bar_h / 2);
        let y_bot = (base_y + bar_h / 2).min(h - 4);
        for bx in x..(x + bar_w).min(w) {
            for by in y_top..y_bot {
                set_pixel(buf, w, bx, by, col);
            }
        }
    }
}

/// Pixel-art "REC" label using 5×7 bitmap font (no dep needed).
fn draw_rec_label(buf: &mut [u32], w: usize, h: usize, x0: usize, col: u32) {
    // Bitmaps for R, E, C — each 5 columns × 7 rows, MSB = top.
    const R: [u8; 7] = [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001];
    const E: [u8; 7] = [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111];
    const C: [u8; 7] = [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110];

    let glyphs = [R, E, C];
    let scale = 2usize;
    let char_w = 5 * scale;
    let gap = scale;
    let char_h = 7 * scale;
    let y0 = h / 2 - char_h / 2;

    for (gi, glyph) in glyphs.iter().enumerate() {
        let gx = x0 + gi * (char_w + gap);
        for (row, &bits) in glyph.iter().enumerate() {
            for col_i in 0..5usize {
                if (bits >> (4 - col_i)) & 1 == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            set_pixel(buf, w, gx + col_i * scale + sx, y0 + row * scale + sy, col);
                        }
                    }
                }
            }
        }
    }
}
