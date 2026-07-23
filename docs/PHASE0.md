# Phase 0 — Skeleton + prove the HONEST jail (~3 wks)

_The load-bearing gate. Nothing in Phase 1 commits until the escape suite passes on the Mac
under the real Seatbelt profile. Rushing this defeats the entire product._

## Milestones
- [~] **M0.1** Tauri shell spawns + supervises the Node daemon; **WS auth from the start**
      (per-session token + Origin, or unix socket — Atlas C6); tokens stream to React chat.
      ✅ 2026-07-23: app compiles + window opens on Mason's Mac (teal AYGENT UI live).
      REMAINING: actually wire the daemon<->UI WS round-trip (token auth handshake).
- [ ] **M0.2** Seatbelt profile denies file+exec for the daemon (Atlas C1) + handle-based
      Rust broker (atomic openat/O_NOFOLLOW/component-compare/nlink/firmlink/case/stale —
      Atlas C2) + folder picker + scoped bookmark.
- [ ] **M0.2b** MCP transport capability split (`mcp.net` vs `mcp.local-exec`) enforced now.
- [ ] **M0.3** Anthropic end-to-end: key→Keychain→loop→one handle-based jailed fs tool→stream.
- [ ] **M0.4** Freeze the four contracts (`ToolDef` + broker RPC + `Capability` enum + folder
      lock) — see `CONTRACTS.md`.
- [ ] **✅ GATE** — escape suite tests 1–14 PASS against the daemon under its real Seatbelt
      profile. Then, and only then, commit to Phase 1.

## Current status (2026-07-23)
Scaffolding laid down on Windows (source is platform-independent):
- `README.md`, `docs/CONTRACTS.md` (four frozen contracts), `docs/PHASE0.md`
- `seatbelt/folder-mode.sb` — the OS jail profile (deny file+exec by default)
- `src-tauri/src/broker.rs` — handle-based path broker skeleton + component-compare + TODO(M0.2)
- `daemon/src/index.ts` — daemon entry with WS-auth fail-closed, no ambient fs
- `daemon/test/escape-suite.md` — the 25-test gate (14 jail + checkpoint + vault + concurrency)
- `scripts/mac-setup.sh` — one-shot Mac bootstrap

## Next (needs the Mac)
1. Flesh `src-tauri` into a real Tauri v2 project (`cargo tauri init` on the Mac).
2. Implement the broker resolution algorithm (the 8 rules in broker.rs TODO).
3. Wire the Seatbelt supervisor: Rust launches the daemon under `folder-mode.sb`.
4. Implement WS auth (token/unix-socket) in `daemon/src/core/ws.ts`.
5. Write the escape suite as runnable tests; `scripts/gate.sh` executes them.
6. Prove tests 1–14 green on macOS → GATE.

## Build-host reality
Source authored anywhere; **compile/run/sign/notarize + the gate run on the Mac.** Handoff
package + `MAC-HANDOFF.md` delivered when the scaffold is code-complete enough to run.
