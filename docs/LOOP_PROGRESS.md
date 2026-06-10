# Holler — Loop Progress

Living checklist for the `/loop` self-build. The loop updates this every
iteration. The GUI backlog (P0–P9) lives in the loop prompt; rules in CLAUDE.md.

## Handoff (latest)
- **2026-06-10** — **P0 (egui integration spike) DONE.** Branch
  `feature/gui-egui-spike` (off main), commits `f2765d4` (code) + docs commit,
  clean tree, build/clippy/tests green (26 tests).
  **STOP per protocol — Yassir must confirm the spike before P1 features.**

  **What Yassir must visually verify (the part I can't):**
  1. `cd /Users/mosaab/Documents/Projects/Holler && git checkout feature/gui-egui-spike`
  2. `./scripts/bundle-macos.sh && open ./Holler.app` (or just `cargo run`)
  3. Tray menu → **"Settings…"** → a 720×480 "Holler Settings" window opens
     **in front** (heading + one label on a dark panel — intentionally empty).
  4. Close it (red traffic light) → reopen from the tray → must come back
     cleanly. Repeat open/close a few times.
  5. While the window is open AND after closing it: hold **Ctrl+Alt+Space**,
     speak, release → recording overlay pill appears, transcript still lands
     at cursor/clipboard/history; tray animation unaffected.
  6. Resize the window; drag between monitors if available (Retina vs non-
     Retina: text should stay sharp — egui gets the scale factor per-frame).
  7. Windows (CI or a VM, later): same open/close/reopen; settings window
     should show in taskbar (unlike the overlay) and close via ✕.

  **Renderer decision:** `egui_glow 0.34.3` + `glutin 0.32.3` (manual
  `egui-winit`, never eframe) — over egui-wgpu (dep/RSS cost) and softbuffer
  (unsupported for egui). Rationale + gotchas in DISCOVERIES 2026-06-10.

  **Next (after Yassir's go-ahead):** P1 — settings shell + sidebar navigation
  (General · Hotkey · Providers · Permissions · History · Stats · About),
  placeholder panels, sensible sizing/theme. Then P2 config editing.

## Backlog (GUI loop, 2026-06-10)
- [x] **P0 egui integration spike** — egui_glow inside the single winit loop;
  tray "Settings…" opens/closes on-demand window; renderer decision recorded.
- [ ] **P1 settings shell + navigation** (awaiting P0 visual confirmation)
- [ ] **P2 config view/edit (General + Hotkey live re-register)**
- [ ] **P3 providers & keys (secrets.toml, coming-soon list)**
- [ ] **P4 permissions panel**
- [ ] **P5 history viewer**
- [ ] **P6 stats**
- [ ] **P7 overlay redesign** (STOP for visual QA)
- [ ] **P8 clipboard-fallback toast + config flag**
- [ ] **P9 branding assets + packaging polish** (STOP for visual QA)

## Log
- 2026-06-10 — P0 egui spike — f2765d4 — egui_glow+glutin in the single loop; build+clippy+26 tests green; smoke-launch OK; **STOPPED for visual QA**
- 2026-06-09 — loop 30mn tasks complete — b7392dd+8c28115 — overlay pre-render fix, CI bundle fix; 25 tests green; LOOP STOPPED (pre-GUI backlog done)
- 2026-06-08 — task 2 (VAD silence trim) — e153ce7 — build+clippy+tests green; 3 new VAD tests
- 2026-06-08 — task 1 (remappable PTT key) — b5a5cfb — build+clippy+tests green; 7 new parser tests
