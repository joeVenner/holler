# Holler — Loop Progress

Living checklist for the `/loop` self-build. The loop updates this every
iteration. The GUI backlog (P0–P9) lives in the loop prompt; rules in CLAUDE.md.

## Handoff (latest)
- **2026-06-10 (later)** — **AUTO MODE ON** (Yassir: don't wait for
  confirmation; push + merge PRs for this repo allowed). **P0 merged to main
  (PR #3). P1 (settings shell + navigation) DONE** on
  `feature/gui-settings-shell`: sidebar General · Hotkey · Providers ·
  Permissions · History · Stats · About, placeholder panels, About already
  real (name/version/licence), dark app theme, 760×520 centred on the
  primary monitor, min size 640×420. Gates green (26 tests), smoke-launch OK.
  **Next:** P2 — config view/edit (General + Hotkey live re-register).

  **Visual QA backlog for Yassir (non-blocking, auto mode):**
  - P0/P1: rebuild (`./scripts/bundle-macos.sh && open Holler.app`) → tray →
    "Settings…": window opens centred + in front, dark theme; sidebar routes
    all 7 sections; About shows v0.1.0; close → reopen clean; PTT keeps
    working with the window open; resize ≥ 640×420 only; Retina text sharp.
  - Windows (CI later): window in taskbar, ✕ closes, same routing.

## Backlog (GUI loop, 2026-06-10)
- [x] **P0 egui integration spike** — egui_glow inside the single winit loop;
  tray "Settings…" opens/closes on-demand window; renderer decision recorded.
  Merged: PR #3.
- [x] **P1 settings shell + navigation** — 7-section sidebar, placeholder
  panels, real About, dark theme, centred 760×520.
- [ ] **P2 config view/edit (General + Hotkey live re-register)**
- [ ] **P3 providers & keys (secrets.toml, coming-soon list)**
- [ ] **P4 permissions panel**
- [ ] **P5 history viewer**
- [ ] **P6 stats**
- [ ] **P7 overlay redesign** (STOP for visual QA)
- [ ] **P8 clipboard-fallback toast + config flag**
- [ ] **P9 branding assets + packaging polish** (STOP for visual QA)

## Log
- 2026-06-10 — AUTO MODE granted by Yassir (push/PR/merge OK, no QA gates); P0 merged via PR #3
- 2026-06-10 — P1 settings shell — feature/gui-settings-shell — sidebar nav + placeholders + About; egui 0.34 Panel::left rename gotcha → DISCOVERIES; 26 tests green
- 2026-06-10 — P0 egui spike — f2765d4 — egui_glow+glutin in the single loop; build+clippy+26 tests green; smoke-launch OK; stopped for visual QA (gate later lifted)
- 2026-06-09 — loop 30mn tasks complete — b7392dd+8c28115 — overlay pre-render fix, CI bundle fix; 25 tests green; LOOP STOPPED (pre-GUI backlog done)
- 2026-06-08 — task 2 (VAD silence trim) — e153ce7 — build+clippy+tests green; 3 new VAD tests
- 2026-06-08 — task 1 (remappable PTT key) — b5a5cfb — build+clippy+tests green; 7 new parser tests
