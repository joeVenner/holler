# Holler — Implementation Plan

> Source of truth for architecture and roadmap. Decisions live in `DECISIONS.md`; sourced research in `research/`.

## 1. Architecture at a glance

```
                ┌─────────────────────────────────────────────┐
                │   main thread: winit event loop (the spine)  │
                │   owns → tray-icon + global-hotkey receiver  │
                └───────────────┬─────────────────────────────┘
                                │ PTT key DOWN / UP events
                                ▼
   ┌──────────────┐   hold   ┌────────────────┐  release  ┌──────────────────┐
   │ Audio capture│─────────▶│  Ring buffer    │──────────▶│ STT provider      │
   │ cpal @native │          │ f32 → mono 16k  │           │ (trait): local    │
   └──────────────┘          │ rubato resample │           │ whisper-rs │ cloud│
                             └────────────────┘            └─────────┬────────┘
                                                                     │ raw text
                                                                     ▼
                                                          ┌────────────────────┐
                                                          │ LLM cleanup (trait)│  (optional, per-mode)
                                                          │ Claude/OpenAI/local│
                                                          └─────────┬──────────┘
                                                                    │ final text
                            ┌───────────────────────────┬──────────┴───────────┐
                            ▼                            ▼                      ▼
                  ┌──────────────────┐        ┌──────────────────┐   ┌──────────────────┐
                  │ Injector (trait) │        │ Clipboard set    │   │ History store    │
                  │ paste→fallbacks  │        │ (arboard)        │   │ (SQLite)         │
                  └──────────────────┘        └──────────────────┘   └──────────────────┘
```

### The one hard integration risk
macOS: `global-hotkey` + `tray-icon` both require the main-thread event loop; `egui` via `eframe` wants its own. Solution: **single main-thread `winit` loop owns tray + hotkey**; render settings with **manual `egui-winit` + `egui-wgpu`** inside that loop — never `eframe::run_native`. Phase 0 proves the loop before anything else. (See research/02.)

## 2. Crate stack (verified current — research/02, /03, /04)

| Concern | Crate | Notes |
|---|---|---|
| Event loop | `winit` | Main-thread spine |
| Tray | `tray-icon` + `muda` | Native, no WebView; create on `StartCause::Init` (macOS main thread) |
| Hotkey (PTT) | `global-hotkey` | Use `HotKeyState::Pressed`/`Released`; bare key OK (`HotKey::new(None, Code::F8)`); debounce auto-repeat; treat `Released` defensively |
| Audio | `cpal` | Don't assume 16k/mono; convert f32 → downmix → resample. Open stream only while key held |
| Resample | `rubato` (`FftFixedIn`) | 48k→16k mono |
| VAD | `voice_activity_detector` (Silero v5, via `ort`) | Phase 1.5; trims silence. `webrtc-vad` if avoiding ONNX runtime footprint |
| Local STT | `whisper-rs` | `large-v3-turbo`; features: `metal` (mac), CPU/`vulkan` (Win). Mmap model |
| Cloud (STT/LLM/TTS) | `reqwest` (HTTP) + `tokio-tungstenite` (WS) | No vendor needs a Rust SDK |
| Injection | `enigo` (0.5.x) + `arboard` | enigo `text()` for Unicode typing; `key()` for Cmd/Ctrl+V |
| History/config store | `rusqlite` (bundled) | Searchable local history |
| Secrets | `keyring` | API keys in OS keychain — never in TOML |
| Config | `serde` + `toml` + `directories` | `~/.config/holler/` (XDG) / `~/Library/Application Support/Holler` |
| Allocator | `mimalloc` | `#[global_allocator]`, lower idle RSS |
| Async | `tokio` | Network providers |

## 3. Module layout (Cargo workspace)

```
holler/
├─ Cargo.toml                 # [workspace]
├─ CLAUDE.md
├─ docs/
└─ crates/
   ├─ holler-core/   # PTT state machine, session pipeline orchestration, events
   ├─ holler-audio/  # cpal capture, rubato resample, (1.5) Silero VAD trim
   ├─ holler-stt/    # trait SttProvider { transcribe(samples) }; LocalWhisper, Deepgram, OpenAI
   ├─ holler-llm/    # trait LlmProvider { cleanup(text, mode) }; Claude, OpenAI, OpenAICompatible(local)
   ├─ holler-inject/ # trait Injector { insert(text) }; clipboard-paste → keystroke → manual
   ├─ holler-store/  # SQLite history + TOML config + keyring secrets
   └─ holler-app/    # binary: winit loop + tray + hotkey; (phase 2) egui settings
```
Provider traits are the key abstraction: local/cloud and Claude/OpenAI/local swap by config without touching the pipeline.

## 4. PTT session state machine (holler-core)
```
Idle --key down--> Recording (cpal stream open, push samples to ring buffer)
Recording --key up--> Processing (close stream; resample; [VAD trim]; STT; [LLM cleanup])
Processing --done--> Injecting (clipboard-paste → fallbacks) --> set clipboard --> log history --> Idle
Recording --Esc--> Cancelled --> Idle (discard)
```
Tray icon reflects state (idle/recording/processing). Debounce auto-repeat `Pressed` events on hold.

## 5. Phased roadmap

### Phase 0 — Spike the risky integration
- `git init`; create workspace + `mimalloc`; release profile (`opt-level="z"`, `lto`, `strip`, `panic="abort"`).
- Main-thread `winit` loop owning `tray-icon` + `global-hotkey`.
- **Prove PTT key down/up fires reliably on macOS AND Windows**, auto-repeat debounced. Log press/release.
- Exit criteria: hold key → "DOWN" logged once; release → "UP" logged once; tray menu quits cleanly.

### Phase 1 — Core local dictation loop (MVP)
- `holler-audio`: cpal capture gated by hold → ring buffer → rubato 16k mono f32.
- `holler-stt`: `LocalWhisper` via `whisper-rs` (`large-v3-turbo`, metal/CPU), transcribe on release.
- `holler-inject`: clipboard-paste primary + keystroke fallback + manual fallback.
- `holler-store`: set clipboard (`arboard`) + SQLite history insert; TOML config; keyring scaffold.
- Config: PTT combo, model tier (auto-detect default), injection mode.
- **Exit criteria: hold key, talk, release → text appears at cursor, on clipboard, in history — fully offline.**

### Phase 1.5 — VAD + feedback
- Silero VAD silence trimming; tray icon state (idle/recording/processing); optional minimal overlay.

### Phase 2 — Cloud STT + LLM cleanup + egui settings
- `SttProvider`: Deepgram (batch-on-release via HTTP; WS only if live words wanted), OpenAI (`gpt-4o-mini-transcribe`).
- `LlmProvider`: Claude, OpenAI, OpenAICompatible(local Ollama). "Modes": raw / cleaned / formatted prompts.
- egui settings window via manual `egui-winit`+`egui-wgpu` integration; history search UI; provider/key management (keyring).

### Phase 3 — TTS read-back + distribution hardening
- `TtsProvider`: OpenAI / Deepgram Aura. Separate hotkey reads current selection/clipboard aloud.
- macOS code-signing + notarization (stable Developer ID), Accessibility onboarding flow; Windows installer.

## 6. Memory-efficiency tactics
- No resident WebView (native tray); GUI window created on demand, dropped on close.
- Audio stream + VAD/ONNX + whisper model loaded only during a PTT session; release after (or after inactivity timeout).
- Event-driven hotkeys (no polling — avoid `device_query`).
- `mimalloc`; release profile strip/LTO; mmap whisper model; auto-select model size by detected HW.
- Benchmark RSS on Windows WebView2 isn't relevant (no WebView); measure idle RSS of the tray process.

## 7. Cross-platform gotchas to remember (from research/04)
- macOS: keystroke/paste injection needs **Accessibility** permission; secure-input fields (passwords) block both paste and keystrokes (OS-enforced) → manual fallback. Re-signing invalidates TCC grant; CGEvent taps silently disable on inconsistent signing → use stable Developer ID.
- Windows: `SendInput`+`KEYEVENTF_UNICODE`; **UIPI** blocks injecting into elevated windows from a normal process (silent failure) → run medium-IL, manual fallback for admin targets.
- Clipboard restore after paste is **racy** — use ~80–120ms delay; keep clipboard ops on one thread (Windows).
- Clipboard history managers (Win+V, Maccy) capture transcripts even after restore — privacy note; arboard can't set Windows history-exclusion formats (needs raw Win32).
