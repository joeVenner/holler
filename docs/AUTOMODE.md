# Holler — Autonomous Build Playbook (`/loop`)

Standing instructions for the self-build loop. **Re-read this file at the start
of every iteration** — it is the single source of truth. If this file and chat
memory disagree, this file wins (chat context is summarized away between fires).

---

## Operating rules (non-negotiable)

- **Branch & git:** Work only on `feature/phase-0-scaffold` (or a `feature/…`
  branch you create). **NEVER** commit to `main`/`master`. **NEVER** `git push`.
  **NEVER** force-push. Identity `joeVenner / ylafrimi@gmail.com`. Conventional
  Commits. **No AI attribution** in commit messages.
- **One small atomic task per iteration.** Readability > cleverness. Match the
  surrounding code's style.
- **Verify before you build on it:** check crate APIs against the installed
  source in `~/.cargo/registry/src/` and pin versions with `cargo add` — never
  guess. (Research agents get versions right but hallucinate APIs — see
  `docs/DISCOVERIES.md`.) For live web APIs, spawn a research agent.
- **Quality gate — ALL must pass before any commit:**
  - `cargo build` — no errors
  - `cargo clippy --quiet` — **zero** warnings
  - `cargo test` — all green
  - Startup smoke OK: run `./target/debug/holler` ~2s, expect `[holler] ready`,
    then kill it.
- **Do NOT make live STT calls** (Deepgram/OpenAI) — it spends Yassir's balance
  and needs his key. **Do NOT** attempt to verify injection, microphone, or
  Accessibility — they are interactive and impossible headless. Mark such checks
  "needs Yassir".
- **Never read or print the keychain secret.**
- **Never re-litigate locked decisions** in `docs/DECISIONS.md`.
- **Docs sync after each task:** append a `## [YYYY-MM-DD] Context Update` to
  `docs/DISCOVERIES.md` (newest-first), update `CLAUDE.md` Status + Next-action
  (and `docs/PLAN.md` if the roadmap shifts), tick `docs/LOOP_PROGRESS.md`, and
  update the memory file.
- **On a gate failure you can't quickly fix:** `git restore .` (discard the
  change), mark the task **BLOCKED** with the reason in `LOOP_PROGRESS.md`, and
  **stop the loop** for Yassir.

---

## Backlog (do the first unchecked task in `LOOP_PROGRESS.md`)

### 1. Remappable PTT key from config
- Parse `config.ptt_key` (e.g. `ctrl+alt+space`, `ctrl+shift+f8`, `f8`,
  `cmd+alt+d`) into `global_hotkey` `Modifiers` + `Code`. Case-insensitive;
  accept aliases: ctrl/control, cmd/super/meta, alt/opt/option, shift.
- Register the hotkey **from config** at init — the parsed combo replaces the
  compiled-in `PTT_MODS` / `PTT_CODE` constants in
  `crates/holler-app/src/main.rs` as the source of truth. On parse failure: log
  a clear warning and fall back to `Ctrl+Alt+Space`.
- Reflect the active combo in the tray tooltip **and** the
  `[holler] ready — hold X to talk` log line.
- Put the parser in `holler-config` (preferred) or `holler-app`, with unit
  tests: valid combos, invalid input → error/fallback, case-insensitivity,
  alias handling.
- **Acceptance:** build/clippy/tests green; parser tests cover the cases above.
  (Yassir verifies the live keypress + fallback behavior when back.)

### 2. Phase 1.5 — Silero VAD silence trim
- Add a **verified** VAD crate: `voice_activity_detector` (Silero v5, via `ort`)
  or `webrtc-vad` if the ONNX-runtime footprint is problematic. Pick whichever
  builds cleanly cross-platform and **document the choice** in DISCOVERIES.
  Verify its API against source first.
- In `crates/holler-audio`, trim leading/trailing silence from the 16 kHz mono
  buffer **before** it goes to STT. Guard against an all-silence clip (keep a
  small minimum / skip gracefully — never send an empty clip).
- Config-gate it: add `vad: bool` (default `true`) to `holler-config`; thread it
  through so the app can disable trimming.
- Unit-test the trim on **synthetic** buffers (silence-only, speech-in-middle,
  all-speech). No live audio.
- **If the VAD/ONNX dependency will not build cleanly, back it out
  (`git restore .` / `cargo remove`), mark the task BLOCKED, and stop** — do not
  commit a broken or bloated build.
- **Acceptance:** whole-workspace build/clippy/tests green; a synthetic
  silence-padded buffer is measurably trimmed.

---

## Stop conditions (end the loop — do NOT reschedule)
- Both backlog tasks are ✅ or BLOCKED.
- You reach the **egui / Phase-2 settings window** — STOP; Yassir oversees that
  (the project's 2nd hard integration risk).
- Anything needs a new decision from Yassir, a secret, a `git push`, or
  interactive verification you cannot do headless.

When stopping, write a clear **Handoff** at the top of `LOOP_PROGRESS.md`: what's
done, what's next, and anything Yassir must verify interactively.
