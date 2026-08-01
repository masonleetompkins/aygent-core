# RESUME — read this first to get back to where we are

_For future-Cleo (or a fresh session after compaction/gateway restart). Last updated
2026-07-31 by Cleo (running INSIDE production AYGENT on Fable 5). This file is the fast
path back into the AYGENT build._

## What AYGENT is
A native **macOS app**: "the AI agent you actually own." Works out of the box, no terminal,
touches only folders you choose (OS-enforced jail), BYO API keys, real UI. Multi-agent,
Obsidian-vault-native, rewindable checkpoints. **$20 one-time, lifetime updates.**
Stack: **Tauri v2 (Rust shell) + Node/TS daemon + React UI.** Bundle ID `build.masonlee.aygent`.

## Read these, in order
1. `RESUME.md` (this) — the fast orient.
2. `docs/PHASE0.md` — milestone tracker with ✅/remaining, always current.
3. `docs/CONTRACTS.md` — the four frozen contracts (ToolDef, broker RPC, Capability enum,
   folder lock). Don't change without a migration plan.
4. `docs/BROKER-RPC-DECISION.md` — why there are two WS channels.
5. `../BUILD-SPEC.md` — full construction blueprint (parts A–I, build order).
6. `../BUILD-SCOPE-macos.md` — what/why + all locked decisions.
7. `../ATLAS-REVIEW.md` — the red-team; its 6 critical fixes are already folded into the spec.

## Where we are (2026-07-31)
**SELF-HOSTING, FOR REAL.** AYGENT ships as a bundled production Mac app; Cleo runs inside
it (model: Fable 5) and develops AYGENT from within — the loop below. Phase 0 gate passed
2026-07-23 (10/10 Rust escape-suite tests, TOCTOU race hammered 5,000×, Seatbelt jail is an
OS-enforced boundary). Pro Mode, keychain git auth, promote.sh, own-your-agent config all
landed 2026-07-31.

## How we work (the live loop — NEW as of 2026-07-31)
The old OpenClaw/Windows workflow is retired. The loop is now fully inside AYGENT:

- **Cleo runs inside production `/Applications/AYGENT.app`** (Pro Mode), jail =
  `/Users/masontompkins/AYGENT/Cleo/`.
- **Staging checkout lives IN her jail:** `AYGENT-Stage/` (this repo), branch `staging`.
- Loop: Cleo pulls → edits → **builds locally and chases her own errors** (bounded digests:
  exit code, error counts, key `error[...]` lines) → hands Mason a verified staging build →
  **Mason QAs manually** → on his green light, Cleo pushes → Mason replaces
  `/Applications/AYGENT.app` **manually**. The agent never rebuilds or relaunches its own host.
- Self-modification boundary: Cleo only ever touches the staging checkout in her jail.

### Harness gotchas (AYGENT-side, replaces old PowerShell gotchas)
- `shell_run` spawns with a scrubbed PATH — plain `cargo`/`node` fail with "no such file or
  directory." **Wrap builds in `sh -lc "..."`** to get the login PATH. (Papercut; candidate fix.)
- Toolchain on this Mac: Node v24 (`/usr/local/bin/node`), Rust 1.97 (`~/.cargo/bin/cargo`),
  tauri-cli 2.11, git 2.54. GitHub user `masonleetompkins`. Mac user `masontompkins`.

## Next steps (unchanged from Phase 0 close-out)
- **M0.2b** — enforce MCP transport split (`mcp.net` Folder-OK vs `mcp.local-exec` Pro-only).
- **M0.3** — Anthropic end-to-end: key→macOS Keychain→agent loop→one jailed fs tool→streamed.
- **M0.4** — freeze the four contracts (see `docs/CONTRACTS.md`).
- Security-scoped **bookmark persistence** (folder re-picked each launch).
- **Phase 1**: providers, Settings UI (THE WEDGE), scheduler, connections/MCP, vault-native
  memory, checkpoints (shadow-git), multi-agent + shared pools, Telegram, license, notarize/sign.

## Handoff / publish reality
NOT an Xcode app (no `.xcodeproj`). Finish/publish = CLI pipeline: `cargo tauri build` →
`codesign` → `notarytool` → `stapler` → signed `.dmg`. Needs Apple Developer cert + Xcode
*command-line tools*. Must build/sign on the Mac; source authored anywhere.

## History
Old workflow (OpenClaw gateway, Windows authoring, Mason pasting build errors by hand) is
preserved in git history of this file. Full early-session narrative:
`~/.openclaw/workspace/memory/2026-07-23.md`.
