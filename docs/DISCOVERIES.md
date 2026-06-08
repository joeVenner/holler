# Holler — Discoveries Log

Append hard-learned technical lessons and edge cases here, newest first, using the format below.

```
## [YYYY-MM-DD] Context Update
- **What changed:** ...
- **Why:** ...
- **Impact:** ...
- **Reference:** commit / PR / file
```

---

## [2026-06-08] Context Update — Phase 1 MVP complete: inject + store + config (text reaches the cursor)
- **What changed:** Three new crates close the Phase-1 loop. On a finished transcription the app (main thread) now: copies to the **system clipboard**, records to **SQLite history**, and **injects at the active cursor**. Provider/model/injection-mode come from a **TOML config**.
  - `holler-inject` (enigo 0.6.1): `Paste` mode = OS paste chord (Cmd/Ctrl+V), `Type` mode = `enigo.text()`.
  - `holler-store` (rusqlite 0.40.1, `bundled`): `History` with record/search/recent; pure persistence, in-memory unit tests.
  - `holler-config` (directories 6.0.0 + toml 1.1.2 + serde): `Config` (ptt_key, stt_provider, stt_model, injection_mode) with `#[serde(default)]`, load-or-create in the OS config dir.
- **Versions pinned (`cargo add`):** enigo 0.6.1, arboard 3.6.1, rusqlite 0.40.1, directories 6.0.0, toml 1.1.2.
- **Design lessons:**
  - **enigo is main-thread-only on macOS** (CGEvent/TIS + Accessibility). Injection therefore runs in the `UserEvent::Transcript` handler (main thread), not the worker. Clipboard (arboard, also `!Send`) lives there too. Both are created **lazily** (the injector can pop an Accessibility prompt) — never on the launch path, same rule as the keychain.
  - **"Copy memory" simplifies paste injection to zero clipboard gymnastics.** Holler *wants* the transcript left on the clipboard, so paste = `set clipboard` (which is also the copy feature) → fire Cmd/Ctrl+V. No save/restore dance. A ~60ms settle delay before the paste chord covers clipboard-propagation raciness (acceptable to run on the main loop — no rendering to stall).
  - **Clipboard belongs in the app, not `holler-store`.** Keeping `holler-store` pure SQLite makes it unit-testable without a display (arboard needs one); clipboard is ephemeral main-thread output, co-located with injection.
  - **arboard `Clipboard` and rusqlite `Connection` are both not `Sync`** (Connection is `Send`, not `Sync`) → one per thread, here the main thread.
- **Phase 1 exit criteria MET (pending interactive check):** hold key → speak → release → text at cursor + on clipboard + in history. Needs a human to grant Accessibility + Microphone and confirm injection into a real app.
- **Reference:** `crates/holler-{inject,store,config}/src/lib.rs`, `crates/holler-app/src/main.rs` (`deliver`, `build_provider`).

## [2026-06-08] Context Update — Phase 1 STT: Deepgram provider + keychain-at-launch fix
- **What changed:** Added `DeepgramStt` behind `SttProvider` (providers now split into `openai.rs`/`deepgram.rs`, sharing `encode_wav` + the trait). App picks the provider by stored key (Deepgram preferred); `set-key` accepts `deepgram`. **Fixed a startup hang.**
- **Deepgram batch API (mid-2026, web-verified):** `POST https://api.deepgram.com/v1/listen`, auth header **`Authorization: Token <KEY>`** (the `Token` scheme — `Bearer` 401s; Bearer is only for short-lived `/v1/auth/grant` tokens). Audio is the **raw request body** with `Content-Type: audio/wav` (NOT multipart). Options are query params: default `model=nova-3` + `smart_format=true` (punctuation/caps/number formatting — the big dictation win) + `language=en`. Transcript at `results.channels[0].alternatives[0].transcript` (top-level transcript is already smart-formatted). Errors: `{err_code, err_msg, request_id}` — surface `err_msg` + `request_id`. (`reqwest` 0.13 blocking builder lacks `.query()` with our features → build the query into the URL.)
- **HARD-LEARNED — don't read the keychain on the launch path.** Reading an *existing* key item from a freshly-built (un-ACL'd) unsigned binary triggers a **blocking macOS keychain-access prompt**; doing it in `App::new` froze startup indefinitely (no window/tray, nothing logged) once a key was stored. This is the "rebuild invalidates keychain ACL" issue (research/04) biting at runtime. Fix: **resolve the provider lazily on the PTT-release worker thread** — launch never touches the keychain, and the one-time prompt appears off the main event loop so the tray stays responsive. General rule: keychain (and any potentially-prompting OS call) belongs off the main thread and off the startup path.
- **Reference:** `crates/holler-stt/src/deepgram.rs`, `crates/holler-app/src/main.rs` (`transcribe`, `select_provider`).

## [2026-06-08] Context Update — Phase 1 STT (holler-stt: SttProvider trait + OpenAI, BYOK)
- **What changed:** New `holler-stt` crate: provider-agnostic `SttProvider` trait + `OpenAiStt` (cloud, BYOK). PTT UP now transcribes the captured clip and logs the text. API keys live in the OS keychain (`holler set-key openai <KEY>`). **Phasing change:** cloud STT was slated for Phase 2; pulled into Phase 1 per Yassir ("let the user pick the model; download local OR use API keys, incl. Deepgram"). Local Whisper + Deepgram slot behind the same trait next.
- **Why:** Yassir wants provider-selectable STT (local-or-cloud, BYOK) from the start — consistent with the locked BYOK-traits decision, just earlier. OpenAI chosen as the first impl (fastest path to working text).
- **Versions pinned (`cargo add`, 2026-06-08):** `reqwest 0.13.4` (features `blocking`,`multipart`; default TLS is now **rustls + aws-lc**, no OpenSSL — good cross-platform), `hound 3.5.1`, `serde 1`, `serde_json 1`, **`keyring 3.6.3`** (NOT 4 — see below).
- **Hard-learned lessons:**
  - **keyring 4.0.1 is a trap for a mimalloc app:** its `db-keystore` backend (an unconditional, non-feature-gated dep on desktop) pulls in **`turso`** (a SQLite engine) which registers its **own `#[global_allocator]`** → hard compile error `the #[global_allocator] in this crate conflicts with global allocator in: turso`, plus large binary bloat. Fix: use **keyring 3.x** with `default-features = false, features = ["apple-native","windows-native"]` — lean, compile-time platform backend, no allocator, no turso. keyring 3 needs no runtime store registration (unlike v4's `use_native_store` + `keyring_core::Entry` dance).
  - **Don't block the winit loop on the network.** Transcription runs on a spawned worker thread; the result returns via `EventLoopProxy::send_event(UserEvent::Transcript(..))`. Batch STT needs no tokio — `reqwest::blocking` on the worker thread is simpler and keeps the app sync. (Deviation from the locked "tokio async" stack, justified for the batch path; revisit if streaming is ever added.)
  - **OpenAI transcription API (mid-2026, verified by web agent):** `POST https://api.openai.com/v1/audio/transcriptions`, multipart, `Authorization: Bearer`. Required fields `file` + `model`; `response_format=json` → `{"text": ...}`. Default model **`gpt-4o-mini-transcribe`** (best accuracy/cost for short single-speaker clips; `gpt-4o-transcribe` = higher-accuracy opt-in; `whisper-1` = legacy). WAV accepted directly (no re-encode); 25 MB limit. Errors are `{"error":{"message":...}}` — surface `.message`. Docs moved to `developers.openai.com/api/docs/`.
- **Reference:** `crates/holler-stt/src/{lib.rs,secrets.rs}`, `crates/holler-app/src/main.rs`.

## [2026-06-08] Context Update — Phase 1 audio capture (holler-audio: cpal + rubato)
- **What changed:** New `holler-audio` crate. `AudioCapture::start()` opens the default mic and records; `stop()` returns a `Recording` (16 kHz mono f32 + duration). Wired into the app: PTT DOWN starts capture, PTT UP stops and logs `captured Ns, M samples @ 16kHz`. Pipeline: cpal callback normalises to f32 → downmix to mono → rubato sinc resample to 16 kHz. Builds clean, clippy-clean, 4 unit tests pass; mic-permission'd end-to-end test is interactive (Yassir).
- **Why:** Phase 1 step 1 — get speech into the exact buffer shape Whisper wants, testable offline before pulling in the heavy `whisper-rs` dependency.
- **Versions pinned (resolved live by `cargo add`, 2026-06-08):** `cpal 0.18.1`, `rubato 3.0.0`. NOTE: both are **much newer than my Jan-2026 knowledge** — the ecosystem moved. Always `cargo add` + read the installed source rather than trusting memory or even research-agent prose.
- **Hard-learned API lessons (verified against installed source, not agent output):**
  - **The Phase-1 research agents got the *versions* right but the *APIs* wrong** (hallucinated a plausible cpal `&Data`/`as_slice` callback and an over-complex rubato snippet). Ground-truth check via the crate source in `~/.cargo/registry/src/` is mandatory for anything load-bearing.
  - **cpal 0.18 typed `build_input_stream`** takes `config: StreamConfig` **by value** and a **`FnMut(&[T], &InputCallbackInfo)`** callback (typed slice, NOT `&Data` — that's `build_input_stream_raw`). It returns `Result<Stream, cpal::Error>` (no `BuildStreamError`). `StreamConfig.sample_rate` is now a **bare `u32`** (no `.0`). `StreamConfig` is `Copy` (pass `*config`).
  - **Format-agnostic capture:** match `supported.sample_format()` and monomorphise the stream builder per type; `f32::from_sample(s)` (needs `use cpal::Sample`, bound `f32: FromSample<T>`) normalises every int/float format uniformly.
  - **`Stream` is `!Send`** → `AudioCapture` lives on the main winit thread (where PTT events arrive). Dropping the stream stops the callback **synchronously**, so reading the shared buffer after drop is race-free. Callback uses `try_lock` (never block the realtime audio thread).
  - **rubato 3.0 rewrote its API** around the `audioadapter` crate (re-exported as `rubato::audioadapter_buffers`, so no extra direct dep). One-shot clip resample = `Async::new_sinc(ratio, 1.1, &params, 1024, channels, FixedAsync::Input)` + a `process_into_buffer` loop driven by `Indexing { input_offset, output_offset, partial_len, .. }`, a final partial chunk with `partial_len = Some(left)`, then trim the leading `output_delay()` frames. Copy the idiom from the crate's `examples/process_f64.rs`.
  - **Order matters:** downmix to mono *before* resampling (fewer samples; Whisper wants mono). Sinc (anti-aliased), not polynomial — speech-grade quality keeps STT accuracy up.
- **Reference:** `crates/holler-audio/src/lib.rs`; workflow run `wf_70e5564a-985` (versions ✓, API ✗ — verified manually).

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
