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

# 2. Store your Deepgram API key in the OS keychain (one time)
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

### Configuration

`~/Library/Application Support/Holler/config.toml` (created on first run):

```toml
ptt_key = "ctrl+alt+space"   # informational for now; hotkey is compiled in
stt_provider = "deepgram"    # "deepgram" or "openai"
stt_model = ""               # empty = provider default (deepgram: nova-3)
injection_mode = "paste"     # "paste" (default) or "type" (for apps that block paste)
```

API keys are **never** stored here — they live in the OS keychain.
History lives at `~/Library/Application Support/Holler/history.db` (SQLite).

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

> Note: ad-hoc signing ties permissions to the exact binary, so after
> rebuilding the bundle you may need to re-approve Accessibility. A Developer ID
> signature (Phase 3) makes the grant permanent.
