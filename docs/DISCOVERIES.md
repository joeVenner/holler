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

## [2026-06-10] Context Update — egui settings window spike (renderer: egui_glow, not egui-wgpu)
- **What changed:** P0 of the GUI backlog: a "Settings…" tray item opens an (intentionally empty) egui window rendered inside the existing single winit loop via **manual `egui-winit` integration** (`egui_glow::EguiGlow`); closing it drops the window, GL context and all egui state (PLAN.md §6). Tray, PTT hotkey and the softbuffer overlay are untouched.
- **Renderer decision — `egui_glow 0.34.3` + `glutin 0.32.3` over `egui-wgpu`/softbuffer:** glow is eframe's own default renderer (battle-tested), drives the system OpenGL driver (WGL on Windows, CGL on macOS — deprecated since 10.14 but still shipping), and costs a fraction of wgpu's dependency tree, compile time and resident memory. A softbuffer-backed egui (CPU-rasterising tessellated meshes) is unsupported community territory — rejected. The integration is isolated in `settings.rs`, so swapping to `egui-wgpu` later (if Apple ever drops GL) touches one file. PLAN.md §34 updated accordingly.
- **Hard-learned lessons:**
  - **Version chain that unifies:** egui/egui-winit/egui_glow **0.34.3** + glutin **0.32.3** + glutin-winit **0.5.0** all agree on winit **0.30.13** — no duplicate-winit split. `egui_glow` re-exports `glow` AND `egui_winit` (via its `winit` feature), so neither needs to be a direct dependency.
  - **egui 0.34 `EguiGlow::run` no longer returns a repaint delay** (older docs/examples disagree). Repaint scheduling now comes exclusively through `Context::set_request_repaint_callback`; we forward `info.delay` over the existing `EventLoopProxy` as `UserEvent::SettingsRepaint` and fold it into `about_to_wait`'s earliest-deadline `ControlFlow::WaitUntil`. Also, 0.34's `Context::run_ui` root `Ui` has **no background fill** — wrap the UI in a `CentralPanel` or you get text painted on the raw GL clear color.
  - **Paint the first frame before `set_visible(true)`.** Don't wait for a `RedrawRequested` on a hidden window (not all platforms deliver one); `SettingsWindow::create` paints synchronously, then shows + focuses. Avoids the white flash (egui#2279).
  - **LSUIElement agents can still take key focus:** winit's `Window::focus_window()` on macOS calls `activateIgnoringOtherApps(true)` + `makeKeyAndOrderFront`, which is exactly what an Accessory-policy app needs to bring the settings window to the front. No activation-policy juggling required (winit 0.30 has no runtime policy setter anyway).
  - `glutin-winit`'s `DisplayBuilder` works fine against a *running* loop's `ActiveEventLoop` — on-demand GL window creation from a tray-menu click is unproblematic. Use `ApiPreference::FallbackEgl` (native WGL/CGL first; egui#2520).
- **Impact:** `holler-app` (new `settings.rs`; `main.rs` menu + event routing + merged wake deadlines), `Cargo.toml`/lock.
- **Reference:** commit `f2765d4`, branch `feature/gui-egui-spike`; `crates/holler-app/src/settings.rs`.

## [2026-06-10] Context Update — Cross-platform hardening + release readiness
- **What changed:** Closed the gaps from a multi-agent review pass making Holler genuinely native on both macOS and Windows and cleanly releasable. Key edits:
  - **API keys: keychain → `secrets.toml`** (`holler-config::secrets`, `0600` on Unix, env-var override). Dropped the `keyring` dependency entirely. Reverses the original DECISIONS.md "keys in keychain" choice (see that file).
  - **P0 launch crashes fixed:** PTT hotkey registration now degrades gracefully instead of `.expect()`-aborting under `panic="abort"` (common on Windows when the combo is taken); added `#![cfg_attr(windows + release, windows_subsystem="windows")]` so the released exe doesn't pop a console.
  - **Tray feedback:** failures (no key, mic, transcription, history) surface on the tray tooltip — a tray agent has no visible stderr.
  - **Cross-platform parity:** Windows overlay skips the taskbar + created inactive; overlay position anchors to `monitor.position()` (multi-monitor); Windows `ms-settings:` launched via `explorer.exe`; platform-tuned clipboard settle (100ms Win / 60ms mac).
  - **Correctness:** VAD keeps the trailing partial speech frame; empty/too-short clips and empty transcripts are guarded (no clipboard clobber / wasted API call); Deepgram `language=multi`; SQLite `busy_timeout`; no `.expect()` on the icon animation hot path.
  - **Release:** workspace version → `0.1.0`; CI builds `--locked`; artifact version derived from the git tag; added `LICENSE-MIT`/`LICENSE-APACHE`.
- **Why:** Dev/verification had been macOS-only; Windows was unproven and several `.expect()`s were latent first-run crashes under the release `panic="abort"` profile.
- **Key lessons:**
  - **`keyring` on an ad-hoc-signed macOS bundle is a trap:** identity changes each rebuild, so the TCC grant never sticks and macOS re-prompts for the login password every run. A `0600` file in the config dir is the pragmatic BYOK fix and removes a platform-divergent code path.
  - Under `panic="abort"`, any `.expect()` reachable on the launch path or a timer callback is a silent hard crash for a tray app (no console). Audit them.
  - winit window positions are in **global desktop space**, not primary-monitor-local — assuming a `(0,0)` origin breaks multi-monitor layouts. Use `MonitorHandle::position()`.
  - `Command::new("ms-settings:…")` fails — a URI is not an executable; launch it via `explorer.exe`.
- **Reference:** branch `feature/cross-platform-hardening`; review workflow `wf_dde89c44-8ad`; `docs/DECISIONS.md`.

## [2026-06-08] Context Update — Phase 1.5 VAD silence trim (webrtc-vad)
- **What changed:** `holler_audio::vad_trim` trims leading/trailing silence from 16 kHz f32 recordings before STT using WebRTC VAD (Quality mode, 30 ms frames). Config gate `vad: bool` (default `true`) in `holler-config`. App logs trimmed seconds; skips VAD when disabled.
- **Crate choice:** `webrtc-vad 0.4.0` over `voice_activity_detector` (Silero v5 via `ort`) — ONNX runtime footprint and download overhead not worth it for PTT use case. `webrtc-vad` compiles in ~3.6 s, has minimal deps, and the C FFI is stable. `!Send` raw pointer stays on the calling thread (function-local VAD instance, no cross-thread sharing).
- **VAD signal for tests:** Pure sine waves can be too spectrally narrow for WebRTC's sub-band GMM classifier. A mix of 300 Hz + 900 Hz at 0.8 amplitude reliably triggers Quality mode detection across two voice sub-bands.
- **Why Quality mode (not Aggressive):** PTT recordings start close to the key-press; aggressive mode risks cutting soft speech starts. Quality mode catches all real voice.
- **Impact:** `holler-audio` (gains `webrtc-vad` dep + `vad_trim` fn + 3 tests), `holler-config` (adds `vad: bool`), `holler-app` (wires trim before `transcribe`).
- **Reference:** commit `e153ce7`, `crates/holler-audio/src/lib.rs`.

## [2026-06-08] Context Update — remappable PTT key from config
- **What changed:** `config.ptt_key` (TOML string, e.g. `"ctrl+alt+space"`) is now parsed at init into a live `HotKey` via a thin wrapper in `holler-config::ptt`. Hardcoded `PTT_MODS`/`PTT_CODE`/`PTT_LABEL` constants removed from `holler-app`. Tray tooltip and ready-log reflect the active combo; bad input falls back to `Ctrl+Alt+Space` with a warning.
- **Why:** User-configurable PTT was spec'd in Phase 1 and is the top backlog item.
- **Implementation lesson:** `global-hotkey 0.8` ships its own `HotKey::from_str` (alias-aware, case-insensitive) — only `meta` and `opt` needed pre-normalisation before delegating to it. `Code` (from `keyboard_types`) implements `Display` producing `"Space"`, `"F8"`, `"KeyA"` — strip the `"Key"` prefix for the label.
- **Impact:** `holler-config` (gains `ptt` module + `global-hotkey` dep), `holler-app/src/main.rs`. 7 new unit tests.
- **Reference:** commit `b5a5cfb`, `crates/holler-config/src/ptt.rs`.

## [2026-06-08] Context Update — animated tray + Deepgram server-side cleanup (LLM pass now optional)
- **What changed:** (1) Tray icon is now **state-aware + animated**: calm blue dot (idle), pulsing red dot (recording), comet-trail spinner (transcribing). (2) Deepgram query gained **`dictation=true`** (+ explicit `punctuate=true`). (3) Tray menu gained "Edit Settings (config.toml)" + "Open History Folder" as a stopgap settings entry point.
- **Deepgram cleanup verdict (web-researched — saves building LLM cleanup prematurely):** Deepgram cleans dictation **server-side, zero extra latency/cost**:
  - `smart_format=true` → punctuation, capitalisation, numbers/dates/currency/emails/URLs.
  - `dictation=true` (needs `punctuate`) → spoken "period"/"comma"/"new line"/"new paragraph" become real marks/newlines. (Caveat: a punctuation word spoken *after a pause* may be transcribed literally — Deepgram open issue.)
  - `filler_words` **defaults to false → "um"/"uh" stripped** automatically (don't set it true).
  - Leave `profanity_filter`/`redact`/`measurements` OFF (they mutate intended text).
  - **What Deepgram CANNOT do (needs an LLM):** remove repetitions/false-starts ("the the", "where we will use where we"), strip fillers beyond um/uh ("like", "you know"), or rephrase. So **LLM cleanup stays optional + off-by-default** (Phase 2 polish toggle) — matches the original locked scope.
- **Tray animation pattern:** programmatic RGBA icons (no committed binaries); animation advances only while non-idle via `ControlFlow::WaitUntil(now + 90ms)` in `about_to_wait`, frame bumped on `StartCause::ResumeTimeReached` in `new_events`. Idle → `ControlFlow::Wait` (full sleep, no polling — preserves the low-power goal). `tray.set_icon(Some(..))` on the main thread per frame; building a 32×32 `Icon::from_rgba` each frame is negligible.
- **Note:** recording pulse is a symmetric sine, so frames 0 and FRAMES/2 are identical by design (caught a naive animation test).
- **Reference:** `crates/holler-app/src/icons.rs`, `crates/holler-stt/src/deepgram.rs`.

## [2026-06-08] Context Update — macOS .app bundle (fixes injection + keychain permission friction)
- **What changed:** `scripts/bundle-macos.sh` builds a double-clickable, ad-hoc-signed `Holler.app` (LSUIElement menubar agent; Info.plist with `NSMicrophoneUsageDescription`). Added a `README.md` quick-start. **Local Whisper deferred** per Yassir (Deepgram is the focus).
- **Why:** Running `cargo run` (a bare, unsigned `target/debug/holler`) failed injection with *"the application does not have the permission to simulate input"* — macOS Accessibility (AXIsProcessTrusted) can't be reliably granted to a transient terminal-launched binary, and the keychain ACL re-prompts because the binary identity changes each build. Both need a **stable app identity**.
- **Lessons:**
  - **Injection (paste/type) needs Accessibility; the mic needs `NSMicrophoneUsageDescription`.** A bundled, signed `.app` is the only way to grant these persistently. The transcript still works without Accessibility — only the synthetic paste is blocked (text lands on the clipboard as the manual fallback, by design).
  - **Bundle essentials:** `LSUIElement=true` (menubar agent, no Dock icon / no main window — matches the tray-only design); `NSMicrophoneUsageDescription` (a bundled app is *killed* on mic access without it, unlike a bare binary); `codesign --sign -` (ad-hoc) to get a valid Designated Requirement.
  - **Ad-hoc signing caveat:** the TCC grant is tied to the binary's cdhash, so **rebuilding invalidates Accessibility** (must re-approve). A self-signed or Developer ID cert keys TCC to the cert identity instead → survives rebuilds. Deferred to Phase 3 (distribution hardening).
  - `open ./Holler.app` launches via LaunchServices (permissions apply, logs go to the system log); run `./Holler.app/Contents/MacOS/holler` directly to see stdout logs under the same bundle identity.
- **Reference:** `scripts/bundle-macos.sh`, `README.md`.

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
