# Holler — Decisions Log

Captured during the planning session on **2026-06-08** with Yassir. These are **locked** unless Yassir reopens them. Do not re-litigate during implementation.

## Product scope
- **In v1:** push-to-talk dictation — hold a keyboard combo, speak, release → transcribe → optional LLM cleanup → inject text at the active cursor + copy to clipboard + log to searchable history.
- **Phase 2 add:** LLM post-processing modes (raw / cleaned / formatted) + egui settings UI.
- **Phase 3 add:** TTS read-back on a **separate hotkey** that reads the current selection/clipboard aloud.
- **Out of scope (v1):** voice commands ("new line", "send", etc.).

## Technical decisions
| Topic | Decision | Rationale |
|---|---|---|
| Language/runtime | Rust | Memory efficiency, single binary, cross-platform. |
| STT | Hybrid — local-first `whisper-rs` (whisper.cpp), cloud opt-in (Deepgram/OpenAI) | Privacy + offline + $0/min by default; cloud for weak HW / max accuracy. |
| PTT trigger | Keyboard combo only | Clean `global-hotkey` path; avoids `rdev` + macOS Accessibility silent-failure. Mouse can be added later behind a trait. |
| App shell | Native tray (`tray-icon`) + on-demand `egui` | Leanest idle RAM; no resident WebView. Tauri's Windows RAM win over Electron is marginal anyway. |
| Copy memory | Clipboard set **and** searchable SQLite history | Yassir chose "Both". |
| AI providers | Provider-agnostic/BYOK behind traits — Claude, OpenAI, Deepgram, local OpenAI-compatible (Ollama) | Flexibility. **Key storage REVISED 2026-06-10:** moved from the OS keychain (`keyring`) to a `0600` `secrets.toml` in the config dir (separate from `config.toml`). Ad-hoc-signed macOS bundles change identity each rebuild, so the keychain TCC grant never stuck and macOS re-prompted on every run. `HOLLER_<PROVIDER>_KEY` env var overrides the file. |
| TTS | Deferred to phase 3 | De-risk v1; design traits now, build later. |
| Distribution | Public eventually → plan signing/notarization + permission onboarding from the start | Stable Developer ID; clean macOS TCC flow. |

## Recommendations I made on Yassir's behalf (open to challenge)
- **Defer GUI to phase 2**, ship Phase 1 with a TOML config file. Removes the tray+hotkey+egui main-thread-loop integration risk from the MVP critical path.
- **Cloud STT = batch (send clip on release)**, not streaming. Short utterances gain little from streaming; only add streaming if live words-on-screen is wanted.
- **Layered injection**: clipboard-paste primary, keystroke fallback, manual fallback.

## OPEN QUESTIONS (need Yassir's input; safe defaults assumed for automode)
1. **Local model default tier** — assumed **auto-detect** (`large-v3-turbo` on Apple Silicon/decent GPU, `small` on weak CPU). Confirm or pin a single model. *(Assumed default lets automode proceed.)*
2. **PTT key combo default** — not chosen. Assume a sensible default (e.g. hold `Right Alt`/`F8`) and make it configurable. Confirm preference.
3. **Bundled vs downloaded Whisper model** — ship a small model in the installer, or download on first run? Assume **download-on-first-run with progress UI** (keeps installer small). Confirm.
4. **App/bundle identity** for macOS signing (Developer ID, bundle id) — needed before phase 3. Yassir to provide.

## Reference apps studied (see research/01)
Handy (cjpais — closest open-source exemplar, study its `Cargo.toml`), VoiceTypr, Open-Less, keyless (pure-Rust Candle), Whispering/Epicenter, whisperi (Win SendInput), FnKey (Deepgram streaming), dictum, Vibe, VoiceInk (Swift), Wispr Flow, superwhisper, Aqua, Willow, Talon.
