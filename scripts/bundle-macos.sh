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
VERSION="0.1.0"
APP_DIR="${APP_NAME}.app"
BIN_SRC="target/release/holler"

echo "==> Building release binary"
cargo build --release -p holler-app

if [[ ! -f "$BIN_SRC" ]]; then
  echo "error: $BIN_SRC not found after build" >&2
  exit 1
fi

echo "==> Assembling $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$BIN_SRC" "$APP_DIR/Contents/MacOS/holler"

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
    <key>LSMinimumSystemVersion</key>   <string>11.0</string>
    <!-- Menubar agent: no Dock icon, no main window. -->
    <key>LSUIElement</key>              <true/>
    <!-- Required for cpal mic capture in a bundled app. -->
    <key>NSMicrophoneUsageDescription</key>
    <string>Holler transcribes your speech to text while you hold the push-to-talk key.</string>
</dict>
</plist>
PLIST

echo "==> Ad-hoc code signing"
codesign --force --sign - --timestamp=none "$APP_DIR/Contents/MacOS/holler"
codesign --force --sign - --timestamp=none "$APP_DIR"
codesign --verify --verbose "$APP_DIR"

echo
echo "Built ./$APP_DIR"
echo "Launch:        open ./$APP_DIR"
echo "Debug w/ logs: ./$APP_DIR/Contents/MacOS/holler"
echo "Set the key:   ./$APP_DIR/Contents/MacOS/holler set-key deepgram <KEY>"
echo
echo "First run: grant Accessibility (System Settings → Privacy & Security →"
echo "Accessibility → enable Holler) and allow Microphone when prompted."
