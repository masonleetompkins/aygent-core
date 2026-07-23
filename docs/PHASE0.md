# Phase 0 — Skeleton + prove the HONEST jail (~3 wks)

_The load-bearing gate. Nothing in Phase 1 commits until the escape suite passes on the Mac
under the real Seatbelt profile. Rushing this defeats the entire product._

## Milestones
- [x] **M0.1 ✅ DONE (2026-07-23)** Tauri shell spawns + supervises the Node daemon;
      **WS auth working** (per-session token + Origin — Atlas C6); UI<->daemon handshake live.
      Verified on Mason's Mac: teal pill `● daemon connected ✓ (:61439, 0ms)`. Full three-layer
      stack (Rust shell -> Node daemon -> authenticated WS -> React UI) alive end to end.
- [~] **M0.2** Seatbelt profile denies file+exec for the daemon (Atlas C1) + handle-based
      Rust broker (atomic openat/O_NOFOLLOW/component-compare/nlink/firmlink/case/stale —
      Atlas C2) + folder picker + scoped bookmark.
      ✅ 2026-07-23: broker RESOLUTION LOGIC done + 8/8 Rust escape tests PASS on macOS
         (traversal, absolute, sibling-prefix, symlink mid-path + final, /tmp forbidden,
         legit existing + new files admitted).
      ✅ 2026-07-23 (d): native folder picker (async, non-blocking) + broker scope
         registration + live jail-probe UI, working on Mason's Mac (folder chosen).
      ✅ 2026-07-23 (b): Rust-hosted broker WS (privileged server) + daemon authed client
         + daemon-side jail self-test. VERIFIED END-TO-END on Mason's Mac: after folder pick,
         daemon reports `inside=ADMIT · outside=refuse:Forbidden`. The jailed brain can reach
         inside the chosen folder and is structurally refused /etc/passwd. Two-channel trust
         model (UI<->daemon, daemon<->Rust broker) per BROKER-RPC-DECISION.md.
      ✅ 2026-07-23 (e): REAL SEATBELT JAIL VERIFIED on Mason's Mac. Daemon launches under
         sandbox-exec (deny file+exec by default), boots clean (reads its own daemon/ tree
         incl. node_modules), and runs `[aygent] ws listening` + `daemon up` — while macOS
         refuses it access to everything else. Fixes en route: execvp node (profile
         templating w/ real node bin), node runtime boot paths (dyld cache / dev / sockets /
         iokit), ws EPERM (allow whole daemon/ not just dist/). The broker is now an
         OS-ENFORCED boundary, not a convention. Tooling: scripts/test-jail.sh.
      REMAINING: (c) real openat/O_NOFOLLOW fd layer (atomic open behind handles) +
         security-scoped bookmark persistence + stale handling; then the fs-level TOCTOU/
         hardlink escape tests run against the daemon UNDER this profile = the Phase 0 GATE.
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
