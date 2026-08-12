# Stage build — Aug 11, 2026, 16:45 PDT — v1.0.1 (rebuild, all 3 commits)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings**. All grid checks + tsc + vite passed. Both bundles produced (.app + notarizable dmg).

**This rebuild folds THREE commits into one v1.0.1 bundle** (all pushed to `origin/staging`):
- `d92f428` — feat(chat): context-window meter + running cost + in-chat compaction (all providers); v1.0.1
- `6b1bd34` — fix: `github_read_file` now full-decodes file contents (no truncation)
- `f11516a` — feat: Muse UI

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app`, Contents mtime **Aug 11 16:45:03**
- dmg: `dmg/AYGENT_1.0.1_aarch64.dmg`, **17,060,110 bytes** (~16.3 MB), mtime **Aug 11 16:45:22**
- default features (WKWebView browser path)

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This build supersedes the 16:40 v1.0.1 build** (`d92f428` only). It carries the same context-window meter work PLUS the `github_read_file` full-decode fix and the Muse UI. Same version number (1.0.1) — the two extra commits are non-breaking additions on top.

## What this rebuild adds vs. 16:40 (v1.0.1, `d92f428`)
1. **`github_read_file` full decode (`6b1bd34`)** — the GitHub read-file tool now returns the **complete** file contents (previously truncated / partial-decode). Agents reading repo files get the whole file.
2. **Muse UI (`f11516a`)** — UI work for the Muse agent path.

## Context-meter work (carried from 16:40)
**Root cause it fixes:** the vanishing long turn — provider stream code discarded each API's usage block, so the app never knew how full the context was and a turn could silently blow past the window.

1. **`pricing.rs` (new)** — cloud model context windows + $/Mtok for Anthropic, OpenAI, OpenRouter, Meta.
2. **`StreamEvent::Usage`** — `{input, output, cache_read, cache_write, context_window}` now parsed for **all** providers:
   - Anthropic (`message_start` / `message_delta`)
   - OpenAI + OpenRouter (`stream_options.include_usage`)
   - Meta (`response.usage`)
   - local llama.cpp (real prompt/generated token counts + true capped window)
3. **New commands** — `chat_model_info` + `conv_compact`.
4. **Chat header UI** — context% bar (amber ≥75%, red ≥90%) + running **$** cost + **Compact** button + at-the-wall warning.
5. **Per-message stamp** — tokens + $ cost for that turn. Local models = context tracking, no cost.
6. **Version bump 1.0.0 → 1.0.1** — sidebar reads it from `tauri.conf.json`.
7. **`github_read_file` full decode** (`6b1bd34`) — returns the WHOLE decoded file with a `# path · sha · size` header (200k cap), not a truncated base64 slice; dir listings + >1MB blobs fall back gracefully. +2 unit tests.
8. **Muse (Meta) selectable in UI** (`f11516a`) — provider dropdown (Agents + Onboarding) + key row (Settings). Completes the already-shipped Muse backend.

## Smoke QA for this build (~8 min)
1. **github_read_file full decode (new)** — on a GitHub-connected agent, read a **large** repo file (a few hundred+ lines) → the agent receives the **entire** file, no truncation or garbled tail.
2. **Muse UI (new)** — exercise the Muse agent path in the UI → renders/behaves as expected, no console errors.
3. **Context meter shows on a cloud turn** — send a chat turn on an Anthropic/OpenAI agent → header shows the **context% bar** and a **running $** figure. More turns → % climbs, cost accumulates.
4. **Amber/red thresholds** — drive toward full (or a big paste) → bar goes **amber at ≥75%**, **red at ≥90%**, at-the-wall warning appears.
5. **Compact button** — hit **Compact** on a long conversation → `conv_compact` runs, context% drops, conversation stays coherent.
6. **Per-message cost stamp** — each cloud message shows its **tokens + $**. Numbers sane.
7. **Local model path** — on a llama.cpp agent, context% tracks with **real** token counts and the **true capped window**; **no $ cost** shown.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Muse tool call** — a tool call with args arrives intact (the earlier fix still holds).
4. **whoami** — "what can you do?" → tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 11, 2026, 16:45 PDT — v1.0.1

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings**. All grid checks + tsc + vite passed. Both bundles produced (.app + notarizable dmg). Versioned **1.0.1**. REBUILT to fold in two previously-uncommitted changes (see below), so the bundle now matches committed history.

**Commits (all pushed to `origin/staging`):**
- `d92f428` — feat(chat): context-window meter + running cost + in-chat compaction (all providers); v1.0.1
- `6b1bd34` — fix(connectors): github_read_file returns the full decoded file, not truncated base64
- `f11516a` — feat(ui): surface Muse (Meta) as a selectable provider

_Note: `6b1bd34` + `f11516a` were already present in the running stage bundle's working tree (that's why your bundle had Muse working) but had never been committed. Now committed + folded into this rebuild._

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app`, **63M**, mtime Aug 11 16:45:03
- dmg: `dmg/AYGENT_1.0.1_aarch64.dmg`, **17,060,110 bytes** (~16.3 MB), mtime Aug 11 16:45:22
- default features (WKWebView browser path)

**Superseded by the 16:45 rebuild** (same version, adds `6b1bd34` github_read_file fix + `f11516a` Muse UI).

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This build supersedes the Aug 11 15:18 build** (`ce74375`, v1.0.0). It carries all prior work (Muse streaming fix, whoami tool, GFM tables) plus the context-window meter and the version bump to 1.0.1.

## What this build adds vs. 15:18 (v1.0.0)
**Root cause it fixes:** the vanishing long turn — provider stream code discarded each API's usage block, so the app never knew how full the context was and a turn could silently blow past the window.

1. **`pricing.rs` (new)** — cloud model context windows + $/Mtok for Anthropic, OpenAI, OpenRouter, Meta.
2. **`StreamEvent::Usage`** — `{input, output, cache_read, cache_write, context_window}` now parsed for **all** providers:
   - Anthropic (`message_start` / `message_delta`)
   - OpenAI + OpenRouter (`stream_options.include_usage`)
   - Meta (`response.usage`)
   - local llama.cpp (real prompt/generated token counts + true capped window)
3. **New commands** — `chat_model_info` + `conv_compact`.
4. **Chat header UI** — context% bar (amber ≥75%, red ≥90%) + running **$** cost + **Compact** button + at-the-wall warning.
5. **Per-message stamp** — tokens + $ cost for that turn. Local models = context tracking, no cost.
6. **Version bump 1.0.0 → 1.0.1** — sidebar reads it from `tauri.conf.json`.

Files touched: `pricing.rs` (new, +124), `provider.rs`, `lib.rs` (+119), `local_provider.rs`, `meta_provider.rs`, `openai_provider.rs`, `Cargo.toml`, `tauri.conf.json`, `ui/package.json`, `ui/src/lib/turns.ts`, plus `CONTEXT-METER-PLAN.md`, `docs/CAPABILITIES.md` (updated to 1.0.1), `connectors.rs`/`descriptors.rs`, and the three provider-dropdown UI files.

## Smoke QA for this build (~6 min)
1. **Context meter shows on a cloud turn** — send a chat turn on an Anthropic/OpenAI agent → header shows the **context% bar** and a **running $** figure. Send more turns → % climbs, cost accumulates.
2. **Amber/red thresholds** — drive a conversation toward full (or a big paste) → bar goes **amber at ≥75%**, **red at ≥90%**, and the at-the-wall warning appears near the limit.
3. **Compact button** — hit **Compact** on a long conversation → `conv_compact` runs, context% drops, conversation stays coherent afterward.
4. **Per-message cost stamp** — each cloud message shows its **tokens + $**. Numbers look sane (not zero, not absurd).
5. **Local model path** — on a llama.cpp agent, context% tracks with **real** token counts and the **true capped window**; **no $ cost** shown (local = free).
6. **OpenRouter / Meta** — one turn each if agents exist → usage parses (context% + cost populate), no crash on the usage block.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Muse tool call** — a tool call with args arrives intact (the 15:18 fix still holds).
4. **whoami** — "what can you do?" → tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 11, 2026, 15:18 PDT

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 0 warnings**. All grid checks + tsc + vite passed. Both bundles produced (.app + notarizable dmg).

**Commit:** `ce74375` — Fix Muse tool calls: capture streamed function_call arguments correctly

**Artifact:** `AYGENT-Stage/src-tauri/target/release/bundle/macos/AYGENT.app`
- binary: `Contents/MacOS/AYGENT`, 38,802,856 bytes, mtime Aug 11 15:17:51
- standalone binary: `target/release/aygent`, 38,802,856 bytes, mtime Aug 11 15:18:12
- dmg: `bundle/dmg/AYGENT_1.0.0_aarch64.dmg`, 17,033,724 bytes, mtime Aug 11 15:18:12
- default features (WKWebView browser path)

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This build supersedes the Aug 10 09:20 build** (`9c6f860`). It carries all prior
work (whoami tool, GFM table rendering + polish) plus the Muse streaming fix.

## What this build fixes vs. 09:20
1. **Muse tool calls now work** — the streamed `function_call` arguments are
   captured/accumulated correctly during the response stream, so tool calls on
   Muse's model path no longer arrive with empty or truncated argument JSON.
   Previously a streamed tool call could fire with malformed/missing args.

## Smoke QA for this build (~5 min)
1. **Muse tool call (the fix)** — trigger an agent on the affected model path to
   make a tool call that takes arguments (e.g. read_file with a path, or
   write_file). Confirm the tool receives the **full, correct arguments** and
   executes — no empty/truncated arg JSON, no "invalid arguments" error.
2. **Multi-arg + longer args** — do a call with several args or a long string
   arg (e.g. write_file with a paragraph of content) → args arrive intact,
   nothing dropped across stream chunks.
3. **Non-tool turn still streams** — a plain chat turn (no tools) still streams
   tokens normally (regression check the streaming path wasn't broken).

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **whoami** — ask "what can you do?" → tools list renders as a clean table.
4. **Browser** — open a page in the in-app browser, agent read of the page.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

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
