#!/usr/bin/env bash
# PHASE 0 CEF spike — one-shot runner (macOS / Apple Silicon).
# Does: bootstrap -> export CEF framework -> set DYLD env -> bundle -> open.
# Renders https://www.google.com/ in a REAL Chromium window (cefsimple default URL).
#
# Prereqs: Xcode CLT, Rust >= 1.85 (edition 2024), cmake (brew install cmake), ~3-4GB disk.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "== [1/5] bootstrap cef-rs @ pinned tag =="
bash bootstrap.sh

cd vendor/cef-rs

echo "== [2/5] export/fetch CEF framework (~hundreds of MB; minutes on first run) =="
cargo run -p export-cef-dir -- --force "$HOME/.local/share/cef"

echo "== [3/5] set macOS env (CEF_PATH + DYLD_FALLBACK_LIBRARY_PATH) =="
export CEF_PATH="$HOME/.local/share/cef"
export DYLD_FALLBACK_LIBRARY_PATH="${DYLD_FALLBACK_LIBRARY_PATH:-}:$CEF_PATH:$CEF_PATH/Chromium Embedded Framework.framework/Libraries"
echo "CEF_PATH=$CEF_PATH"

echo "== [4/5] build + bundle cefsimple.app =="
cargo run --bin bundle-cef-app -- cefsimple -o target/bundle

echo "== [5/5] launch (should open a Chromium window on google.com) =="
# Clear quarantine so Gatekeeper doesn't block the ad-hoc bundle (local spike only).
xattr -dr com.apple.quarantine target/bundle/cefsimple.app 2>/dev/null || true
open target/bundle/cefsimple.app

echo ""
echo "DONE. Verify it's REAL Chromium:"
echo "  \"target/bundle/cefsimple.app/Contents/MacOS/cefsimple\" --url=chrome://version"
echo "  (WebKit/WKWebView cannot render chrome:// pages — this is conclusive.)"
