# Talker — Project North Star (read this first)

> **User:** Yassir. **Philosophy:** readability > cleverness; small iterative changes; ask instead of assuming; challenge suboptimal paths before coding (see global `~/.claude/CLAUDE.md`).

**Talker** is a cross-platform (Windows + macOS), memory-efficient, Rust **push-to-talk dictation** desktop app — a "walkie-talkie for your agents." Hold a keyboard combo, speak; on release the audio is transcribed (local-first, cloud opt-in), optionally cleaned up by an LLM, then **injected at the active cursor** and **copied to the clipboard + a searchable local history**. TTS read-back comes later.

---

## 🏷️ Naming: project renamed **Talker → Holler** (2026-06-08)

The product/binary/workspace are now **`holler`** (crates `holler-app`, future `holler-core`, …). The repo directory and the planning docs (`PLAN.md`, `DECISIONS.md`, `research/`) still say "Talker"/`talker-*` — a full rename sweep is pending Yassir's call (see open question in chat). Treat any `talker-*` crate name in the docs as `holler-*`.

## ⚠️ Status: PHASE 0 SCAFFOLD DONE — awaiting interactive verification

- ✅ Git repo initialised (`feature/phase-0-scaffold`); Cargo workspace + `crates/holler-app` (binary `holler`) + `mimalloc` + lean release profile.
- ✅ The one hard integration risk is wired: a single main-thread `winit` loop owns `tray-icon` + `global-hotkey`; events funnel in as `UserEvent`s via `EventLoopProxy`. Builds clean, clippy-clean, and **smoke-passes** (`[holler] ready` prints; tray + hotkey init without panic).
- ⏳ **Remaining Phase 0 exit criteria need a human at the keyboard:** grant Accessibility, hold F8 → expect one `PTT DOWN`, release → one `PTT UP`, tray → Quit exits. (Can't be automated in-sandbox.)

Next action: have Yassir run `cargo run`, grant Accessibility, and confirm the PTT down/up + Quit behaviour. Then begin **Phase 1** (`docs/PLAN.md` §5).

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
