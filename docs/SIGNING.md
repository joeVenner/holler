# macOS code signing & notarization

The release pipeline signs the universal macOS DMG with a **Developer ID**
certificate, notarizes it with Apple, and staples the ticket — so it installs
without a Gatekeeper warning on any Mac, and the Accessibility/Microphone grants
survive app rebuilds (a stable signing identity). Until the secrets below are
set, CI falls back to **ad-hoc** signing (Gatekeeper still warns).

## One-time setup

### 1. Create a "Developer ID Application" certificate
Xcode → Settings → Accounts → your Apple ID → **Manage Certificates** → ＋ →
**Developer ID Application**. (Requires the paid Apple Developer Program.)

Find its full identity string and your Team ID:
```bash
security find-identity -v -p codesigning
# → "Developer ID Application: Your Name (ABCDE12345)"   ← TEAMID is the (...)
```

### 2. Export the cert as a `.p12` and base64 it
Keychain Access → My Certificates → right-click the *Developer ID Application*
cert → **Export** → `.p12` (set an export password). Then:
```bash
base64 -i Certificate.p12 | pbcopy   # base64 now on the clipboard
```

### 3. Create an app-specific password
[appleid.apple.com](https://appleid.apple.com) → Sign-In & Security →
**App-Specific Passwords** → generate one for "Holler notarization".

### 4. Add these GitHub repo Secrets
Repo → Settings → Secrets and variables → **Actions** → New repository secret:

| Secret | Value |
|---|---|
| `MACOS_CERT_P12_BASE64` | the base64 string from step 2 |
| `MACOS_CERT_PASSWORD` | the `.p12` export password |
| `MACOS_SIGN_IDENTITY` | `Developer ID Application: Your Name (TEAMID)` (exact string from step 1) |
| `APPLE_ID` | your Apple ID email |
| `APPLE_TEAM_ID` | the Team ID, e.g. `ABCDE12345` |
| `APPLE_APP_PASSWORD` | the app-specific password from step 3 |

> Never put these in the repo or paste them in chat — repo Secrets only.

### 5. Release
Tag as usual — the next release DMG is signed, notarized, and stapled:
```bash
git tag -a v0.1.1 -m "..." && git push origin v0.1.1
```
Verify a downloaded DMG would pass Gatekeeper on a clean Mac:
```bash
spctl -a -t open --context context:primary-signature -v Holler-macOS-universal.dmg  # → accepted
```

## Signing locally (optional)
On your own Mac with the cert in your keychain:
```bash
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" bash scripts/bundle-macos.sh
```
Notarization then mirrors the CI step (`xcrun notarytool submit … --wait` →
`xcrun stapler staple …`). Entitlements live in `scripts/holler.entitlements`
(hardened runtime needs `com.apple.security.device.audio-input` for the mic).

## Windows
Windows signing (an OV/EV Authenticode cert to avoid SmartScreen) is a separate,
still-deferred task — the current Windows ZIP is unsigned.
