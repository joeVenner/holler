#!/usr/bin/env bash
#
# Package Holler as a double-clickable macOS .app bundle.
#
# Why a bundle? macOS only grants Accessibility (to synthesise the paste/type)
# and remembers keychain "Always Allow" for an app with a STABLE identity —
# i.e. a signed .app, not a bare `cargo run` binary. Bundling also makes the
# app launchable from Finder / Spotlight / `open`.
#
# Usage:
#   scripts/bundle-macos.sh            # release build + bundle + ad-hoc sign
#   open ./Holler.app                  # launch it (menubar agent, no dock icon)
#
# First launch: grant Accessibility (System Settings → Privacy & Security →
# Accessibility → enable Holler) and allow Microphone when prompted.
#
# NOTE: ad-hoc signing ties the grant to the exact binary, so after you rebuild
# you may have to re-approve Accessibility. A real Developer ID cert (Phase 3,
# or a self-signed cert) makes the grant survive rebuilds.

set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Holler"
BUNDLE_ID="com.holler.holler"
# Honor a VERSION from the environment (CI derives it from the git tag);
# fall back to a sane default for local builds. `set -u` requires the `:-`.
VERSION="${VERSION:-0.1.0}"
APP_DIR="${APP_NAME}.app"

# Allow CI to pass a pre-built binary via BINARY_PATH env var.
# If not set, build from source (standard local usage).
if [[ -n "${BINARY_PATH:-}" ]]; then
  BIN_SRC="$BINARY_PATH"
  echo "==> Using pre-built binary: $BIN_SRC"
else
  BIN_SRC="target/release/holler"
  echo "==> Building release binary"
  cargo build --release -p holler-app
fi

if [[ ! -f "$BIN_SRC" ]]; then
  echo "error: binary not found at $BIN_SRC" >&2
  exit 1
fi

echo "==> Assembling $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$BIN_SRC" "$APP_DIR/Contents/MacOS/holler"

# App icon (Finder / DMG / Get Info). Regenerate with scripts/gen-icons.py.
ICON_SRC="$(dirname "$0")/../assets/Holler.icns"
ICON_KEY=""
if [[ -f "$ICON_SRC" ]]; then
  cp "$ICON_SRC" "$APP_DIR/Contents/Resources/Holler.icns"
  ICON_KEY=$'    <key>CFBundleIconFile</key>          <string>Holler</string>'
else
  echo "warning: $ICON_SRC missing — bundling without an icon" >&2
fi

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>             <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>      <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>       <string>${BUNDLE_ID}</string>
    <key>CFBundleExecutable</key>       <string>holler</string>
    <key>CFBundleVersion</key>          <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundlePackageType</key>      <string>APPL</string>
${ICON_KEY}
    <key>LSMinimumSystemVersion</key>   <string>11.0</string>
    <!-- Menubar agent: no Dock icon, no main window. -->
    <key>LSUIElement</key>              <true/>
    <!-- Required for cpal mic capture in a bundled app. -->
    <key>NSMicrophoneUsageDescription</key>
    <string>Holler transcribes your speech to text while you hold the push-to-talk key.</string>
</dict>
</plist>
PLIST

# Sign with a real Developer ID when SIGN_IDENTITY is set (CI/release), so the
# app passes Gatekeeper for everyone and the TCC grant survives rebuilds.
# Otherwise fall back to ad-hoc for local dev (Gatekeeper will warn).
ENTITLEMENTS="$(dirname "$0")/holler.entitlements"
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  echo "==> Developer ID signing + hardened runtime ($SIGN_IDENTITY)"
  # Entitlements + hardened runtime go on the executable; the bundle wrapper is
  # then sealed over it. A secure --timestamp is required for notarization.
  codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGN_IDENTITY" "$APP_DIR/Contents/MacOS/holler"
  codesign --force --options runtime --timestamp \
    --sign "$SIGN_IDENTITY" "$APP_DIR"
else
  echo "==> Ad-hoc code signing (no SIGN_IDENTITY — Gatekeeper will warn on other Macs)"
  codesign --force --sign - --timestamp=none "$APP_DIR/Contents/MacOS/holler"
  codesign --force --sign - --timestamp=none "$APP_DIR"
fi
codesign --verify --verbose "$APP_DIR"

echo
echo "Built ./$APP_DIR"
echo "Launch:        open ./$APP_DIR"
echo "Debug w/ logs: ./$APP_DIR/Contents/MacOS/holler"
echo "Set the key:   ./$APP_DIR/Contents/MacOS/holler set-key deepgram <KEY>"
echo
echo "First run: grant Accessibility (System Settings → Privacy & Security →"
echo "Accessibility → enable Holler) and allow Microphone when prompted."
