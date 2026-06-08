# Holler — Loop Progress

Living checklist for the `/loop` self-build. The loop updates this every
iteration. Rules + full task specs live in `docs/AUTOMODE.md`.

## Handoff (latest)
- **2026-06-08** — Loop not started yet. Branch `feature/phase-0-scaffold`, clean
  tree. Next up: task 1 (remappable PTT key). Scope: tasks 1–2 then STOP before
  the egui GUI. Pacing: every 30 minutes.

## Backlog
- [ ] **1. Remappable PTT key from config** — combo parser → register from
  `config.ptt_key`; tray tooltip + ready-log reflect it; fallback on bad input;
  parser unit tests.
- [ ] **2. Phase 1.5 Silero VAD silence trim** — verified VAD crate; trim
  leading/trailing silence in `holler-audio` before STT; `vad: bool` config gate
  (default true); synthetic-buffer unit tests. Back out + BLOCK if it won't build
  cleanly.

## Log
<!-- One line per iteration: YYYY-MM-DD — task — commit hash — note -->
