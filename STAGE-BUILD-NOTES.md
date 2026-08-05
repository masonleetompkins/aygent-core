# Stage build — Aug 4, 2026, 17:52 PDT

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings** (down from 55 — the warning sweep, commit `fd50bcb`). Grid checks + tsc + vite all passed.

**Artifact:** `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app`
- binary: `Contents/MacOS/aygent`, 37,340,472 bytes, mtime Aug 4 17:52
- default features (WKWebView browser path — same config as the Aug 4 14:01 build)

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this build contains vs. 14:01
The dead-code sweep only: −440/+51 lines across 21 files. No feature work.
Deleted legacy JSON stores (conversations.rs, settings.rs, most of agents.rs)
plus assorted orphans; two real fixes wired in:
1. `cef_engine::shutdown_engine` now actually runs at `RunEvent::Exit` (was documented but never called). N/A in this bundle (engine-cef off) but the wiring compiles in.
2. `sandbox` is now a real cargo feature (off = unchanged behavior).
3. `db.rs` debug-asserts migrations end at `SCHEMA_VERSION` (release builds unaffected; dev builds will catch a forgotten bump).

## Smoke QA (regression pass, ~5 min)
1. **Launch** — app opens, agents + conversations all present (the deleted modules were the *legacy JSON* stores; if anything's missing, migration/repo wiring is the suspect. Expected: no change).
2. **One chat turn with tools** — read/write a file in the agent folder (exercises broker after the `AgentScope.agent_id` / `Handle` removal).
3. **Browser** — open a page in the in-app browser, agent read of the page (CdpSession lost its unused `id` field; session create/teardown paths touched).
4. **Kill a running shell proc** from Pro Mode if handy (exec.rs `kill` had the mut/NoScope edits).
5. **Cmd+Q** — quits cleanly, no "stopped unexpectedly", daemon gone from Activity Monitor.

## Carry-over live QA from 14:01 (still untested)
- Notion/Vercel connect with a REAL credential (validates connector pattern)
- Second GitHub account + nickname radio switch
- Write-toggle blast-radius text read-through
- Tools & Skills tabs origin badges
