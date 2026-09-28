# AYGENT Core

_The AI agent you actually own._ A native desktop app: capable agents that work
out of the box, touch only folders you choose, run on your own API keys or
local models, and are configured entirely from a real UI — no terminal.

- Stack: **Tauri v2 (Rust)** shell + **Node/TS** daemon + **React** UI
- Platforms: **macOS · Windows · Linux** (one tree, CI-built for all three)
- Keys: **BYOK** (Anthropic, OpenAI, OpenRouter, Muse) — stored in the OS
  keychain, never leave Rust. Or run **local models** (GGUF, MLX) offline.
- Safety: **Folder jail** per agent (OS-enforced; Seatbelt on macOS) with
  per-agent **Allow Shell Access** for building and running code.
- License: **Apache-2.0** (see `LICENSE`). Contributions welcome — see
  `CONTRIBUTING.md`.

AYGENT Pro (paid, closed-source) adds hosted credits, account sync, and the
Dashboard / Sparks / Video panels on top of this exact core:
https://masonlee.build

## Layout

```
aygent-core/
  src-tauri/     Rust shell: window, jail broker, providers, tools, memory
  daemon/        Node/TS engine: agent loop, scheduler
  ui/            React + Vite frontend (Tauri WebView)
  seatbelt/      macOS sandbox profiles (deny file+exec by default)
  scripts/       dev setup (mac-setup.sh), jail diagnostic (test-jail.sh)
```

## Build from source

Prereqs: Rust (`rustup`), Node 20+, per-OS webview deps (CI installs them;
on Linux: `webkit2gtk` + friends — see `.github/workflows/matrix.yml`).

```bash
# UI + daemon bundles
npm ci --prefix ui && npm run build --prefix ui
npm ci --prefix daemon && npm run build --prefix daemon

# Checks (what CI enforces)
./ui/node_modules/.bin/tsc --noEmit          # UI types
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib

# App bundle (unsigned dev build)
cargo tauri build
```

First launch: pick an agent folder, add a provider key in Settings →
Providers (or pull a local model in Settings → Local Models), open Chat.

## Security model

- The agent can only touch files inside its folder. Everything else is denied
  by the OS sandbox, not by convention.
- API keys live in the platform keychain and are injected server-side per
  call — the UI and the agent never see them.
- Found a secret, private data, or a safety hole in this tree? Don't open a
  public PR — see `CONTRIBUTING.md` (Security) first.
