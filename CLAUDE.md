# Holler — Project North Star (read this first)

> **User:** Yassir. **Philosophy:** readability > cleverness; small iterative changes; ask instead of assuming; challenge suboptimal paths before coding (see global `~/.claude/CLAUDE.md`).

**Holler** is a cross-platform (Windows + macOS), memory-efficient, Rust **push-to-talk dictation** desktop app — a "walkie-talkie for your agents." Hold a keyboard combo, speak; on release the audio is transcribed (local-first, cloud opt-in), optionally cleaned up by an LLM, then **injected at the active cursor** and **copied to the clipboard + a searchable local history**. TTS read-back comes later.

---

## 🏷️ Naming: renamed **Talker → Holler** (2026-06-08, full sweep)

Originally planned as "Talker"; renamed to **Holler** before the first code. Product/binary/workspace are `holler`; crates are `holler-app` (and future `holler-core`, `holler-audio`, …). Code, docs, and the repo directory have all been swept. (Historical "Talker" mentions may linger in git history.)

## ⚠️ Status: PHASE 1 MVP CODE-COMPLETE + macOS .app bundle — awaiting interactive verification

- ✅ **Phase 0** (`feature/phase-0-scaffold`): Cargo workspace + `crates/holler-app` (binary `holler`) + `mimalloc` + lean release profile. Single main-thread `winit` loop owns `tray-icon` + `global-hotkey`; events funnel in as `UserEvent`s via `EventLoopProxy` (`ControlFlow::Wait` + forwarder thread, no polling). PTT key = **Ctrl+Alt+Space** (F8 collided with macOS media keys). Smoke-passes.
- ✅ **Phase 1 · audio** (`crates/holler-audio`): `AudioCapture` opens the mic only while PTT is held (cpal 0.18, `!Send` Stream on main thread), normalises any format → f32, downmixes to mono, resamples to 16 kHz (rubato 3.0 sinc). 4 unit tests.
- ✅ **Phase 1 · STT** (`crates/holler-stt`): provider-agnostic `SttProvider` trait + **`OpenAiStt`** (`gpt-4o-mini-transcribe`) and **`DeepgramStt`** (`nova-3`, smart_format), both cloud/BYOK. App picks the provider by stored key (Deepgram preferred), resolved **lazily on the worker thread** (never reads the keychain on the launch path — that blocks on the OS prompt). Key in OS keychain via `holler set-key <openai|deepgram> <KEY>` (keyring 3). **Cloud STT pulled forward from Phase 2** per Yassir. clippy-clean, 6 unit tests.
- ✅ **Phase 1 · deliver** (`holler-inject` + `holler-store` + `holler-config`): on a transcript the app copies to clipboard, records to SQLite history, and injects at the cursor (paste chord, or type). Provider/model/injection-mode driven by a TOML config (`<config_dir>/Holler/config.toml`, default provider `deepgram`). 13 unit tests; clippy-clean.
- ✅ **Packaging** (`scripts/bundle-macos.sh`): double-clickable, ad-hoc-signed `Holler.app` (LSUIElement menubar agent, mic usage string). A bundle gives macOS the **stable identity** required to grant **Accessibility** (synthesise paste/type) and remember keychain access — the fix for the "no permission to simulate input" error that bare `cargo run` binaries hit. See `README.md`.
- ⏳ **Needs a human at the keyboard:** `scripts/bundle-macos.sh` → `open ./Holler.app`; grant **Accessibility** (enable Holler in System Settings) + **Microphone**; focus a text field, hold Ctrl+Alt+Space, speak, release → text at cursor + clipboard + history.

**Decision update (2026-06-08):** **Local Whisper deferred** — Deepgram (`nova-3`) is the focus. **LLM cleanup deferred to an optional, off-by-default Phase-2 toggle** — Deepgram does the cleanup server-side (`smart_format` + `dictation`; "um/uh" stripped by default). An LLM is only needed for repetition/false-start removal + rephrasing.

- ✅ **Tray UX:** state-aware **animated** tray (idle blue dot / recording pulsing red / processing spinner; `ControlFlow::WaitUntil` only while active, full sleep when idle). Menu has "Edit Settings (config.toml)" + "Open History Folder" as a stopgap settings entry.
- ✅ **Deepgram dictation mode:** query now `smart_format=true&dictation=true&punctuate=true` — spoken punctuation/newlines + formatting, server-side.

Next action: **Phase 2 GUI build-out** (docs/LOOP_PROGRESS.md backlog P1–P9). The hard 2nd integration risk is **cleared**: the egui settings window spike landed 2026-06-10 (`feature/gui-egui-spike`) — manual `egui-winit`+**`egui_glow`** (re-decided over egui-wgpu, see DISCOVERIES) in the same single winit loop, window on demand from the tray's "Settings…" item, dropped on close. Remaining: settings panels (provider/model menu, remappable PTT key, keys, permissions, history, stats), overlay redesign, branding. Phase 1.5 (webrtc-vad trim) is done.

Before writing code, read in order:
1. `docs/DECISIONS.md` — every locked decision + open questions. **Do not re-litigate these.**
2. `docs/PLAN.md` — architecture, crate stack, module layout, phased roadmap.
3. `docs/research/` — the sourced research that justifies the choices (4 reports).
4. `docs/DISCOVERIES.md` — hard-learned lessons (append as you learn).

---

## Locked decisions (from planning, 2026-06-08)

| Decision | Choice |
|---|---|
| Scope | STT dictation → optional LLM cleanup → inject. **TTS read-back deferred to phase 3.** No voice commands in v1. |
| STT backend | **Hybrid**: local-first (`whisper-rs`/whisper.cpp) default, cloud opt-in (Deepgram / OpenAI). |
| PTT trigger | **Keyboard combo only** (clean `global-hotkey` path, no mouse → no `rdev` → no silent-fail macOS perm). |
| App shell | **Native tray** (`tray-icon`) + on-demand `egui` settings. **No resident WebView.** Not Tauri. |
| "Copy memory" | **Both**: set system clipboard AND keep searchable SQLite history. |
| AI providers | **Provider-agnostic / BYOK**: Claude, OpenAI, Deepgram, and local OpenAI-compatible (Ollama) all configurable behind traits. |
| Distribution | Build with code-signing + notarization + permission onboarding in mind from day one. |
| Local model default | **Assumed: auto-detect** (`large-v3-turbo` on capable HW, `small` fallback). ⚠️ Not explicitly confirmed by Yassir — see open questions. |

## Recommended decisions I made (challenge if needed)
- **Defer the GUI to phase 2**; ship Phase 1 with a TOML config file to keep the only thorny integration risk off the critical path.
- **Cloud STT = batch-on-release**, not streaming (short utterances gain little from streaming).

## The one hard integration risk
On macOS, `global-hotkey` + `tray-icon` both need the **main-thread event loop**, and `egui` via `eframe` wants its own loop. Solution: a single main-thread **`winit`** loop owns tray + hotkey; render settings with **manual `egui-winit` + `egui-wgpu`** inside that loop — never `eframe::run_native`. Phase 0 must prove the tray + hotkey loop before anything else.

## Core crate stack (verified current, see PLAN.md §2)
`winit` (loop) · `tray-icon`+`muda` (tray) · `global-hotkey` (PTT, use `Pressed`/`Released`, debounce auto-repeat) · `cpal` (audio) · `rubato` (48k→16k mono) · `voice_activity_detector` (Silero, phase 1.5) · `whisper-rs` (local STT; `metal` on mac) · `reqwest`+`tokio-tungstenite` (cloud, raw HTTP/WS — no vendor SDK) · `enigo`+`arboard` (inject) · `rusqlite` (history) · `keyring` (API keys — NEVER in config files) · `serde`+`toml`+`directories` (config) · `mimalloc` (idle RSS) · `tokio` (async).

## Injection strategy (layered)
1. Clipboard-paste (save→set→Cmd/Ctrl+V via enigo→wait ~80–120ms→restore) — primary.
2. Keystroke typing (`enigo.text()`) — fallback for terminals/RDP that block paste.
3. Manual (leave on clipboard + notify) — macOS secure-input fields & Windows elevated/UIPI windows (unbypassable by design).

---

## 🗂️ Project Discovery Protocol (per global rule)
Analyze stack, read this file + `docs/`, mimic existing patterns before coding.

## 📝 Documentation Sync Rule (per global rule)
After significant changes append to `docs/DISCOVERIES.md` using the `[YYYY-MM-DD] Context Update` format, and keep this file fresh (update the Status section + decision table when reality changes).

## 🔗 Git
Follow `~/.claude/rules/git.md`. Repo initialised 2026-06-08 (work on `feature/phase-0-scaffold`). Never commit to main; branch as `feature/...`; identity `joeVenner / ylafrimi@gmail.com`; never commit secrets/`.env`/keys.
