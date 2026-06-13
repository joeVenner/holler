//! Shared 5×7 bitmap font for the softbuffer overlays (the clipboard toast and
//! the read-aloud status popup). Uppercase A–Z plus the punctuation those
//! overlays need; lowercase folds to uppercase and anything unmapped renders
//! blank. Extracted from `toast.rs` so multiple overlays draw text identically
//! without duplicating the glyph table — egui would need a GL context for a
//! momentary overlay, and `ab_glyph` an embedded TTF; a fixed bitmap font stays
//! the leaner, self-contained choice (docs/DISCOVERIES.md, 2026-06-12).

use crate::overlay::{blend, Rgb};

/// Pixel scale of one glyph cell: a 5×7 glyph renders at 10×14 px.
pub const SCALE: i32 = 2;
pub const GLYPH_W: i32 = 5;
pub const GLYPH_H: i32 = 7;
/// Horizontal advance per glyph: the cell plus one blank column.
pub const ADVANCE: i32 = (GLYPH_W + 1) * SCALE;

/// Total rendered pixel width of `msg` (no trailing inter-glyph gap).
pub fn text_width(msg: &str) -> i32 {
    let n = msg.chars().count() as i32;
    if n == 0 {
        0
    } else {
        n * ADVANCE - SCALE
    }
}

/// Draw `msg` as scaled 5×7 glyphs with top-left at `(x0, y0)` into a `buf_w`×
/// `buf_h` XRGB buffer. Pixels outside the buffer are clipped.
pub fn draw_text(buf: &mut [u32], buf_w: i32, buf_h: i32, x0: i32, y0: i32, msg: &str, col: Rgb) {
    let mut x = x0;
    for ch in msg.chars() {
        let g = glyph(ch);
        for (row, bits) in g.iter().enumerate() {
            for c in 0..GLYPH_W {
                if (bits >> (GLYPH_W - 1 - c)) & 1 == 1 {
                    fill_cell(buf, buf_w, buf_h, x + c * SCALE, y0 + row as i32 * SCALE, col);
                }
            }
        }
        x += ADVANCE;
    }
}

/// Fill one `SCALE`×`SCALE` font cell (crisp, full coverage).
fn fill_cell(buf: &mut [u32], buf_w: i32, buf_h: i32, px: i32, py: i32, col: Rgb) {
    for dy in 0..SCALE {
        for dx in 0..SCALE {
            put(buf, buf_w, buf_h, px + dx, py + dy, col);
        }
    }
}

/// Opaque pixel write addressed by coordinate, bounds-checked against the buffer.
fn put(buf: &mut [u32], buf_w: i32, buf_h: i32, x: i32, y: i32, col: Rgb) {
    if x < 0 || y < 0 || x >= buf_w || y >= buf_h {
        return;
    }
    blend(buf, (y * buf_w + x) as usize, col, 1.0);
}

/// 5×7 uppercase bitmap font (each row is 5 bits, MSB = leftmost column).
fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        // Dash / em-dash: a single mid-height bar.
        '-' | '—' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100],
        '\'' => [0b00100, 0b00100, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000],
        // space and anything unmapped → blank cell.
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_width_is_zero_for_empty_and_scales_with_length() {
        assert_eq!(text_width(""), 0);
        assert_eq!(text_width("A"), GLYPH_W * SCALE);
        assert_eq!(text_width("AB"), GLYPH_W * SCALE + ADVANCE);
    }

    #[test]
    fn glyph_lookup_folds_case_and_blanks_unknown() {
        assert_eq!(glyph('a'), glyph('A'));
        assert_ne!(glyph('A'), [0; 7]);
        assert_eq!(glyph(' '), [0; 7]);
        assert_eq!(glyph('~'), [0; 7]);
    }

    #[test]
    fn draw_text_clips_to_buffer_without_panicking() {
        // A 1-px buffer must absorb a multi-glyph string via bounds checks.
        let mut buf = [0u32; 1];
        draw_text(&mut buf, 1, 1, 0, 0, "HELLO", (255, 255, 255));
    }
}
