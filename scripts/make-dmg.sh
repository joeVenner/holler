#!/usr/bin/env bash
#
# Build a drag-to-install .dmg from a macOS .app bundle.
#
# The bare `hdiutil create -srcfolder App.app` we used before produced a disk
# image containing ONLY the app — no /Applications target, so there was nothing
# to drag it onto. This stages the app next to an /Applications symlink and lays
# the Finder window out so the mounted volume shows the familiar "drag Holler →
# Applications" arrangement.
#
# Usage:
#   scripts/make-dmg.sh <App.app> <output.dmg> [volume-name]
#
# The Finder layout is best-effort: on a headless CI runner the AppleScript may
# not be able to drive Finder, so a failure there is logged and ignored — the
# /Applications symlink (the part that makes drag-to-install work) is created
# unconditionally and does not depend on Finder.

set -euo pipefail

APP="${1:?usage: make-dmg.sh <App.app> <output.dmg> [volume-name]}"
DMG="${2:?usage: make-dmg.sh <App.app> <output.dmg> [volume-name]}"
APP_BASENAME="$(basename "$APP")"
VOLNAME="${3:-${APP_BASENAME%.app}}"

if [[ ! -d "$APP" ]]; then
  echo "error: app bundle not found: $APP" >&2
  exit 1
fi

STAGE="$(mktemp -d)"
MOUNT_DIR="$(mktemp -d)"
RW_DMG="$(mktemp -u).dmg"
cleanup() {
  # Detach a still-mounted volume (e.g. if osascript bailed mid-layout) so the
  # temp dirs can be removed and no stray /Volumes entry is left behind.
  hdiutil detach "$MOUNT_DIR" >/dev/null 2>&1 || true
  rm -rf "$STAGE" "$MOUNT_DIR" "$RW_DMG"
}
trap cleanup EXIT

echo "==> Staging $APP_BASENAME + /Applications symlink"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

# A read-WRITE image first, so Finder can persist the window layout into its
# .DS_Store; we compress it to read-only UDZO at the end.
echo "==> Creating read-write image"
hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE" -fs HFS+ \
  -format UDRW -ov "$RW_DMG" >/dev/null

echo "==> Laying out the Finder window (best-effort)"
if hdiutil attach "$RW_DMG" -mountpoint "$MOUNT_DIR" -nobrowse -noverify >/dev/null 2>&1; then
  # Drive Finder by the volume name (it mounts under /Volumes/<VOLNAME>), not
  # the -nobrowse mountpoint, since Finder addresses disks by name. Non-fatal.
  osascript - "$VOLNAME" "$APP_BASENAME" <<'APPLESCRIPT' 2>/dev/null || \
    echo "    (Finder layout skipped — headless or automation denied; symlink still present)"
on run argv
  set volName to item 1 of argv
  set appName to item 2 of argv
  tell application "Finder"
    tell disk volName
      open
      set theWindow to container window
      set current view of theWindow to icon view
      set toolbar visible of theWindow to false
      set statusbar visible of theWindow to false
      set the bounds of theWindow to {200, 150, 700, 480}
      set viewOpts to the icon view options of theWindow
      set arrangement of viewOpts to not arranged
      set icon size of viewOpts to 96
      set position of item appName of theWindow to {130, 175}
      set position of item "Applications" of theWindow to {380, 175}
      update without registering applications
      delay 1
      close
    end tell
  end tell
end run
APPLESCRIPT
  sync
  hdiutil detach "$MOUNT_DIR" >/dev/null 2>&1 || true
else
  echo "    (could not attach image for layout — shipping with symlink only)"
fi

echo "==> Compressing to $DMG"
rm -f "$DMG"
hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null

echo "Built $DMG"
