#!/usr/bin/env bash
# PHASE 0 CEF spike bootstrap.
# Clones tauri-apps/cef-rs at an EXACT pinned tag into ./vendor/cef-rs.
# Idempotent: if vendor/cef-rs already exists at the right tag, does nothing.
#
# Pinned: tag export-cef-dir-v151.0.0+151.3.11 (commit bab38d9)
#   cef crate 151.0.0+151.3.11  ->  CEF 151.3.11  ->  Chromium 151.x
set -euo pipefail

REPO="https://github.com/tauri-apps/cef-rs.git"
TAG="export-cef-dir-v151.0.0+151.3.11"
DEST="vendor/cef-rs"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

if [ -d "$DEST/.git" ]; then
  echo "[bootstrap] $DEST already present; fetching + checking out pinned tag..."
  git -C "$DEST" fetch --tags --depth 1 origin "refs/tags/$TAG:refs/tags/$TAG" || true
  git -C "$DEST" checkout -q "tags/$TAG"
else
  echo "[bootstrap] cloning cef-rs @ $TAG (shallow) into $DEST ..."
  mkdir -p vendor
  # Shallow clone of just the tagged commit.
  git clone --depth 1 --branch "$TAG" "$REPO" "$DEST"
fi

echo "[bootstrap] checked out:"
git -C "$DEST" log --oneline -1
echo "[bootstrap] cef workspace version:"
grep -m1 '^version' "$DEST/Cargo.toml" || true
echo "[bootstrap] DONE. Next: cd $DEST && cargo run -p export-cef-dir -- --force \$HOME/.local/share/cef"
