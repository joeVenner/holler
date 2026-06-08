# Push-to-Talk Voice Dictation Apps — 2025/2026 Landscape Survey

Research to inform a new cross-platform (Windows + macOS) Rust desktop dictation app. Scope: apps where you hold/press a hotkey, speak, and transcribed text is injected at the active cursor in whatever app has focus.

## Executive summary / key takeaways for your design

- **The dominant open-source reference architecture is Tauri 2 (Rust backend + web frontend).** Handy, VoiceTypr, Open-Less, Whispering, keyless, whisperi, dictum all use it.
- **Converged crate stack** across the best Rust projects: `cpal` (audio capture) + `rubato` (16 kHz resampling) + `rdev` or `tauri-plugin-global-shortcut` (global hotkey) + `enigo` (keystroke/paste simulation) + `whisper-rs`/`transcribe-rs`/`Candle` (STT) + Silero VAD. **Study Handy first** — cleanest, most popular, MIT-licensed exemplar.
- **Text injection: clipboard-paste is the de-facto standard**, not character-by-character typing. Pattern: save clipboard → set clipboard to transcript → simulate Cmd/Ctrl+V via `enigo` → restore clipboard. Pure keystroke typing is the fallback for terminals/CLIs that block paste. macOS apps often try the Accessibility (AX) API to insert into the focused element first, falling back to clipboard.
- **STT split:** privacy/offline tools run local whisper.cpp or NVIDIA Parakeet; latency/accuracy-focused tools stream to cloud (Deepgram Nova-3, Groq Whisper-v3, OpenAI). Several do both via BYOK.
- **The product differentiator in 2025/26 is LLM post-processing** (filler removal, punctuation, app-aware formatting, "AI mode"), not raw transcription.

---

## Commercial products

### Wispr Flow
- macOS, Windows, iOS, Android. Commercial. ~$15/mo (free tier). $81M raised; reportedly ~$2B valuation talks May 2026 (Bloomberg). Windows app is **Electron** (per Spokenly review). Cloud pipeline: audio → OpenAI subprocessor + fine-tuned Llama cleanup. Hold-to-talk; types into active app; cloud STT. AI auto-editing. SOC 2 Type II + HIPAA.

### superwhisper
- macOS (most complete), Windows, iOS. Commercial, **native** (Apple-Silicon optimized). Free tier; Pro ~$8.49/mo, ~$84.99/yr, or **$249.99 lifetime** (one April-2026 source claimed $849 — unconfirmed). STT: local Whisper (tiny→large-v3 turbo) + **Parakeet V2/V3** on Apple Silicon; cloud BYOK (OpenAI, Deepgram, Groq). 100+ langs. Standout: **"Modes"** — per-config hotkey + model + LLM post-processing prompt + auto-activation rules.

### Aqua Voice
- macOS, Windows, iOS. Commercial cloud. YC W24. 1,000 free words then ~$8/mo. Proprietary **Avalon** STT (technical corpora; claims 97.4% vs Whisper 65.1% on hard technical subset). Standout: **real-time text display as you speak**, launches <50ms, ~450ms insertion latency, custom dictionaries. Developer-oriented.

### Willow Voice
- macOS, Windows, iOS. Commercial. Free (2,000 words/wk); $15/mo or $144/yr; Team $10–12/user/mo. STT undocumented (markets "local" + SOC 2/HIPAA — treat as cloud w/ compliance). Tray app, hotkey, inserts into active app. Targets comms (email/Slack), auto-formats/tone, "AI Mode", writing-style memory.

### MacWhisper
- macOS only. Gumroad €59 (~$69) lifetime Pro + free tier; App Store variant subscription. Local whisper.cpp. **Primarily a file-transcription tool**, dictation mode "bolted on" w/ higher latency. Whisper-UI reference only, not a direct competitor.

### Talon Voice
- macOS, Windows, Linux. Freemium (beta + extra engines $25/mo Patreon). Partially OSS. Own **Wav2Letter Conformer** engine, very low latency. A full **voice-command/accessibility system**, not just dictation. Best reference for low-latency on-device recognition + grammar/command design.

---

## Open-source Rust / Tauri projects (most valuable for architecture)

### ⭐ Handy — github.com/cjpais/Handy (study first)
- CJ Pais. **Tauri 2 (Rust + React/TS). MIT. Win/macOS/Linux.** ~20k stars (Apr 2026), biweekly releases.
- **Verified Cargo.toml deps:**
  - Audio: `cpal = "0.16.0"`, resampling `rubato = "0.16.2"`
  - Hotkeys: `rdev` (git fork `rustdesk-org/rdev`) + `tauri-plugin-global-shortcut = "2.3.1"`
  - Injection: `enigo = "0.6.1"` + `tauri-plugin-clipboard-manager = "2.3.2"` (clipboard-set + enigo-driven paste)
  - STT: `transcribe-rs = "0.3.8"` with per-OS accel features — `whisper-cpp`/`onnx` default, `whisper-metal` (mac), `whisper-vulkan`/`ort-directml` (Win), `whisper-vulkan` (Linux). Wraps **Whisper** and **NVIDIA Parakeet V3** (CPU).
  - VAD: `vad-rs` (git `cjpais/vad-rs`) — **Silero** VAD.
- PTT: hold configurable shortcut → record → release → paste into active app. Overlay window for feedback.
- `transcribe-rs` is CJ Pais's own crate, reused by Whispering — good unified-STT abstraction reference.

### VoiceTypr — github.com/moinulmoin/voicetypr
- Tauri (Rust 55% / TS 39%). AGPL-3.0. macOS 13+ / Win 10-11. "Pay once". Local Whisper, auto language detection, inserts text at cursor ("works in Cursor, Claude Code, ChatGPT, Slack"). Modules: `audio/`, `whisper/`, `commands/`. Exact crates unconfirmed (mirror Handy).

### Open-Less / openless — github.com/Open-Less/openless
- Tauri 2 (Rust + React/TS). MIT. macOS 12+ / Win 10+. "Hold key, speak, release → AI-polished text at cursor."
- **OS-native hotkeys instead of a crate** — macOS `CGEventTap`, Windows `WH_KEYBOARD_LL` low-level hook, Linux `rdev`. PTT explicit state machine (hold=record, release=process, Esc=cancel).
- Audio: native mic → 16 kHz mono Int16 PCM, RMS metering.
- **Injection (`insertion.rs`): macOS AX focused-element API → clipboard + Cmd+V, clipboard-only fallback** — good layered-fallback reference.
- STT: cloud (Volcengine streaming ASR, OpenAI-compatible batch, Apple Speech) + local (Qwen3-ASR 0.6B/1.7B, Windows Foundry Local Whisper). AI polish: 4 output modes.

### keyless — github.com/hate/keyless
- Tauri 2 + React. MIT. macOS/Linux/Windows. **100% local pure-Rust inference via Candle.**
- Audio: `cpal` (~100ms frames, 48k→16k via `rubato`). Hotkey: `rdev` (custom fork, macOS key-name gen disabled to avoid crashes — real gotcha). Injection: **`enigo` char-by-char typing** (needs macOS Accessibility). STT: **Candle running Whisper on-device** — no cloud. Best pure-Rust local-inference reference.

### Whispering — github.com/EpicenterHQ/epicenter
- Braden Wong (YC). Svelte 5 + Tauri. AGPLv3. Cross-platform. ~22MB. Hotkey → transcribe → optional AI transform → copy-paste at cursor; also hands-free VAD mode. Uses **`transcribe-rs`** STT abstraction. Backends: local (Whisper C++, Speaches) + cloud (Groq, OpenAI, ElevenLabs, Deepgram) BYOK. Injection clipboard-paste.

### whisperi — github.com/xarthurx/whisperi
- Tauri 2.x (Rust 61% / TS 35%). MIT. Windows-only. **Injection: native Win32 `SendInput` real keystrokes** — works in CLIs/terminals where paste fails. Tap-to-toggle or PTT. **Cloud-only STT** (Groq Whisper Large v3, OpenAI, Mistral Voxtral Mini, Qwen3 ASR Flash). Documents bug: hotkeys break after RDP, need re-registration.

### FnKey — github.com/evoleinik/fnkey
- **Pure Rust macOS menu-bar app (no Tauri). GPL-3.0.** Hold **Fn**, speak, release → paste. Mic active only while held. **STT: streams to Deepgram Nova-3 over WebSocket** while speaking (Groq Whisper-v3 batch fallback). **Injection: clipboard swap-then-restore.** Custom keyword boosting. Good low-latency streaming-while-holding reference.

### dictum — github.com/nitin27may/dictum
- macOS PTT, Tauri/Rust. `cpal` → 16k mono WAV → OpenAI/Azure OpenAI Whisper API. Hotkey: `tauri-plugin-global-shortcut`. **Injection: clipboard + Cmd+V.** Compact readable reference.

### Vibe — github.com/thewh1teagle/vibe
- Tauri + Rust whisper.cpp bindings, cross-platform, offline, 30x speedup (OpenBLAS/GPU). **File/URL/recording transcription, not PTT-into-focus.** whisper.cpp-in-Tauri reference only.

### Others
- **VoiceInk** (github.com/Beingpax/VoiceInk): macOS-only, **Swift**, whisper.cpp + Parakeet, GPL-3, $39.99 one-time, ~4.3k stars. System-wide insertion, "Power Mode" per-app configs. Leading Swift OSS alternative (UX reference).
- **OpenWhispr** — cross-platform, local (Parakeet/Whisper) + cloud BYOK.
- **nerd-dictation**, **whisper-writer**, **hyprwhspr**, **VoxType** — Linux/Python, injection-technique ideas only.

---

## Cross-cutting design guidance (distilled)

| Concern | Recommended approach |
|---|---|
| App framework | Tauri 2 (ecosystem default) — **but Talker chose native tray for leanness** |
| Audio capture | `cpal` + `rubato` → 16 kHz mono |
| Global hotkey | `rdev` (PTT/mouse, macOS fork caveats) or `tauri-plugin-global-shortcut`/`global-hotkey` (simpler). Open-Less goes OS-native |
| VAD | Silero via `vad-rs` |
| STT local | whisper.cpp via `whisper-rs`/`transcribe-rs`; or Candle (keyless) |
| STT cloud | Deepgram Nova-3 (streaming), Groq, OpenAI — BYOK |
| Text injection | Default clipboard save→set→paste→restore; fallbacks AX (mac) / SendInput (Win); char-typing only where paste blocked |
| Differentiator | LLM post-processing (modes / AI mode) |
| Permissions | macOS Accessibility required for keystroke injection |

## Uncertainty flags
- superwhisper lifetime price ($249.99 vs $849 claim) — unconfirmed.
- Aqua Voice framework — unknown.
- Willow STT backend / "local" claim — undocumented; treat as cloud.
- VoiceTypr / Whispering exact crate lists — verify via Cargo.toml.
- Wispr Flow Electron claim — third-party (Spokenly), not vendor.

## Primary sources
Repos: Handy (+ raw Cargo.toml verified), VoiceTypr, Open-Less, keyless, Whispering/Epicenter, whisperi, FnKey, dictum, Vibe, VoiceInk, primaprashant/awesome-voice-typing. Products: wisprflow.ai, superwhisper.com, aquavoice.com, willowvoice.com, MacWhisper/Gumroad, talonvoice.com; reviews getvoibe.com / spokenly.app; Bloomberg.
