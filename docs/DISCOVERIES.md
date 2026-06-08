# Talker — Discoveries Log

Append hard-learned technical lessons and edge cases here, newest first, using the format below.

```
## [YYYY-MM-DD] Context Update
- **What changed:** ...
- **Why:** ...
- **Impact:** ...
- **Reference:** commit / PR / file
```

---

## [2026-06-08] Context Update — Phase 0 scaffold + integration spike (project renamed Talker → Holler)
- **What changed:** `git init`; Cargo workspace + `crates/holler-app` (binary `holler`). Built the single-loop spike that the whole architecture bets on: one main-thread `winit` 0.30 loop owning `tray-icon` + `global-hotkey`. Builds clean, clippy-clean, smoke-passes (init without panic). Interactive PTT/Quit verification still pending a human + Accessibility grant.
- **Why:** Phase 0's mandate is to prove the risky integration before any audio/STT work (PLAN.md §0, §5).
- **Versions pinned (verified live on crates.io 2026-06-08 via a 5-agent fan-out):** `winit 0.30.13`, `tray-icon 0.24.0`, `muda 0.19.2` (transitive, via `tray_icon::menu`), `global-hotkey 0.8.0`, `mimalloc 0.1.52`.
- **Hard-learned API/integration lessons:**
  - **`HotKeyState` lives at the `global_hotkey` crate root**, not under `global_hotkey::hotkey` (compile error otherwise). `GlobalHotKeyEvent` exposes `.id: u32` and `.state` as **public fields** (not methods).
  - **`global-hotkey` has no callback API — only a `static` channel** (`GlobalHotKeyEvent::receiver()`). Polling it under `ControlFlow::Poll` is a busy loop and violates the "no polling" goal (PLAN.md §6). Solution used: keep the loop in `ControlFlow::Wait` and drain the channel on a **dedicated forwarder thread** that blocks on `recv()` and wakes the loop via `EventLoopProxy::send_event()`. Tray/menu use `set_event_handler` → same proxy. One unified `UserEvent` channel.
  - **macOS main-thread rules confirmed:** tray icon AND hotkey manager must be created on the main thread *after* the loop starts → done in `ApplicationHandler::resumed()` (made idempotent; it can fire more than once).
  - **`set_event_handler` and `::receiver()` are mutually exclusive** for tray/menu events (handler wins; receiver goes silent). We use the handler.
  - **PTT edge detection:** a single `ptt_held: bool` is the source of truth — first `Pressed` logs DOWN, subsequent `Pressed` (OS auto-repeat) are ignored, `Released` logs UP. This is the debounce.
  - Dropped `muda` as a *direct* dependency — consuming it via the `tray_icon::menu` re-export keeps menu/tray versions locked in lockstep (the agent flagged version-mismatch init failures on macOS).
  - `panic = "abort"` is set only on `[profile.release]`; dev/test keep unwinding (test harness requires it, and winit's macOS event loop is happier unwinding during iteration).
- **Reference:** `Cargo.toml`, `crates/holler-app/src/main.rs`; workflow run `wf_46a3aa0c-3eb`.

## [2026-06-08] Context Update — Planning research (pre-code)
- **What changed:** Completed a 4-stream deep research pass (existing apps, Rust stack, STT, text injection) and locked the architecture. No code yet.
- **Why:** De-risk the build by grounding every choice in current (2025–2026) sourced evidence before scaffolding.
- **Impact:** Established crate stack, module layout, phased roadmap (`PLAN.md`) and locked decisions (`DECISIONS.md`).
- **Key lessons surfaced (verify at integration time):**
  - `global-hotkey` is **keyboard-only** (no mouse); supports `Pressed`/`Released` but auto-repeat needs debouncing and `Released` has documented edge cases.
  - `tray-icon` + `global-hotkey` + `egui` contend for the macOS main-thread event loop → use one `winit` loop + manual egui integration, not `eframe`.
  - Tauri's idle-RAM advantage over Electron is **marginal on Windows** (shared WebView2 memory) — validated the choice to go native-tray.
  - Injection: clipboard-paste is the industry default (Wispr Flow confirms); per-char keystroke typing is slow/racy for long text → fallback only.
  - macOS secure-input fields and Windows UIPI/elevated windows **cannot** be injected into → manual-paste fallback is mandatory, not optional.
  - For short PTT utterances, cloud **streaming** STT buys little over batch-on-release.
  - whisper.cpp `large-v3-turbo` is the local sweet spot; auto-select smaller models on weak hardware.
- **Reference:** `docs/research/01-04`, `docs/PLAN.md`, `docs/DECISIONS.md`.
