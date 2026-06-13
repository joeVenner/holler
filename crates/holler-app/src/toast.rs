//! Transient "Copied to clipboard — paste it" notification.
//!
//! Shown when auto-paste can't run (Accessibility not granted, or injection
//! failed) so the user knows the transcript is on the clipboard and just needs
//! a manual paste. Like the recording overlay it's a borderless, always-on-top,
//! **non-activating** softbuffer window (it must never steal focus from the
//! field the user is about to paste into), reusing the overlay's anti-aliased
//! pill paint helpers. Text is a dependency-free 5×7 bitmap font (see
//! `glyph`) — egui would need a GL context for a momentary toast, and `ab_glyph`
//! would need an embedded TTF; for a few fixed words a tiny bitmap font is the
//! leaner, self-contained choice (docs/DISCOVERIES.md, 2026-06-12).

use std::num::NonZeroU32;
use std::sync::Arc;

use softbuffer::{Context, Surface};
use winit::{
    dpi::{LogicalSize, PhysicalPosition},
    event_loop::ActiveEventLoop,
    window::{Window, WindowAttributes, WindowLevel},
};

use crate::font::{self, GLYPH_H, SCALE};
use crate::overlay::{blend, pack, sd_round_rect, Rgb};

pub const WIDTH: u32 = 420;
pub const HEIGHT: u32 = 56;
/// Seconds the toast stays on screen before auto-dismissing.
pub const VISIBLE_SECS: u64 = 4;

const BG: Rgb = (18, 18, 20);
const PILL: Rgb = (40, 40, 46);
const RING: Rgb = (70, 70, 80);
const TEXT: Rgb = (224, 224, 230);
const ACCENT: Rgb = (96, 174, 255); // small left dot, echoes the overlay meter

/// Owns the toast window and its softbuffer surface. Created once (hidden) and
/// shown on demand — the same resident-but-hidden model as the overlay.
pub struct Toast {
    window: Arc<Window>,
    _ctx: Context<Arc<Window>>,
    surface: Surface<Arc<Window>, Arc<Window>>,
}

impl Toast {
    /// Build the toast window (hidden). `None` on failure — a missing toast must
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
        // Sit higher than the recording overlay (which hugs the bottom) so the
        // two never visually collide if they're ever briefly co-resident.
        let margin = (110.0 * scale) as i32;
        let x = origin.x + (monitor_size.width as i32 - win_w) / 2;
        let y = origin.y + monitor_size.height as i32 - win_h - margin;

        let attrs = WindowAttributes::default()
            .with_title("Holler Notification")
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_position(PhysicalPosition::new(x, y))
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false);

        // Windows: keep it out of the taskbar and, crucially, don't let it take
        // focus — the user is about to paste into another app.
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
        })
    }

    /// Render `msg` and reveal the toast. The caller arms the auto-dismiss timer.
    pub fn show_message(&mut self, msg: &str) {
        self.render(msg);
        self.window.set_visible(true);
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    fn render(&mut self, msg: &str) {
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
        paint(&mut buf, msg);
        buf.present().ok();
    }
}

/// Paint the pill + accent dot + centred message into the softbuffer.
fn paint(buf: &mut [u32], msg: &str) {
    let w = WIDTH as i32;
    let h = HEIGHT as i32;
    let cx = w as f32 / 2.0;
    let cy = h as f32 / 2.0;
    let half_w = w as f32 / 2.0 - 1.5;
    let half_h = h as f32 / 2.0 - 1.5;
    let radius = half_h;

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

    // Small accent dot on the left.
    draw_dot(buf, 22.0, cy, 5.0, ACCENT);

    // Centre the text in the space right of the dot.
    let text_left = 40;
    let text_w = font::text_width(msg);
    let avail = w - text_left - 18;
    let x0 = text_left + (avail - text_w).max(0) / 2;
    let y0 = (h - GLYPH_H * SCALE) / 2;
    font::draw_text(buf, w, h, x0, y0, msg, TEXT);
}

/// Anti-aliased filled dot (coverage from distance), bounds-checked.
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
                put(buf, x, y, col, cov);
            }
        }
    }
}

/// `blend` addressed by pixel coordinate within the toast, bounds-checked.
fn put(buf: &mut [u32], x: i32, y: i32, col: Rgb, a: f32) {
    if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
        return;
    }
    blend(buf, (y * WIDTH as i32 + x) as usize, col, a);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toast_message_fits_the_window() {
        let msg = "COPIED TO CLIPBOARD — PASTE IT";
        // Must fit between the accent dot and the right padding.
        assert!(
            font::text_width(msg) <= WIDTH as i32 - 40 - 18,
            "toast text overflows the pill"
        );
    }
}
