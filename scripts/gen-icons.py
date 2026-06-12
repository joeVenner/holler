#!/usr/bin/env python3
"""Regenerate Holler's icon assets from the master design (assets/holler.svg).

The icon is pure geometry — a charcoal squircle with a five-bar voice waveform
(blue level bars + a red record bar at centre), matching the recording overlay.
`assets/holler.svg` is the human-editable vector master; this script renders the
SAME geometry with Pillow so it runs without a native SVG/cairo backend (none is
available in this dev/CI box). If you edit holler.svg, mirror the change in the
GEOMETRY constants below (they use identical 1024-space coordinates).

Outputs (under assets/):
  - Holler.iconset/  : macOS PNG set (16…1024, @1x/@2x)
  - Holler.icns      : macOS bundle icon (via `iconutil`; macOS only)
  - holler.ico       : Windows .exe icon (multi-size, via Pillow)
  - holler-512.png   : preview for docs / quick eyeballing

Run: python3 scripts/gen-icons.py
"""
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

MASTER = 1024  # render at this size, then downscale (LANCZOS) for crisp AA.
BG_TOP = (0x26, 0x26, 0x2B)
BG_BOTTOM = (0x16, 0x16, 0x18)
RING = (0x3A, 0x3A, 0x42)
BLUE = (0x5B, 0x8D, 0xEF)
RED = (0xFF, 0x46, 0x46)

# Backdrop squircle and the five waveform bars, in 1024-space (mirror holler.svg).
SQUIRCLE = (96, 96, 96 + 832, 96 + 832)  # x0, y0, x1, y1
SQUIRCLE_RADIUS = 188
BAR_RADIUS = 42
#         x,   y,   w,   h,  colour
BARS = [
    (190, 402, 84, 220, BLUE),
    (330, 322, 84, 380, BLUE),
    (470, 252, 84, 520, RED),
    (610, 322, 84, 380, BLUE),
    (750, 402, 84, 220, BLUE),
]


def vertical_gradient(size: int, top: tuple, bottom: tuple) -> Image.Image:
    grad = Image.new("RGB", (size, size))
    draw = ImageDraw.Draw(grad)
    for y in range(size):
        t = y / (size - 1)
        col = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        draw.line([(0, y), (size, y)], fill=col)
    return grad


def render_master() -> Image.Image:
    """Draw the icon once at MASTER resolution as RGBA."""
    canvas = Image.new("RGBA", (MASTER, MASTER), (0, 0, 0, 0))

    # Charcoal squircle: a vertical gradient clipped to a rounded-rect mask.
    mask = Image.new("L", (MASTER, MASTER), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        SQUIRCLE, radius=SQUIRCLE_RADIUS, fill=255
    )
    canvas.paste(vertical_gradient(MASTER, BG_TOP, BG_BOTTOM).convert("RGBA"),
                 (0, 0), mask)

    draw = ImageDraw.Draw(canvas)
    # Hairline top highlight on the squircle edge.
    draw.rounded_rectangle(SQUIRCLE, radius=SQUIRCLE_RADIUS,
                           outline=RING + (180,), width=3)
    # Waveform bars.
    for x, y, w, h, col in BARS:
        draw.rounded_rectangle([x, y, x + w, y + h], radius=BAR_RADIUS,
                               fill=col + (255,))
    return canvas


def build_iconset(master: Image.Image) -> Path:
    iconset = ASSETS / "Holler.iconset"
    if iconset.exists():
        shutil.rmtree(iconset)
    iconset.mkdir(parents=True)
    specs = [
        (16, False), (16, True), (32, False), (32, True),
        (128, False), (128, True), (256, False), (256, True),
        (512, False), (512, True),
    ]
    for base, retina in specs:
        px = base * 2 if retina else base
        suffix = "@2x" if retina else ""
        master.resize((px, px), Image.LANCZOS).save(
            iconset / f"icon_{base}x{base}{suffix}.png")
    return iconset


def build_icns(iconset: Path) -> None:
    if sys.platform != "darwin" or shutil.which("iconutil") is None:
        print("note: iconutil unavailable (non-macOS) — skipping .icns; CI builds it")
        return
    out = ASSETS / "Holler.icns"
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(out)],
                   check=True)
    print(f"wrote {out.relative_to(ROOT)}")


def build_ico(master: Image.Image) -> None:
    sizes = [16, 24, 32, 48, 64, 128, 256]
    out = ASSETS / "holler.ico"
    master.resize((256, 256), Image.LANCZOS).save(
        out, format="ICO", sizes=[(s, s) for s in sizes])
    print(f"wrote {out.relative_to(ROOT)}")


def main() -> None:
    ASSETS.mkdir(exist_ok=True)
    master = render_master()
    iconset = build_iconset(master)
    build_icns(iconset)
    build_ico(master)
    master.resize((512, 512), Image.LANCZOS).save(ASSETS / "holler-512.png")
    print(f"wrote {(ASSETS / 'holler-512.png').relative_to(ROOT)}")
    print("done.")


if __name__ == "__main__":
    main()
