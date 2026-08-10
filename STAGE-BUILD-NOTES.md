# Stage build — Aug 10, 2026, 09:20 PDT

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings**. All grid checks + tsc + vite passed.

**Commit:** `9c6f860` — polish: markdown table rendering (no mid-token splits inside cells, real header rule, even row heights) + whoami Tools column left-aligned

**Staging pushed:** `e141e66..9c6f860`

**Artifact:** `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app`
- binary: `Contents/MacOS/aygent`, 38,696,408 bytes, mtime Aug 10 09:20:48
- default features (WKWebView browser path)

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This build supersedes the Aug 10 09:10 build** (`e141e66`, 38,696,408 bytes, mtime Aug 10 09:10:26). That build introduced GFM table rendering; this one polishes it.

## What this build adds vs. 09:10
1. **No mid-token splits inside table cells** — cell contents now render via
   `renderInline` with an `inCell` flag, so inline markup (bold, code, links)
   inside a table cell no longer breaks across the pipe boundary.
2. **Real header rule** — the header/body divider renders as a genuine rule
   instead of a rendered separator row.
3. **Even row heights** — table rows are visually uniform.
4. **whoami Tools column left-aligned** — the Tools column in the `whoami`
   output table is left-aligned for readability (was center/default).

## Smoke QA for this build (~5 min)
1. **Cloud agent** — ask "what can you do?" → `whoami` runs and the tools list
   renders as a **real table** with a clean header rule, even rows, and the
   **Tools column left-aligned**.
2. **Inline markup in a cell** — send a table whose cell contains **bold**,
   `code`, or a link → renders intact, no broken pipes / split tokens.
3. **Local-model agent** — ask "who are you / what model are you running?" →
   `whoami` answers with identity + model/provider, table renders clean.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Browser** — open a page in the in-app browser, agent read of the page.
4. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 10, 2026, 09:10 PDT

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings**. All grid checks + tsc + vite passed.

**Commit:** `e141e66` — feat: `whoami` tool (self-config report) + GFM table rendering in Markdown (cloud + local model paths)

**Staging pushed:** `e30d8fa..e141e66`

**Artifact:** `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app`
- binary: `Contents/MacOS/aygent`, 38,696,408 bytes, mtime Aug 10 09:10:26
- default features (WKWebView browser path)

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This build supersedes the Aug 10 09:00 whoami-only build** (`38,713,976` bytes, mtime Aug 10 09:00:28). That earlier bundle had the `whoami` tool but NOT the GFM table rendering. This bundle contains both.

## What this build adds
1. **`whoami` tool** — an agent can report its own configuration: name, model +
   provider, context mode + folder, and every tool / connection / MCP server /
   skill it has, grouped by origin. Read-only.
2. **GFM table rendering** — pipe-delimited markdown tables now render as real
   HTML tables in the chat Markdown component, on BOTH the cloud and local
   model paths (previously tables showed as raw `| ... |` pipes).

## Smoke QA for this build (~5 min)
1. **Cloud agent** — ask "what can you do?" → `whoami` runs and the
   connected-accounts / tools list renders as a **real table**, not raw pipes.
2. **Local-model agent** — ask "who are you / what model are you running?" →
   `whoami` answers with identity + model/provider correctly.
3. **whoami sections read right** — confirm the identity, model, context-mode,
   and origin-grouped tool/connection sections all render cleanly (carry-over
   check the existing sections still look correct).

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Browser** — open a page in the in-app browser, agent read of the page.
4. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 4, 2026, 18:22 PDT

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings**. All 20 grid checks + tsc + vite passed.

**Commit:** `4e3005b` — feat(icon): Dock icon persists via NSWorkspace `setIcon:forFile:` + glow removed (on top of `25da447` rename fix)

**Artifact:** `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app`
- binary: `Contents/MacOS/aygent`, 37,332,712 bytes, mtime Aug 4 18:22:22
- default features (WKWebView browser path — same config as the 18:14 build)

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This build supersedes 18:14.** It contains everything from 18:14 (rename fix + sidebar label) plus the icon work.

## What this build adds vs. 18:14
1. **Icon persistence** — theme icon is now written onto the .app bundle via
   NSWorkspace `setIcon:forFile:`, so Finder and the Dock keep the themed icon
   even when the app isn't running.
2. **Glow removal** — themed Dock icon is now flat (no glow effect).

## Smoke QA for this build (~5 min)

### New: icon persistence + glow
1. Launch the Stage bundle. Change theme in **Settings** → Dock icon updates
   immediately, **flat / no glow**.
2. **Quit** the app → check the .app in **Finder** and (if kept in Dock) the
   Dock tile → icon KEEPS the themed look.
3. Note: on a **fresh bundle** the first icon test requires setting the theme
   once while the app is running (the bundle icon is only written at that
   point). After that it persists.

### Rename fix (from 18:14, retest on this bundle)
4. **Rename a conversation**, then send another message in it → title must
   STAY the renamed title after the turn completes.
5. Restart the app → renamed title still present.
6. Check the sidebar label change reads right.

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

### Previous build — Aug 4, 18:14 PDT (`25da447`)
Rename-persistence fix (turn-save preserves stored title) + sidebar label
tweak. Superseded by 18:22.

### Previous build — Aug 4, 17:52 PDT (dead-code sweep, `fd50bcb`)
Built clean, 0 warnings (down from 55). −440/+51 lines across 21 files, no
feature work. Deleted legacy JSON stores (conversations.rs, settings.rs, most
of agents.rs) plus orphans; wired `cef_engine::shutdown_engine` at
`RunEvent::Exit`, made `sandbox` a real cargo feature, added a db.rs
debug-assert that migrations end at `SCHEMA_VERSION`.
