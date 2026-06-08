# Holler — Loop Progress

Living checklist for the `/loop` self-build. The loop updates this every
iteration. Rules + full task specs live in `docs/AUTOMODE.md`.

## Handoff (latest)
- **2026-06-08** — Tasks 1 & 2 done. Branch `feature/phase-0-scaffold`, clean tree.
  Both backlog tasks complete. **STOP — next is Phase 2 (egui GUI), Yassir oversees.**
  Yassir should: run `scripts/bundle-macos.sh`, open `Holler.app`, grant Accessibility
  + Mic, dictate with `Ctrl+Alt+Space` — verify text appears at cursor, on clipboard,
  and in history. Can change `ptt_key` / `vad` in `config.toml` to test those gates.

## Backlog
- [x] **1. Remappable PTT key from config** — combo parser → register from
  `config.ptt_key`; tray tooltip + ready-log reflect it; fallback on bad input;
  parser unit tests.
- [x] **2. Phase 1.5 VAD silence trim** — `webrtc-vad` (chose over Silero/ONNX);
  `holler_audio::vad_trim`; `vad: bool` config gate; 3 synthetic-buffer tests green.

## Log
- 2026-06-08 — task 1 (remappable PTT key) — b5a5cfb — build+clippy+tests green; 7 new parser tests
- 2026-06-08 — task 2 (VAD silence trim) — e153ce7 — build+clippy+tests green; 3 new VAD tests
