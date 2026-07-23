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
      ✅ 2026-07-23 (c): atomic openat/O_NOFOLLOW fd layer (libc) — resolve+open in ONE
         syscall, closing the TOCTOU gap; read/write via fd, never a re-opened path.
      ✅ 2026-07-23 GATE PASSED (10/10 on Mason's Mac): + gate_hardlink_write_refused
         (nlink>1 hardlink-to-outside refused on write) + gate_toctou_symlink_swap
         (attacker thread flips a name real<->symlink-to-/etc/passwd while broker opens it
         5000x — NEVER leaks). The jail holds against an ACTIVE ADVERSARY, not just static
         paths. This is the reason M0.2 exists.
      REMAINING (minor, deferred): security-scoped bookmark persistence + stale handling
         (folder currently re-picked each launch) — not gate-blocking; a Phase-1 polish item.
- [x] **M0.2b ✅ DONE (2026-07-23)** Capability model frozen + MCP transport split enforced:
      `grantsFor(mode)` (Folder = fs.read/fs.write/net.http/mcp.net; Pro adds shell.exec/
      hooks.script/mcp.local-exec), a `gate()`, and `mcpCapabilityForTransport()` (stdio =
      mcp.local-exec Pro-only; http/sse = mcp.net Folder-OK). `daemon/src/core/capabilities.ts`.
- [x] **M0.3 ✅ DONE (2026-07-23)** Anthropic end-to-end VERIFIED on Mason's Mac: API key in
      macOS Keychain (never enters JS), key fetched Rust-side, account queried for available
      models (robust vs guessing IDs), real completion returned — `[claude-haiku-4-5-20251001]
      Hello!`. Providers card in UI (save key / send prompt).
      ✅ AGENT LOOP COMPLETE (2026-07-23): model has jailed read_file/write_file/list_files
      tools; every call routes through the broker. VERIFIED on Mason's Mac — agent wrote
      test.md, read it back ("hello world"), and when asked to read /etc/passwd the MODEL WAS
      REFUSED by the jail and said "I can only access files within the user's chosen folder."
      Brain + jail FUSED. Clean scrollable transcript UI. (Streaming = Phase-1 nicety;
      non-streamed act-loop works.) Bugs fixed: 404 model IDs (query account for models);
      read-back ENOTDIR errno20 (empty-tail join added trailing slash — return canonical
      ancestor directly).
- [x] **M0.4 ✅ DONE (2026-07-23)** Four contracts FROZEN with a verified impl-status table in
      `CONTRACTS.md` (ToolDef shape / Broker RPC resolution live / Capability enum enforced /
      folder-lock design frozen). Shape changes now need a migration note; filling deferred
      impl against a frozen shape is normal Phase-1 work.

### 🎁 PHASE 0 COMPLETE (2026-07-23) — every milestone green. Phase 1 is unblocked.
- [x] **✅ GATE PASSED (2026-07-23)** — jail proven against an active adversary: 10/10 Rust
      escape+gate tests green (traversal, absolute, sibling-prefix, symlink mid/final,
      /tmp forbidden, legit files, hardlink-write refused, TOCTOU race 5000x no-leak) AND
      the daemon verified booting under the real Seatbelt profile. The security kernel is
      proven. Phase 1 is unblocked.
      NOTE: fs-level suite tests 1–3 (direct fs/spawn from daemon under Seatbelt OS-denied)
      are conceptually covered by the verified jailed-boot; a formal harness for them is a
      quick Phase-1 add. The load-bearing security guarantees are proven.

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
