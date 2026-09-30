# AYGENT Core

_The AI agent you actually own._ A free, open-source, native desktop app
(macOS · Windows · Linux) for working with AI agents that live in folders
you choose, run on keys and models you control, and are configured entirely
from a real UI — no terminal, no account, no subscription.

## What it is

AYGENT Core is a desktop harness for agentic AI work: create agents, point
each one at a folder, talk to it in Chat, and let it read, write, organize,
remember, and run on a schedule — inside boundaries the operating system
enforces, not just promises.

Out of the box: **Chat** · **Agents** (as many as you want) · **Tools** +
**Skills** (extend what agents can do) · **Scheduler** (interval + cron jobs)
· **Save Points** (rewind any change) · **Vault-native memory** (agents
remember durable facts in markdown you keep) · **Connections** (GitHub,
Notion, and more, per-agent on/off switches) · **MCP** servers · **Local
models** (GGUF + Apple-silicon MLX, fully offline).

## How it's different from other harnesses

- **An app, not a CLI or SDK.** If you can use settings screens, you can use
  AYGENT. Nothing to `pip install`, no API to wire, no YAML to write.
- **You own the keys and the data.** Bring your own provider keys
  (Anthropic, OpenAI, OpenRouter, Muse) — they're stored in the OS
  keychain and injected per call on the Rust side, so the UI and the agent
  never see them. Or skip the cloud entirely with downloaded local models.
- **Jailed by the OS, not by convention.** Each agent can only touch its own
  folder — enforced by Seatbelt on macOS and equivalent confinement
  elsewhere. Shell access is a separate, per-agent, explicitly-granted
  switch, not a default.
- **Agents, plural, with memory.** Most harnesses give you one chat box.
  AYGENT gives you a roster of agents with their own folders, memories,
  schedules, and histories — plus Save Points, so every change is rewindable.
- **Free and open (Apache-2.0).** No seat, no meter, no phone-home. Your
  folder plus your keychain is your whole setup — moving machines means
  moving a folder.

AYGENT Pro (paid, closed-source) adds hosted subscription credits with
metered Intern/Workhorse/Expert tiers, a Wallet screen, and the Dashboard /
Sparks / Video panels on top of this exact core: https://masonlee.build

## Download (no building)

Get the latest release from
[Releases](https://github.com/masonleetompkins/aygent-core/releases):

| OS | File | Notes |
|----|------|-------|
| macOS (Apple Silicon) | `AYGENT-1.1.0-macOS-arm64.dmg` | Open, drag AYGENT to Applications. Unsigned dev build — on first launch, right-click → Open (once), then launch normally. |
| Linux (x86_64) | `AYGENT-1.1.0-Linux.AppImage` | `chmod +x` the file, then run it. No install needed. |
| Windows | — | No packaged build for 1.1.0 yet — build from source below (one command section). Check Releases for a newer version first. |

Intel Macs: no prebuilt binary — build from source below (`cargo tauri build --bundles dmg`).

First run (all platforms):

1. Open AYGENT. Pick an **agent folder** when asked — the agent will only
   ever be able to touch files inside it.
2. Add a brain in **Settings → Providers**: paste one provider key
   (it goes straight to the OS keychain), or go to **Local Models** and
   download a free model to run fully offline.
3. Open **Chat** and put it to work.

Updates are the same flow: download the new release, install over the old
one. Your agents, memory, and settings live in your agent folders — updating
never touches them.

## Build from source

Prereqs (all platforms): Rust via [rustup](https://rustup.rs) and Node 24+.

**macOS**

```bash
xcode-select --install   # command-line tools (compiler + SDK)
```

**Linux (Debian/Ubuntu)**

```bash
sudo apt-get update && sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf
```

Other distros need the same WebKitGTK/GTK3 pieces under their own package
names (`webkit2gtk`, `gtk3`, `libayatana-appindicator`, `librsvg`, `patchelf`).

**Windows**

- Install the MSVC Build Tools (via
  [Build Tools for Visual Studio](https://visualstudio.microsoft.com/downloads/),
  check "Desktop development with C++") and add the Rust MSVC target:
  `rustup target add x86_64-pc-windows-msvc`.
- WebView2 ships with Windows 10/11 — nothing to install.

Build:

```bash
git clone https://github.com/masonleetompkins/aygent-core
cd aygent-core

# UI + daemon bundles
npm ci --prefix ui && npm run build --prefix ui
npm ci --prefix daemon && npm run build --prefix daemon

# Checks (what CI enforces)
./ui/node_modules/.bin/tsc --noEmit          # UI types
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --lib

# App bundle (unsigned dev build) — lands in src-tauri/target/release/bundle/
cargo tauri build
```

One platform only? Scope the bundle and skip the rest:

```bash
cargo tauri build --bundles dmg        # macOS
cargo tauri build --bundles appimage   # Linux (or: deb)
cargo tauri build --bundles msi        # Windows
```

On Apple Silicon, append `--target aarch64-apple-darwin` to match the
release DMG above.

Project layout: `src-tauri/` (Rust shell: window, jail broker, providers,
tools, memory) · `daemon/` (Node/TS engine: agent loop, scheduler) · `ui/`
(React + Vite frontend) · `seatbelt/` (macOS sandbox profiles) · `scripts/`
(dev setup, jail diagnostics).

## Security model

- The agent can only touch files inside its folder. Everything else is denied
  by the OS sandbox, not by convention.
- API keys live in the platform keychain and are injected server-side per
  call — the UI and the agent never see them.
- Found a secret, private data, or a safety hole in this tree? Don't open a
  public PR — see `CONTRIBUTING.md` (Security) first.
