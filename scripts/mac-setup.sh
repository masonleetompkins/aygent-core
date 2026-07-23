#!/usr/bin/env bash
# AYGENT — one-shot Mac setup. Run this first after cloning on the Mac.
set -euo pipefail

echo "==> AYGENT Mac setup"

# Xcode command-line tools (NOT the Xcode app) — Tauri + signing need these
if ! xcode-select -p >/dev/null 2>&1; then
  echo "==> Installing Xcode command-line tools (a GUI prompt will appear)..."
  xcode-select --install || true
  echo "    Re-run this script after the CLT install finishes."
fi

# Rust (Tauri shell)
if ! command -v cargo >/dev/null 2>&1; then
  echo "==> Installing Rust via rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
fi

# Node (daemon + UI) — expect >= 20
if ! command -v node >/dev/null 2>&1; then
  echo "!! Node not found. Install Node 20+ (brew install node) then re-run."
  exit 1
fi

# Tauri CLI + project deps
echo "==> Installing project dependencies..."
npm install
( cd daemon && npm install )
( cd ui && npm install )

# pinned git binary check (checkpoint engine, Atlas C4)
if ! command -v git >/dev/null 2>&1; then
  echo "!! git not found (checkpoint engine needs it). Install via Xcode CLT/brew."
  exit 1
fi

echo "==> Setup complete."
echo "    Live dev:   ./scripts/dev.sh"
echo "    Phase-0 gate: ./scripts/gate.sh   <-- run this to verify the jail"
echo "    Build .dmg: ./scripts/bundle.sh   (needs Apple Developer cert)"
