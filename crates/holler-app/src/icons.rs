//! Programmatically-drawn tray icons (no binary assets committed). Each
//! function returns a 32×32 RGBA buffer for `tray_icon::Icon::from_rgba`.
//!
//! - [`idle`]: a calm blue dot — "ready".
//! - [`recording`]: a pulsing red dot with an expanding halo — "listening".
//! - [`processing`]: a comet-trail spinner — "transcribing".
//!
//! `recording`/`processing` take a frame index in `0..FRAMES`; the app cycles
//! it on a timer while in that state.

use std::f32::consts::TAU;

/// Icon edge length in pixels.
pub const SIZE: u32 = 32;
/// Frames in one animation loop.
pub const FRAMES: usize = 12;

const BLUE: [u8; 3] = [0x5b, 0x8d, 0xef];
const RED: [u8; 3] = [0xff, 0x46, 0x46];

fn blank() -> Vec<u8> {
    vec![0u8; (SIZE * SIZE * 4) as usize]
}

/// Alpha-blend `rgb` at `coverage` (0..1) over the pixel at (x, y), src-over.
fn blend(buf: &mut [u8], x: i32, y: i32, rgb: [u8; 3], coverage: f32) {
    let n = SIZE as i32;
    if x < 0 || y < 0 || x >= n || y >= n {
        return;
    }
    let a = coverage.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = ((y * n + x) * 4) as usize;
    let dst_a = buf[i + 3] as f32 / 255.0;
    let out_a = a + dst_a * (1.0 - a);
    for c in 0..3 {
        let src = rgb[c] as f32;
        let dst = buf[i + c] as f32;
        let out = if out_a > 0.0 {
            (src * a + dst * dst_a * (1.0 - a)) / out_a
        } else {
            0.0
        };
        buf[i + c] = out.round().clamp(0.0, 255.0) as u8;
    }
    buf[i + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Anti-aliased filled disc at `max_alpha` peak opacity.
fn disc(buf: &mut [u8], cx: f32, cy: f32, r: f32, rgb: [u8; 3], max_alpha: f32) {
    let lo = (cx.min(cy) - r - 1.0).floor() as i32;
    let hi = (cx.max(cy) + r + 1.0).ceil() as i32;
    for y in lo..=hi {
        for x in lo..=hi {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let cov = (r - d + 0.5).clamp(0.0, 1.0);
            blend(buf, x, y, rgb, cov * max_alpha);
        }
    }
}

/// "Ready" — a calm, static blue dot.
pub fn idle() -> Vec<u8> {
    let mut b = blank();
    let c = SIZE as f32 / 2.0;
    disc(&mut b, c, c, 6.0, BLUE, 0.95);
    b
}

/// "Listening" — a pulsing red core with a fading expanding halo.
pub fn recording(frame: usize) -> Vec<u8> {
    let mut b = blank();
    let c = SIZE as f32 / 2.0;
    let phase = frame as f32 / FRAMES as f32;
    let pulse = (phase * TAU).sin() * 0.5 + 0.5; // 0..1

    // Expanding halo, brightest when the core is smallest.
    disc(&mut b, c, c, 8.0 + pulse * 6.0, RED, 0.22 * (1.0 - pulse));
    // Pulsing solid core.
    disc(&mut b, c, c, 6.0 + pulse * 1.8, RED, 1.0);
    b
}

/// "Transcribing" — a blue ring with a bright head that orbits, leaving a trail.
pub fn processing(frame: usize) -> Vec<u8> {
    let mut b = blank();
    let n = SIZE as i32;
    let c = SIZE as f32 / 2.0;
    let outer = 11.0;
    let inner = 6.5;
    let head = frame as f32 / FRAMES as f32 * TAU;

    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let d = (dx * dx + dy * dy).sqrt();
            let ring = (outer - d + 0.5).clamp(0.0, 1.0) * (d - inner + 0.5).clamp(0.0, 1.0);
            if ring <= 0.0 {
                continue;
            }
            // Brightness trails behind the rotating head.
            let ang = dy.atan2(dx);
            let behind = (head - ang).rem_euclid(TAU) / TAU; // 0 at head → 1 around
            let brightness = 0.18 + 0.82 * (1.0 - behind).powi(2);
            blend(&mut b, x, y, BLUE, ring * brightness);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECT: usize = (SIZE * SIZE * 4) as usize;

    #[test]
    fn buffers_are_correctly_sized_and_nonempty() {
        for buf in [idle(), recording(0), processing(0)] {
            assert_eq!(buf.len(), EXPECT);
            assert!(buf.iter().any(|&b| b != 0), "icon should not be blank");
        }
    }

    #[test]
    fn animation_frames_differ() {
        // The recording pulse is a symmetric sine (frame 0 and FRAMES/2 share a
        // value), so compare a frame at the peak instead.
        assert_ne!(recording(0), recording(FRAMES / 4), "recording should animate");
        assert_ne!(processing(0), processing(FRAMES / 2), "processing should animate");
    }
}
