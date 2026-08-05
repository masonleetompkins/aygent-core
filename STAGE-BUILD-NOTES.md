# Stage build — Aug 4, 2026, 18:14 PDT

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings**. All 20 grid checks + tsc + vite passed.

**Commit:** `25da447` — fix(chat): renames persist — titles no longer clobbered by turn-saves

**Artifact:** `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app` (62M)
- binary: `Contents/MacOS/aygent`, 37,337,304 bytes, mtime Aug 4 18:14:29
- default features (WKWebView browser path — same config as the 17:52 build)

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this build contains vs. 17:52
1. **Rename-persistence fix** — conversation titles renamed by the user are no
   longer overwritten by subsequent turn-saves (turn-save now preserves the
   stored title instead of re-deriving it).
2. **Sidebar label** tweak.

## Smoke QA for this build (~3 min)
1. **Rename a conversation**, then send another message in it → title must
   STAY the renamed title after the turn completes (the bug this build fixes).
2. Restart the app → renamed title still present.
3. Check the sidebar label change reads right.

## Regression pass carried from 17:52 (dead-code sweep, if not already done)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Browser** — open a page in the in-app browser, agent read of the page.
4. **Kill a running shell proc** from Pro Mode if handy.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

## Carry-over live QA from 14:01 (still untested)
- Notion/Vercel connect with a REAL credential (validates connector pattern)
- Second GitHub account + nickname radio switch
- Write-toggle blast-radius text read-through
- Tools & Skills tabs origin badges

---

### Previous build — Aug 4, 17:52 PDT (dead-code sweep, `fd50bcb`)
Built clean, 0 warnings (down from 55). −440/+51 lines across 21 files, no
feature work. Deleted legacy JSON stores (conversations.rs, settings.rs, most
of agents.rs) plus orphans; wired `cef_engine::shutdown_engine` at
`RunEvent::Exit`, made `sandbox` a real cargo feature, added a db.rs
debug-assert that migrations end at `SCHEMA_VERSION`.
