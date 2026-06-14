# Contributing to Holler

Thanks for your interest in Holler! 🗣️ This is a Rust push-to-talk dictation
app, and we welcome issues, ideas, and pull requests. This guide covers how to
get set up, the conventions we follow, and where help is most wanted.

By participating you agree to keep things friendly and respectful — assume good
intent, be constructive in reviews, and focus on the work.

## Getting started

```bash
git clone https://github.com/joeVenner/holler.git
cd holler
cargo build            # build the whole workspace
cargo test             # run the unit tests (no network/audio needed)
cargo clippy           # lint — keep it warning-clean
cargo run              # run from a terminal to see logs
```

For real use on macOS, build the signed app bundle (`scripts/bundle-macos.sh`)
— unbundled binaries have flaky Accessibility/keychain grants. See the
[README](README.md#development) for details.

### Project layout

Holler is a Cargo workspace of focused crates. Provider **traits** are the key
abstraction — local/cloud backends swap by config without touching the pipeline.

| Crate | Responsibility |
|-------|----------------|
| `holler-audio` | `cpal` capture, downmix + `rubato` resample to 16 kHz mono |
| `holler-stt` | `SttProvider` trait — Deepgram, OpenAI (bring-your-own-key) |
| `holler-tts` | `TtsProvider` trait — Native (macOS), OpenAI, Deepgram read-aloud |
| `holler-inject` | `Injector` trait — clipboard-paste → keystroke fallback |
| `holler-store` | SQLite transcript history |
| `holler-config` | TOML config + `secrets.toml` key handling |
| `holler-app` | the binary: `winit` event loop + tray + global hotkeys + overlays |

## How we work

### Coding conventions

- **Readability over cleverness.** Small, focused changes beat large rewrites.
- **Comment the _why_, not the _what_.** Match the existing doc-comment style:
  module headers explain the design tradeoff; functions note the non-obvious
  constraint (threading, platform quirk, cancellation, etc.).
- **Match the surrounding code** — naming, error handling, and idioms. New
  backends should implement the relevant provider trait rather than special-casing.
- **No panics on the launch/PTT path.** A missing key, busy hotkey, or absent
  overlay must degrade gracefully (log + fall back), never take down the tray loop.
- **Keep it lean.** Avoid resident GPU/WebView contexts and polling loops; this
  is a memory-conscious menubar app.

### Tests & linting

- Add unit tests for new logic, especially pure functions (text processing,
  parsing, selection logic). Tests must not require network or audio hardware —
  inject side-effect results (see the `holler-stt`/`holler-tts` provider tests
  and the fake providers in `speech.rs`).
- `cargo test` and `cargo clippy` must pass clean before you open a PR.

### Commits & pull requests

- **Branch names** are intent-prefixed and kebab-case: `feature/…`, `fix/…`,
  `refactor/…`, `docs/…`, `test/…`, `chore/…`, `perf/…`.
- **Commit messages** follow [Conventional Commits](https://www.conventionalcommits.org):
  `type(scope): summary` — e.g. `feat(tts): add prefetch`, `fix(app): …`.
  Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
  `chore`, `ci`, `build`. Keep commits small and atomic — one logical change each.
- **Lockfile discipline:** bundle `Cargo.lock` changes into the same commit that
  changed `Cargo.toml`.
- **Open a PR against `main`.** It must build, pass tests + clippy, and clear the
  secret scan. `main` is protected and requires a maintainer review before merge.
- Describe **what** changed and **why**, and how you verified it. Screenshots are
  great for overlay/UI changes.

### Security

- **Never commit secrets** — no API keys, `secrets.toml`, or `.env` files. Keys
  belong in `secrets.toml` (gitignored) or `HOLLER_<PROVIDER>_KEY` env vars.
- Found a vulnerability? Please open a private report rather than a public issue.

## Roadmap — where we'd love help 🚧

Good places to dig in, roughly easy → ambitious:

- 🪟 **Windows read-aloud** — implement a Windows `TtsProvider` backend and
  selection capture. Read-aloud is macOS-only today (the cloud `speak()` returns
  `Unsupported` off-macOS). _Medium._
- 🔊 **Cross-platform cloud-TTS playback** — an audio sink so OpenAI/Deepgram
  voices play on Windows/Linux (playback is macOS/AVFoundation only right now).
  Pairs naturally with the item above. _Medium._
- 🧠 **Offline local STT** — a `LocalWhisper` `SttProvider` using `whisper-rs`
  (`large-v3-turbo`, download-on-demand, mmap'd) so dictation works with no
  network and no key. The flagship offline feature. _Ambitious._
- ✨ **LLM cleanup modes** — an optional post-transcription pass (raw / cleaned /
  formatted) behind a new `LlmProvider` trait (Claude / OpenAI / local Ollama),
  selectable per mode. _Ambitious._
- 🐧 **Linux support** — audio capture, text injection, and overlay backends for
  X11/Wayland. _Ambitious._

Smaller wins are welcome too: documentation, additional voices/models, settings
UX polish, and overlay/layout refinements. If you're planning something larger,
open an issue first so we can align on the approach before you write code.

## License

By contributing, you agree your contributions are dual-licensed under
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), matching the project.
