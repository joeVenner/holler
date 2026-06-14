<div align="center">

# 🗣️ Holler

**A cross-platform, memory-efficient push-to-talk dictation app — a walkie-talkie for your agents.**

Hold a key, speak; on release your words are transcribed, **injected at the cursor**, copied to the clipboard, and saved to a searchable local history. It also **reads text back aloud** on demand.

[![Release](https://img.shields.io/badge/release-v0.5.0-1c8cff)](https://github.com/joeVenner/holler/releases/latest)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-555)](#quick-start-macos)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-blue)](#license)

[**Website**](https://joevenner.github.io/holler/) · [**Releases**](https://github.com/joeVenner/holler/releases) · [**Contributing**](CONTRIBUTING.md)

</div>

---

## ✨ Features

- 🎙️ **Push-to-talk dictation** — hold `Ctrl+Alt+Space`, speak, release. The transcript is injected at your cursor, copied to the clipboard, and logged to history.
- 🔌 **Bring-your-own-key cloud STT** — pluggable `SttProvider` trait with **Deepgram** (`nova-3`) and **OpenAI** (`gpt-4o-mini-transcribe`) backends, selectable per config.
- 🔊 **Read-aloud (text-to-speech)** — read your selection or the clipboard aloud, with **Replay** and **Stop** controls:
  - **Pluggable backends** — offline **Native** macOS voice (no key) or cloud **OpenAI** / **Deepgram** Aura (BYOK).
  - **Fast & robust on long text** — input is whitespace-cleaned (no more double-spaces from terminal copies), split into sentence-sized batches so the first audio starts quickly, and a huge paste never hits the provider as one slow request.
  - **Lazy prefetch** — cloud backends synthesize the next batch(es) while the current one plays (no gaps), and stop fetching the instant you stop listening.
- 🪟 **Native, lightweight overlays** — a recording pill with a live mic level meter, a read-aloud status popup, and a clipboard toast — all rendered on CPU via `softbuffer` with the embedded **Inter** font and a macOS dark-mode palette. No resident WebView, no idle GPU context.
- ⚙️ **Settings UI** — an on-demand `egui` window to pick STT/TTS providers, voices, injection mode, and rebind hotkeys live (no restart).
- 🗂️ **Searchable local history** — every transcript saved to a local SQLite database you own.
- 🔐 **Privacy-first key handling** — API keys live in a separate `secrets.toml` (`0600` on macOS/Linux), never in your shareable `config.toml`; env-var overrides supported.
- 🪶 **Memory-efficient by design** — `mimalloc`, audio/model loaded only during a session, event-driven hotkeys (no polling), LTO + strip release profile.
- 🖥️ **Cross-platform** — macOS and Windows today, with paste-or-type injection and graceful clipboard fallback when the OS blocks input.

## Quick start (macOS)

```bash
# 1. Build a double-clickable app bundle (release + Info.plist + code sign)
scripts/bundle-macos.sh

# 2. Store your Deepgram API key (one time; written to secrets.toml)
./Holler.app/Contents/MacOS/holler set-key deepgram <YOUR_DEEPGRAM_KEY>

# 3. Launch it (menubar agent — no Dock icon)
open ./Holler.app
```

**First launch grants two permissions:**

1. **Accessibility** — needed to paste/type at the cursor. macOS will refuse
   the first time; open **System Settings → Privacy & Security →
   Accessibility** and enable **Holler**, then relaunch.
2. **Microphone** — allow it when prompted.

Then focus any text field, **hold `Ctrl+Alt+Space`**, speak, and release. The
text appears at your cursor, lands on the clipboard, and is saved to history.
Quit from the menubar icon.

## Quick start (Windows)

```powershell
# 1. Build the release binary and a self-contained ZIP (dist\Holler\)
pwsh scripts\bundle-windows.ps1

# 2. Store your Deepgram API key (one time; written to secrets.toml)
dist\Holler\holler.exe set-key deepgram <YOUR_DEEPGRAM_KEY>

# 3. Run it — a blue dot appears in the system tray (no console window)
dist\Holler\holler.exe
```

No special permissions are needed on Windows. Focus any text field, **hold
`Ctrl+Alt+Space`**, speak, and release. Auto-paste can only fail against apps
running **as Administrator** (Windows UIPI blocks input from a normal process);
the text is always on the clipboard as a fallback. Quit from the tray icon.

> Unsigned builds may trip SmartScreen on first run — choose **More info →
> Run anyway**. (Code signing is a later phase.)

## Read-aloud

Holler can speak text back to you — handy for proofreading a dictation or
listening to an agent's reply.

- **Read selection** — `Ctrl+Alt+R` reads the currently selected text.
- **Read clipboard** — tray menu → **Read Clipboard Aloud** (or `Ctrl+Alt+C`).
- **Stop** — `Ctrl+Alt+.` (period), tray **Stop Speaking**, or the **◼** button on the status popup.
- **Replay** — the **⟲** button on the status popup re-reads the last utterance.

The default voice is the **offline** macOS system voice (no key required); switch
to a cloud voice in the **Read Aloud** settings panel.

## Configuration

`config.toml` lives in the OS config dir, created on first run:

- **macOS:** `~/Library/Application Support/com.Holler.Holler/config.toml`
- **Windows:** `%APPDATA%\Holler\Holler\config\config.toml`

Edit it from the tray menu → **Edit Settings (config.toml)**, or use the
**Settings…** window.

```toml
ptt_key = "ctrl+alt+space"   # the hold-to-talk combo; takes effect on relaunch
stt_provider = "deepgram"    # "deepgram" or "openai"
stt_model = ""               # empty = provider default (deepgram: nova-3)
injection_mode = "paste"     # "paste" (default) or "type" (for apps that block paste)
vad = true                   # trim leading/trailing silence before STT; false to disable

# --- Read-aloud (text-to-speech) ---
tts_backend = "native"               # "native" (offline) | "cloud"/"openai" | "deepgram"
tts_voice = ""                       # backend voice id; empty = backend default
tts_rate = 0                         # words/min; 0 = backend default
tts_read_hotkey = "ctrl+alt+r"       # read the current selection aloud
tts_read_clipboard_hotkey = "ctrl+alt+c"  # read the clipboard aloud
tts_stop_hotkey = "ctrl+alt+period"  # stop speaking
```

**API keys** are stored separately in `secrets.toml` (same folder as
`config.toml`, `0600` on macOS/Linux) — never in `config.toml`, so your config
stays safe to share. Set them with `holler set-key <deepgram|openai> <KEY>`.
Alternatively, export `HOLLER_DEEPGRAM_KEY` / `HOLLER_OPENAI_KEY` in your
environment; an env var takes precedence over the file (handy for CI/headless).

History lives next to the config as `history.db` (SQLite) — open its folder
from the tray menu → **Open History Folder**.

## Development

```bash
cargo build              # build everything
cargo test               # run unit tests
cargo clippy             # lint
cargo run                # run from the terminal (see logs); injection/keychain
                         # permissions are flaky for unbundled binaries — use
                         # the .app bundle for real use.
```

For logs while using the bundle's stable identity, run the inner binary:
`./Holler.app/Contents/MacOS/holler`.

> Note: local/ad-hoc signing ties permissions to the exact binary, so after
> rebuilding you may need to re-approve Accessibility. Release DMGs are signed
> with a **Developer ID** and notarized once the signing secrets are configured,
> which also makes the grant stick.

The workspace is split into focused crates — `holler-audio` (capture/resample),
`holler-stt` (transcription providers), `holler-tts` (read-aloud providers),
`holler-inject` (paste/type), `holler-store` (SQLite history), `holler-config`
(TOML + secrets), and `holler-app` (the winit/tray/hotkey binary). Provider
traits are the key abstraction: local/cloud backends swap by config without
touching the pipeline.

## Roadmap & contributing

Holler is actively developed and **contributions are welcome**. A few areas
we'd love help with:

- 🧠 **Offline local STT** — a `whisper-rs` provider (`large-v3-turbo`, download-on-demand) so dictation works with no network and no key.
- ✨ **LLM cleanup modes** — an optional post-processing pass (raw / cleaned / formatted) behind an `LlmProvider` trait (Claude / OpenAI / local Ollama).
- 🪟 **Windows read-aloud** — a Windows TTS backend and selection capture (read-aloud is macOS-only today).
- 🐧 **Linux support** — audio, injection, and overlay backends.
- 🔊 **Cross-platform cloud-TTS playback** — an audio sink so cloud voices play on Windows/Linux (currently macOS-only via AVFoundation).

See [**CONTRIBUTING.md**](CONTRIBUTING.md) for the full guide, coding
conventions, and the PR process.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option.
