#!/usr/bin/env bash
# ============================================================================
# AYGENT — CEF bundle injector (Phase 1, macOS / Apple Silicon).
# ============================================================================
#
# Tauri builds AYGENT.app, but it does NOT know about CEF. CEF's macOS multi-
# process model requires the app bundle to ship, in the EXACT layout below, or
# it crashes on launch:
#
#   AYGENT.app/Contents/
#     MacOS/AYGENT                                  <- the browser process
#     Frameworks/
#       Chromium Embedded Framework.framework/      <- the ~170MB CEF framework
#       AYGENT Helper.app/                          <- 5 helper subprocess apps,
#       AYGENT Helper (GPU).app/                       each running aygent_helper
#       AYGENT Helper (Renderer).app/
#       AYGENT Helper (Plugin).app/
#       AYGENT Helper (Alerts).app/
#
# This script performs that injection AFTER `cargo tauri build` (or into a
# `cargo tauri dev` .app), reproducing cef-rs build_util::mac::bundle's layout
# (Contents/Frameworks + one Helper.app per HELPERS entry, LSUIElement=1, the
# helper binary copied in). It then ad-hoc-signs everything (Phase 1: local;
# Developer-ID notarization is Phase 2).
#
# USAGE:
#   # 1. Ensure the CEF framework is exported (same as the Phase-0 spike step 2):
#   export CEF_PATH="$HOME/.local/share/cef"
#   # (run once, from cef-spike/vendor/cef-rs:  cargo run -p export-cef-dir -- --force "$CEF_PATH")
#   # 2. Build aygent WITH the feature (see run notes) so target/ has the binaries.
#   # 3. Run this against the built .app:
#   bash scripts/cef-bundle.sh "/path/to/AYGENT.app" "target/debug"
#
# ARGS:
#   $1 = path to the built AYGENT.app
#   $2 = the cargo target dir holding the built `AYGENT` + `aygent_helper` bins
#        (target/debug for dev, target/release for a release build)
set -euo pipefail

APP="${1:?usage: cef-bundle.sh <AYGENT.app> <target-dir>}"
TARGET="${2:?usage: cef-bundle.sh <AYGENT.app> <target-dir>}"
CEF_PATH="${CEF_PATH:-$HOME/.local/share/cef}"
FRAMEWORK="Chromium Embedded Framework.framework"
EXEC_NAME="AYGENT"                 # must match Contents/MacOS/<exec> + CFBundleExecutable
HELPER_BIN="aygent_helper"         # the [[bin]] in Cargo.toml
IDENTIFIER="build.masonlee.aygent"

CONTENTS="$APP/Contents"
FRAMEWORKS="$CONTENTS/Frameworks"

echo "== [aygent][cef-bundle] app=$APP target=$TARGET cef=$CEF_PATH"

if [ ! -d "$CEF_PATH/$FRAMEWORK" ]; then
  echo "!! CEF framework not found at: $CEF_PATH/$FRAMEWORK"
  echo "   Export it first (from cef-spike/vendor/cef-rs):"
  echo "     cargo run -p export-cef-dir -- --force \"$CEF_PATH\""
  exit 1
fi
if [ ! -f "$TARGET/$HELPER_BIN" ]; then
  echo "!! helper binary not built: $TARGET/$HELPER_BIN"
  echo "   Build with the feature first (see run notes) so the helper bin exists."
  exit 1
fi

mkdir -p "$FRAMEWORKS"

# --- 1. Copy the CEF framework into Contents/Frameworks -----------------------
echo "== copy framework -> Contents/Frameworks/"
rm -rf "$FRAMEWORKS/$FRAMEWORK"
cp -R "$CEF_PATH/$FRAMEWORK" "$FRAMEWORKS/$FRAMEWORK"

# --- 2. Build the 5 helper .apps ----------------------------------------------
# CEF's expected helper suffixes (must be EXACTLY these names).
HELPERS=("" " (GPU)" " (Renderer)" " (Plugin)" " (Alerts)")

make_helper_plist() {
  # $1 = helper exec name (e.g. "AYGENT Helper (GPU)")
  # $2 = Info.plist path
  cat > "$2" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>English</string>
  <key>CFBundleDisplayName</key><string>$1</string>
  <key>CFBundleExecutable</key><string>$1</string>
  <key>CFBundleIdentifier</key><string>$IDENTIFIER.helper</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>$1</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleSignature</key><string>????</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>CFBundleVersion</key><string>0.0.1</string>
  <key>LSFileQuarantineEnabled</key><true/>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><string>1</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSEnvironment</key><dict><key>MallocNanoZone</key><string>0</string></dict>
</dict>
</plist>
PLIST
}

for suffix in "${HELPERS[@]}"; do
  HNAME="$EXEC_NAME Helper$suffix"
  HAPP="$FRAMEWORKS/$HNAME.app"
  echo "== helper: $HNAME.app"
  rm -rf "$HAPP"
  mkdir -p "$HAPP/Contents/MacOS"
  cp "$TARGET/$HELPER_BIN" "$HAPP/Contents/MacOS/$HNAME"
  chmod +x "$HAPP/Contents/MacOS/$HNAME"
  make_helper_plist "$HNAME" "$HAPP/Contents/Info.plist"
done

# --- 3. Ad-hoc sign (Phase 1, local). Sign INNERMOST-first: framework, then --
#        helper apps, then the outer app. Developer-ID + notarization = Phase 2.
echo "== ad-hoc sign (framework, helpers, app)"
ENT="$(dirname "$0")/cef-entitlements.plist"
codesign --force --sign - "$FRAMEWORKS/$FRAMEWORK" 2>/dev/null || true
for suffix in "${HELPERS[@]}"; do
  HAPP="$FRAMEWORKS/$EXEC_NAME Helper$suffix.app"
  if [ -f "$ENT" ]; then
    codesign --force --sign - --entitlements "$ENT" "$HAPP"
  else
    codesign --force --sign - "$HAPP"
  fi
done
if [ -f "$ENT" ]; then
  codesign --force --sign - --entitlements "$ENT" "$APP"
else
  codesign --force --sign - "$APP"
fi

# Clear the quarantine bit so Gatekeeper doesn't block the ad-hoc bundle locally.
xattr -dr com.apple.quarantine "$APP" 2>/dev/null || true

echo "== [aygent][cef-bundle] DONE. Layout:"
ls -1 "$FRAMEWORKS"
echo "== Launch: open \"$APP\"   (or run the binary directly for stderr logs:"
echo "==   \"$APP/Contents/MacOS/$EXEC_NAME\" )"
