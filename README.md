# Holler

A cross-platform, memory-efficient **push-to-talk dictation** app — a
walkie-talkie for your agents. Hold a key, speak; on release your speech is
transcribed, **injected at the cursor**, copied to the clipboard, and saved to
a searchable local history.

> Status: Phase 1 MVP. Cloud STT working (Deepgram / OpenAI, bring-your-own-key).
> See `docs/PLAN.md` for the roadmap and `docs/DECISIONS.md` for locked choices.

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

### Configuration

`config.toml` lives in the OS config dir, created on first run:

- **macOS:** `~/Library/Application Support/com.Holler.Holler/config.toml`
- **Windows:** `%APPDATA%\Holler\Holler\config\config.toml`

Edit it from the tray menu → **Edit Settings (config.toml)**.

```toml
ptt_key = "ctrl+alt+space"   # the hold-to-talk combo; takes effect on relaunch
stt_provider = "deepgram"    # "deepgram" or "openai"
stt_model = ""               # empty = provider default (deepgram: nova-3)
injection_mode = "paste"     # "paste" (default) or "type" (for apps that block paste)
vad = true                   # trim leading/trailing silence before STT; false to disable
```

**API keys** are stored separately in `secrets.toml` (same folder as
`config.toml`, `0600` on macOS/Linux) — never in `config.toml`, so your config
stays safe to share. Set them with `holler set-key <deepgram|openai> <KEY>`.
Alternatively, export `HOLLER_DEEPGRAM_KEY` / `HOLLER_OPENAI_KEY` in your
environment; an env var takes precedence over the file (handy for CI/headless).

History lives next to the config as `history.db` (SQLite) — open its folder
from the tray menu → **Open History Folder**.

### Development

```bash
cargo build              # build everything
cargo test               # run unit tests
cargo run                # run from the terminal (see logs); injection/keychain
                         # permissions are flaky for unbundled binaries — use
                         # the .app bundle for real use.
```

For logs while using the bundle’s stable identity, run the inner binary:
`./Holler.app/Contents/MacOS/holler`.

> Note: local/ad-hoc signing ties permissions to the exact binary, so after
> rebuilding you may need to re-approve Accessibility. Release DMGs are signed
> with a **Developer ID** and notarized once the signing secrets are configured
> (see [`docs/SIGNING.md`](docs/SIGNING.md)), which also makes the grant stick.

### Troubleshooting

- **I hold the key, speak, release — and nothing happens.** Most likely no API
  key is set (or the provider was mistyped). Hover the tray icon: failures show
  there. Set a key with `set-key` (above). For full logs, run the binary from a
  terminal — macOS: `./Holler.app/Contents/MacOS/holler`; Windows: run
  `holler.exe` from a terminal (a debug build shows logs).
- **"PTT key … already in use".** Another app owns `Ctrl+Alt+Space`. Change
  `ptt_key` in `config.toml` and relaunch — Holler no longer crashes on this,
  it just disables push-to-talk until you pick a free combo.
- **Text lands on the clipboard but isn't pasted.** macOS: grant Accessibility.
  Windows: the target app is likely running as Administrator (UIPI) — either
  paste manually, or set `injection_mode = "type"`.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option.
