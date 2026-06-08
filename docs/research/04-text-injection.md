# Cross-Platform Text Injection + Clipboard for a Rust PTT Dictation App (Win + macOS, 2025–2026)

## TL;DR Recommendation
| | macOS | Windows |
|---|---|---|
| **Primary** | Clipboard-paste: save old clipboard → `arboard` set text → **Cmd+V** via `enigo`/CGEvent → restore | Clipboard-paste: save → `arboard` set → **Ctrl+V** via `enigo`/SendInput → restore |
| **Fallback 1** | Direct keystroke (`enigo.text()` → CGEvent + `CGEventKeyboardSetUnicodeString`) for short/paste-blocking apps | Direct keystroke (`enigo.text()` → SendInput + `KEYEVENTF_UNICODE`) |
| **Fallback 2** | Leave on clipboard + "paste manually" (secure-input/elevated) | Same |
| **Permission** | Accessibility (required); Input Monitoring if hotkey listening | None for same-integrity; admin only for elevated windows |

Mirrors **Wispr Flow**: "uses your clipboard temporarily, pasting via Cmd+V or Ctrl+V, then restores previous contents."

> For a *dictation* app the right default is unambiguously **clipboard-paste**, not per-character keystroke simulation. Keystroke is the fallback. Reasoning (speed, Unicode, layout independence) below.

---

## 1. Text Injection Methods

### 1.1 `enigo` (recommended sim crate)
- Cross-platform (Win/macOS/Linux X11/Wayland/libei). Actively maintained (~1.7k stars, 0.5.x line).
- **Unicode:** `Keyboard::text()` enters arbitrary Unicode "regardless of current keyboard layout" (`enigo.text("Hello ❤️")`). **Text must not contain NULL bytes.** Use `key()` for shortcuts (Cmd+V), not `text()`.
- **Per-platform injection:** Windows → `SendInput` + `KEYEVENTF_UNICODE` (no ANSI/Unicode conversion). macOS → CGEvent + `CGEventKeyboardSetUnicodeString` (attach actual char; no keycode table needed for text path). *Avoid the layout-dependent virtual-keycode mapping approach (kulman.sk).*
- **Speed caveat (critical for long output):** `enigo.text()` has per-char cost — issue #38 reports ~40ms/char on Linux, no flush per keydown/keyup. Per-char typing degrades badly for paragraph-length transcripts; dropped/reordered chars under load are a documented class of problem. **Core reason to prefer clipboard-paste.** (40ms is Linux-specific/dated; Win/macOS faster, but qualitative conclusion holds.)

### 1.2 Clipboard-paste (recommended primary)
Mechanism: write transcript to clipboard → synthesize paste shortcut (`key(Cmd/Ctrl down)+key(V)+key(up)` via `enigo.key()`).
- **Pros:** near-instant regardless of length; perfect Unicode/emoji (target's own paste handler); layout-independent; no fast-typing races.
- **Cons / failure modes (confirmed by Wispr Flow troubleshooting):** some apps **block paste** — password managers, banking/EMR, Citrix, MS RDP, VMware Horizon, Outlook Classic, some corporate Slack EMM, some terminals (cmd/PowerShell). Pollutes clipboard unless save/restore. Clipboard-history managers capture transcript even after restore (privacy).
- **Market:** Wispr Flow = clipboard-paste + restore. superwhisper = macOS Accessibility APIs to read context + insert (local-first). Keystroke-first tools (DictaFlow) market that approach for web apps / weird environments where paste is blocked.

### 1.3 macOS specifics
- **CGEvent posting** (`CGEventPost`/`CGEventCreateKeyboardEvent` + `CGEventKeyboardSetUnicodeString`): no special entitlement, but **requires Accessibility** grant (System Settings → Privacy & Security → Accessibility).
- **AXUIElement "insert text"** (`kAXFocusedUIElementAttribute` → set `kAXValue`/`kAXSelectedText`): more semantic but **unreliable across app types**. Electron/Chromium = headline problem (selection-range bugs, `AXManualAccessibility` → `kAXErrorAttributeUnsupported`, incomplete AX tree). Native AppKit behaves; web views/Electron don't. **Do not rely on AX as primary.**
- **Secure Input / password fields:** macOS Secure Event Input blocks synthetic keystrokes AND synthesized Cmd+V in those fields. OS-enforced, not bypassable. Plan hard fallback ("text on clipboard, paste manually").
- **Permissions:** *Accessibility* required to post events. *Input Monitoring* needed if you **listen** for hotkey via event tap (not for posting). Flag both in onboarding. (Exact split depends on hotkey impl — `global-hotkey` registered hotkeys generally avoid this.)
- **Signing/notarization:** TCC grants to **code identity (signature)**, not just bundle ID — re-signing with a different identity = re-grant. Use **stable Developer ID**. Outside-App-Store apps must be **Developer ID-signed + notarized** for Gatekeeper. **CGEvent taps can silently disable on inconsistent signature** (dev from CLI works, fails from Finder after re-sign). Dev/test reset: `sudo tccutil reset Accessibility <bundle-id>`. (CVE-2025-31250: TCC prompt attribution can be spoofed — be precise about bundle identity.)

### 1.4 Windows specifics
- **`SendInput` + `KEYEVENTF_UNICODE`** = correct primitive (bypasses ANSI/layout). WM_CHAR/WM_UNICHAR are window-targeted workarounds; prefer SendInput for "whatever has focus."
- **UIPI / elevated limitation (hard, by design):** "permitted to inject input only into applications at equal or lesser integrity level." A medium-IL app **cannot inject/paste into elevated/admin windows** (Task Manager, elevated terminal, UAC dialogs).
- **Silent failure:** "neither GetLastError nor return value indicates UIPI blocking." Detect indirectly or treat elevated targets as manual-paste fallback.
- **No persistent permission system** — same-integrity injection needs no grant. Only wall is integrity level (don't run elevated just to inject into admin apps — security/UX cost).

---

## 2. Clipboard (Rust)
- **Crate: `arboard`** (1Password org). `Clipboard::new()`, `get_text()`/`set_text()` (UTF-8), `set_html()`, image, `clear()`.
- **Preserve/restore (manual — arboard has no built-in):**
  1. `let prev = clipboard.get_text().ok();`
  2. `clipboard.set_text(transcript)`
  3. synthesize Cmd/Ctrl+V
  4. **wait for paste to land before restoring** — sharp edge. Restore too early → pastes old contents. Fixed ~50–150 ms delay is what most ship; inherently racy.
  5. `if let Some(p) = prev { clipboard.set_text(p); }`
- **Gotchas:** Windows clipboard openable on one thread at a time — keep ops on a consistent thread. macOS NSPasteboard persists fine. Linux X11/Wayland: data lives in owning process; enable `wayland-data-control`.
- **Clipboard-history privacy:** even with restore, OS/3rd-party history (Win+V, Maccy, Raycast) captures transcript. Windows can exclude via `CanIncludeInClipboardHistory`/`ExcludeClipboardContentFromMonitorProcessing` formats — **not exposed by arboard** (needs raw Win32). Known limitation.

---

## 3. Reliability Matrix & Strategy
| Target | Clipboard-paste | Keystroke sim | AX insert (mac) |
|---|---|---|---|
| Native fields (AppKit/Win32) | H | H | M (native only) |
| Browsers (web inputs) | H | M (slow/racey long) | L |
| Electron (VS Code, Slack, Obsidian) | H | M | **L** (AX broken) |
| Terminals (iTerm/Terminal/Win Terminal) | M (paste/bracketed quirks) | M | L |
| cmd/PowerShell, RDP/Citrix/VMware | L (often blocked) | M (sometimes only thing) | L |
| Password / secure-input | **L (OS-blocked)** | **L (OS-blocked)** | L |
| Elevated/admin (Win) | L (UIPI) | L (UIPI) | n/a |

**Layered strategy (per platform):**
1. **Primary — Clipboard-paste** (fast, Unicode-perfect, browsers/Electron). Save→set→paste→restore.
2. **Fallback A — keystroke (`enigo.text()`)** when paste fails (terminal, RDP) or very short inserts.
3. **Fallback B — manual paste** for secure-input (macOS) + elevated (Win UIPI): leave on clipboard + non-blocking "paste manually (Cmd/Ctrl+V)" hint.

Do **not** make AX insertion primary — Electron/web breakage makes it a liability for a general "whatever has focus" tool.

---

## 4. Permissions & First-Run UX
**macOS (TCC):** request Accessibility on first injection; enigo can check/prompt. Best UX: detect `AXIsProcessTrusted()` before first dictation, show explainer, deep-link `x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility`. If hotkey via event tap, also request Input Monitoring. Ship **signed + notarized + stable Developer ID** (re-signing invalidates grant, can silently disable taps). Provide "permissions broken? reset & re-grant" affordance.
**Windows:** no first-run prompt for same-integrity. Run **standard (medium-IL)**. Document can't type into "Run as administrator" apps. Elevated helper only if truly required. SendInput failures silent → verify or fall back to manual hint.

## Key uncertainties
- enigo per-char timing dated/Linux-sourced; qualitative "slow + risky long text" solid, exact Win/macOS ms unverified.
- macOS Input Monitoring requirement depends on final hotkey impl.
- arboard doesn't expose Windows clipboard-history-exclusion formats — needs raw Win32.
- Paste→restore timing inherently racy; safe delay empirical.

## Sources
enigo: github.com/enigo-rs/enigo, docs.rs/enigo Keyboard, Permissions.md, issue #38. Windows: MS Learn SendInput/WM_CHAR/WM_UNICHAR. macOS: blog.kulman.sk, Keyboard Maestro Secure Input wiki, espanso.org secure input, textexpander.com secure input; electron #36337/#37465, macdevelopers.wordpress.com AX text; jano.dev Accessibility, danielraffel.me CGEvent taps & signing, Apple code signing/notarization. Clipboard: github.com/1Password/arboard, docs.rs/arboard. Products: Wispr Flow docs (text-not-pasting, terminals/WSL), getvoibe.com, spokenly.app.
