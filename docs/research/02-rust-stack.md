# Rust Push-to-Talk Dictation App — Core Stack Research (2025–2026)

Cross-platform (Windows + macOS), memory-efficient, background/tray-resident. Confidence flags: **[High]** primary docs/source, **[Medium]** single source/some staleness, **[Low]** inferred.

## TL;DR — Recommended Stack

| Concern | Recommendation | Why |
|---|---|---|
| Shell / tray | **`tray-icon` + `muda` + `winit`** (no web shell); Tauri v2 only if HTML settings UI wanted | Native tray + tiny settings window; avoids resident WebView when idle |
| Settings window | egui (`eframe`) pure-Rust, or Tauri v2 WebView | Spawn on demand, drop on close |
| Global PTT hotkey (keyboard) | **`global-hotkey` v0.8** | Native OS registration, `Pressed`/`Released`, lowest overhead, no polling |
| Global PTT (mouse / raw hold) | **`rdev`** (`listen`/`grab`) | Only realistic mouse-button PTT + true raw key-up/down |
| Audio capture | **`cpal`** | De-facto standard, cross-platform |
| Resample → 16 kHz mono | **`rubato`** (`FftFixedIn`/`SincFixedIn`) | Standard; pair with manual channel downmix |
| VAD | **`voice_activity_detector`** (Silero v5 via `ort`), or `webrtc-vad` for zero-ML footprint | Silero = accuracy; webrtc-vad = tiny |
| Higher-level glue (optional) | **`voice-stream`** (cpal + rubato + Silero) | Fast path; check build health |
| Allocator | **`mimalloc`** | Lower RSS/fragmentation for long-idle process |

Biggest decision: **mouse-button PTT forces `rdev`** (macOS Accessibility burden). Decide early. **Holler decision: keyboard-only → `global-hotkey`.**

---

## 1. Desktop Shell

### Tauri v2 [High facts / Medium memory verdict]
- Supports tray via `tray-icon` feature; macOS accessory app (no Dock icon) via `app.set_activation_policy(ActivationPolicy::Accessory)`; can start with no window, spawn settings on demand. [High]
- **Memory reality check:** marketing "Tauri ~30–50 MB vs Electron ~400 MB" is misleading. Maintainer-acknowledged issue #5889: with shared Chromium memory counted, Tauri-on-WebView2 (Windows) uses *similar or slightly more* RAM than Electron (Postman-like: Tauri 399 MB vs Electron 318 MB). Real advantage on macOS/Linux (WebKit) and **binary size**, but Windows idle-RAM win is overstated. [Medium]
- For a mostly-idle tray app, the WebView dominates cost; opening settings pays full WebView2/WKWebView cost.

### Lighter alternatives
- **`tray-icon` v0.24 standalone** — lightest reliable path. Native tray Win/macOS/Linux, menus via `muda`, integrates with winit/tao via `EventLoopProxy`. macOS: create tray on main thread after loop starts (`StartCause::Init`). No WebView. [High]
- **egui / `eframe`** — pure-Rust immediate-mode, trivial settings window, keep zero GUI resident. [Medium]
- **Slint** — low resource, DSL, steeper curve. **Dioxus** — desktop mode still WebView. **Raw winit + tray-icon** — absolute minimum. [Medium/High]

**Recommendation:** background-first → **`tray-icon` + `muda` + `winit` + on-demand egui settings**. Tauri only if HTML UI matters more than idle RAM (Windows RAM win marginal).

---

## 2. Global Hotkeys / Push-to-Talk

PTT needs reliable global **key-down (start)** + **key-up (stop)**.

### `global-hotkey` v0.8.0 (also `tauri-plugin-global-shortcut`) [High]
- Supports `HotKeyState::Pressed` and `Released` — PTT-capable. Plugin docs show `match event.state() { Pressed =>…, Released =>… }`.
- Can register a bare key: `HotKey::new(None, Code::F8)`.
- Platforms: Windows, macOS, Linux (X11 only). Needs main-thread event loop (macOS) / win32 message loop (Windows). Native registration (`RegisterHotKey`/Carbon) — lightweight, event-driven.
- **Limitations:** **No mouse-button support** (keyboard `Code` only; open feature req plugins-workspace #3378). Key-**release** has edge cases (X11 ordering #39; release added later #4364). Auto-repeat may emit repeated `Pressed` on hold — **debounce**. [Medium]

### `rdev` [Medium — maintenance stale-ish]
- Low-level global `listen`/`grab` of keyboard **and mouse**, true raw up/down — only clean **mouse-button PTT** option.
- **macOS requires Accessibility permission**; without it the callback is **silently never called, no error** — must detect + prompt.
- Linux: X11 only (`listen`). Maintenance backlog (~46 open issues). Verify version at integration.

### `device_query` [Medium] — polling-based, wastes CPU when idle; avoid for low-footprint bg app.
### `inputbot` [Low] — older, uneven maintenance/macOS support; not recommended.

### macOS permission implications [High]
- `global-hotkey` (registered hotkeys) generally **no** Accessibility/Input-Monitoring needed for plain keyboard hotkeys.
- `rdev`/any raw tap needs **Accessibility** (+ effectively Input Monitoring), must be signed/notarized to prompt cleanly, fails silently if denied. Main cost of mouse/raw-hold PTT.

**Recommendation:** keyboard PTT → **`global-hotkey` v0.8** (debounce auto-repeat, defensive `Released`). Mouse/raw → **`rdev`** + first-run macOS Accessibility flow. **Holler: keyboard-only.**

---

## 3. Audio Capture & VAD

### `cpal` [High]
- De-facto cross-platform mic (WASAPI/CoreAudio). **Don't assume 16 kHz mono** — usually 44.1/48 kHz, multi-channel, i16/f32. Must: convert format → f32, downmix to mono, resample. (cpal #788, #753)

### `rubato` [High]
- Standard resampler. `FftFixedIn` (efficient fixed-ratio, good for 48k→16k streaming) or `SincFixedIn` (highest quality). Pattern: capture cb → `resample_audio(data, 48000, 16000)`.

### VAD
- **`voice_activity_detector` v0.2.1** — Silero **v5** via **`ort` 2.0.0-rc.10**. Fixed windows: 512 samples @16k. Default **downloads prebuilt ONNX Runtime** at build; `load-dynamic` controls binary path. Best accuracy. *Footprint: ORT adds MBs + deploy dep; consider static link / bundle.*
- **`webrtc-vad`** — classic, tiny, **no ML/ORT** — lowest footprint, binary speech/silence, weaker in noise.
- **`wavekat-vad` v0.1.16** (Jun 2026) — unified trait over WebRTC/Silero/TEN/FireRed; young (~19 stars), active. Good for A/B backends.
- **`voice-stream` v0.4.0** — wraps cpal + rubato + Silero. **Caveat: v0.4.0 failed docs.rs build (last clean 0.3.0)** — verify before adopting.

**Recommendation:** `cpal` → manual mono downmix → `rubato` (FftFixedIn) → `voice_activity_detector` (Silero v5). Use `webrtc-vad` to avoid bundling ONNX Runtime.

---

## 4. Idle Memory Footprint Best Practices [Medium]
1. **Swap allocator to `mimalloc`** (`#[global_allocator]`) — lower RSS/fragmentation, better return-to-OS than glibc malloc. (jemalloc via `tikv-jemallocator` alternative.)
2. **Don't keep GUI/WebView resident** — create settings lazily, drop on close (largest lever; argues for tray + on-demand egui).
3. **Avoid polling input/audio** when not in PTT — event-driven hotkeys; open cpal stream only while key held, then close.
4. **Lazy-load VAD/ONNX** only on first capture; consider unloading after inactivity (ORT is notable RSS).
5. **Release back to OS** — idle processes hold freed pages; mimalloc/jemalloc help; hint purge after session.
6. **Strip & optimize binary** (`opt-level="z"/"s"`, `lto`, `strip`, `panic="abort"`).
7. Profile RSS with system allocator first (profilers interact poorly with mimalloc/jemalloc), then re-measure.

---

## Key Tradeoffs
- **Native tray vs Tauri v2:** native = smallest idle RAM, all-Rust, more UI hand-work; Tauri = fast HTML UI but WebView whose Windows idle-RAM win is marginal/contested.
- **`global-hotkey` vs `rdev`:** global-hotkey = clean, event-driven, no special macOS perms, keyboard-only; rdev = mouse + raw hold but needs macOS Accessibility (silent fail) + maintenance-sensitive.
- **Silero vs webrtc-vad:** Silero = accuracy in noise but bundles ONNX Runtime; webrtc-vad = tiny/dependency-light, weaker in noise.

## Uncertainty Flags
- Tauri-vs-Electron idle RAM numbers illustrative (older issue) — benchmark actual app on Windows WebView2. [Medium]
- `global-hotkey` key-**release** edge cases — validate `Released` + auto-repeat both OSes. [Medium]
- `rdev` / `voice-stream` maintenance/build health — re-verify at integration. [Medium]
- Exact crate versions move fast — re-check crates.io before pinning (versions above from docs.rs/lib.rs/GitHub mid-2026). [Medium]

## Sources
Tauri vs Electron: tech-insider.org, johal.in, gethopp.app, tauri issue #5889. Tray: v2.tauri.app system-tray, discussions #6038/#10774, docs.rs/tray-icon, muda. GUI survey: boringcactus.com 2025. Hotkeys: github.com/tauri-apps/global-hotkey, docs.rs/global-hotkey, v2.tauri.app global-shortcut, issues #4364/#39, plugins-workspace #3378. rdev: docs.rs/rdev, github.com/Narsil/rdev, lib.rs. Audio: cpal #788/#753, docs.rs/rubato. VAD: docs.rs/voice_activity_detector, docs.rs/voice-stream, github.com/wavekat/wavekat-vad, snakers4/silero-vad. Memory: framequery.com mimalloc, leapcell jemalloc, oneuptime.com Rust memory.

> **Peer-review note:** the mouse-button-PTT requirement is the fork in the road. Keyboard PTT keeps the macOS permission story clean and avoids rdev's silent-failure trap. If mouse is ever required, abstract the hotkey layer behind a trait so global-hotkey/rdev swap without touching the rest. **Holler chose keyboard-only for v1, trait-abstract later.**
