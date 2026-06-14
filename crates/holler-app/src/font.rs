//! Shared text rendering for the softbuffer overlays (the recording pill, the
//! read-aloud status popup, and the clipboard toast).
//!
//! Glyphs are rasterized from the **embedded Inter** typeface (`assets/Inter.ttf`,
//! SIL OFL — see `assets/Inter-OFL.txt`) via `ab_glyph`, anti-aliased into the
//! XRGB softbuffer. This replaces the original hand-rolled 5×7 bitmap font: a
//! real proportional face with coverage AA is what makes the overlays read as a
//! modern macOS surface rather than a fixed-cell terminal (docs/DISCOVERIES.md,
//! 2026-06-13). `ab_glyph` is already in the tree (egui pulls it transitively),
//! and the font parses once into a process-wide `OnceLock`, so per-frame redraws
//! just walk cached glyph outlines.

use std::sync::OnceLock;

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};

use crate::overlay::{blend, Rgb};

/// The embedded variable Inter face; `ab_glyph` renders its default (Regular)
/// master, which is the weight we want for UI chrome.
static FONT_BYTES: &[u8] = include_bytes!("../assets/Inter.ttf");

/// Render size in pixels. Inter at 15px gives a crisp, legible label that fits
/// comfortably inside the ~56–72px-tall overlay pills.
pub const SIZE_PX: f32 = 15.0;

/// Parse the embedded font once and hand back a shared reference.
fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        FontRef::try_from_slice(FONT_BYTES).expect("embedded Inter font must parse")
    })
}

/// Height (px) of one line of text at [`SIZE_PX`] — ascent over descent. Used by
/// the overlays to vertically centre a label inside a pill.
pub fn text_height() -> i32 {
    let scaled = font().as_scaled(PxScale::from(SIZE_PX));
    (scaled.ascent() - scaled.descent()).round() as i32
}

/// Total advance width (px) of `msg` at [`SIZE_PX`], including kerning. Zero for
/// the empty string.
pub fn text_width(msg: &str) -> i32 {
    let scaled = font().as_scaled(PxScale::from(SIZE_PX));
    let mut width = 0.0;
    let mut prev = None;
    for ch in msg.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            width += scaled.kern(p, id);
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    width.ceil() as i32
}

/// Draw `msg` with the text block's top-left at `(x0, y0)` into a `buf_w`×`buf_h`
/// XRGB buffer, anti-aliased in `col`. The baseline is derived from the font's
/// ascent so the same `(x0, y0)` convention as the old bitmap font still holds.
/// Pixels outside the buffer are clipped.
pub fn draw_text(buf: &mut [u32], buf_w: i32, buf_h: i32, x0: i32, y0: i32, msg: &str, col: Rgb) {
    let font = font();
    let scaled = font.as_scaled(PxScale::from(SIZE_PX));
    let baseline = y0 as f32 + scaled.ascent();
    let mut caret = x0 as f32;
    let mut prev = None;
    for ch in msg.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            caret += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(SIZE_PX, point(caret, baseline));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            let ox = bounds.min.x as i32;
            let oy = bounds.min.y as i32;
            outline.draw(|gx, gy, coverage| {
                blend_px(buf, buf_w, buf_h, ox + gx as i32, oy + gy as i32, col, coverage);
            });
        }
        caret += scaled.h_advance(id);
        prev = Some(id);
    }
}

/// Alpha-composite `col` at `(x, y)` with coverage `a`, bounds-checked to the
/// `buf_w`×`buf_h` buffer.
fn blend_px(buf: &mut [u32], buf_w: i32, buf_h: i32, x: i32, y: i32, col: Rgb, a: f32) {
    if x < 0 || y < 0 || x >= buf_w || y >= buf_h {
        return;
    }
    blend(buf, (y * buf_w + x) as usize, col, a);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_font_parses() {
        // A panic here means the asset is missing/corrupt at build time.
        let _ = font();
        assert!(text_height() > 0);
    }

    #[test]
    fn text_width_is_zero_for_empty_and_grows_with_length() {
        assert_eq!(text_width(""), 0);
        assert!(text_width("WW") > text_width("W"));
        assert!(text_width("W") > 0);
    }

    #[test]
    fn draw_text_writes_pixels_and_clips_to_buffer() {
        // Into a real buffer: at least one pixel should be touched.
        let (w, h) = (80, 24);
        let mut buf = vec![0u32; (w * h) as usize];
        draw_text(&mut buf, w, h, 2, 2, "Hi", (255, 255, 255));
        assert!(buf.iter().any(|&p| p != 0), "draw_text painted nothing");

        // A 1-px buffer must absorb a multi-glyph string via bounds checks.
        let mut tiny = [0u32; 1];
        draw_text(&mut tiny, 1, 1, 0, 0, "Speaking", (255, 255, 255));
    }
}
