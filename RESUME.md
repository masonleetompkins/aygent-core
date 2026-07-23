# RESUME — read this first to get back to where we are

_For future-Cleo (or a fresh session after compaction/gateway restart). Last updated
2026-07-23 by Cleo. This file is the fast path back into the AYGENT build._

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

## Where we are (2026-07-23)
**PHASE 0 GATE: PASSED.** ✅ Running three-layer app + a security kernel proven against an
active adversary (10/10 Rust tests, incl. a TOCTOU symlink-swap race hammered 5,000× with zero
leak, and hardlink-write refused). The daemon runs under a real macOS **Seatbelt jail** that
denies it the whole filesystem — the broker is an OS-enforced boundary, not a convention.

Done: M0.1 (shell+daemon+UI+authed WS) · M0.2(a) resolution logic · (d) folder picker + probe
UI · (b) broker RPC bridge · (e) Seatbelt jail · (c) atomic openat/O_NOFOLLOW fd layer · GATE.

## Next steps (to formally close Phase 0, then Phase 1)
- **M0.2b** — enforce MCP transport split (`mcp.net` Folder-OK vs `mcp.local-exec` Pro-only).
- **M0.3** — Anthropic end-to-end: key→macOS Keychain→agent loop→one jailed fs tool→streamed.
- **M0.4** — freeze the four contracts (see `docs/CONTRACTS.md`).
- Small loose end (not gate-blocking): security-scoped **bookmark persistence** (folder is
  re-picked each launch right now).
- **Phase 1** (the fun part): agent loop, providers (Anthropic/OpenAI/OpenRouter/Ollama),
  the delightful Settings UI (THE WEDGE), scheduler, connections/MCP, vault-native memory,
  checkpoints (shadow-git), full multi-agent + shared pools, Telegram channel, license,
  notarize/sign. Honest estimate ~18–24 solo-weeks. Timeline is NOT the constraint.

## How we work (the live loop)
- **Cleo authors code on the Windows workspace** at `projects/agent-app/aygent/`, commits, and
  **pushes to GitHub** (`github.com/masonleetompkins/aygent`, PRIVATE).
- **Mason pulls + builds/runs on his Mac** at `~/Documents/aygent`, pastes output, Cleo fixes.
  Evidence-driven; small steps; give Mason exact commands one at a time (he's not CLI-fluent).
- Verify commands on the Mac: `cargo test 2>/dev/null` (Rust escape suite),
  `cargo tauri dev` (run the app), `./scripts/test-jail.sh` (daemon under Seatbelt by hand).
- Toolchain on Mac (installed): Node v24, Rust 1.97, tauri-cli 2.11, git 2.54. GitHub user
  `masonleetompkins`. Mac user `masontompkins`. Test Agent Folder: `/Users/masontompkins/Aygent`.

## Gotchas (Cleo-side)
- **PowerShell + git:** exec git pushes/commits often print a false "error" (git's normal
  stderr) but SUCCEED. Verify with `git ls-remote origin main` / commit SHA, don't trust the flag.
- `&&`, `||`, `%` break PowerShell inline commands — use temp files or separate exec calls.
- `chmod +x` on Mac vs git pull conflicts — executable bits are now set IN the repo, so avoid
  re-chmod-ing tracked scripts.

## Handoff / publish reality
NOT an Xcode app (no `.xcodeproj`). Finish/publish = CLI pipeline: `cargo tauri build` →
`codesign` → `notarytool` → `stapler` → signed `.dmg`. Needs Apple Developer cert + Xcode
*command-line tools*. Must build/sign on the Mac; source authored anywhere.

## Full session narrative
`~/.openclaw/workspace/memory/2026-07-23.md` — every decision, bug, and fix from build day one.
