# Holler — Loop Progress

Living checklist for the `/loop` self-build. The loop updates this every
iteration. Rules + full task specs live in `docs/AUTOMODE.md`.

## Handoff (latest)
- **2026-06-08** — Task 1 done. Branch `feature/phase-0-scaffold`, clean tree.
  Next up: task 2 (Silero VAD silence trim). Stop before egui GUI.

## Backlog
- [x] **1. Remappable PTT key from config** — combo parser → register from
  `config.ptt_key`; tray tooltip + ready-log reflect it; fallback on bad input;
  parser unit tests.
- [ ] **2. Phase 1.5 Silero VAD silence trim** — verified VAD crate; trim
  leading/trailing silence in `holler-audio` before STT; `vad: bool` config gate
  (default true); synthetic-buffer unit tests. Back out + BLOCK if it won't build
  cleanly.

## Log
- 2026-06-08 — task 1 (remappable PTT key) — b5a5cfb — build+clippy+tests green; 7 new parser tests
