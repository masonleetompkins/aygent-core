# Stage build — 2026-09-08 ~14:14 PDT — v1.0.16 (media bins + Clean Audio count + timer picker)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, dead-code warnings only (same set). Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `0286d79` — fix(video): Clean Audio counts audible clips only + holds Processing label for the whole run
- `3d92aab` — feat(video): media list view with bins + task_continue timer picker

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41468536 bytes), mtime **Sep 8 14:14**
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg`, **18005722 bytes (~17.2 MB)**, mtime **Sep 8 14:14**

**What changed:**
1. **Media list view** — grid/list toggle in the panel header (persisted). List view has Premiere-style bins: create/rename/delete, collapse per bin, drag assets between bins or Move-to menu, Unfiled + Generated groups. Bins live in `assets.json` (`asset.folder` + `folders[]`), so clips never break. New agent tools `video_media_folder` / `video_media_move`; `video_project` + `video_load` expose folders.
2. **Clean Audio count + Processing hold** — button counts distinct *audible* clips only (linked V+A pair = 1, silent clips skipped): one clip = "1 selected". A local flag holds "Processing…" for the entire run — no spinner flicker between phases.
3. **task_continue timer picker** — parking the turn pops a chat modal: 1 / 3 / 5 / 10 / 15 min + "use suggestion". No answer in 3 min and the agent's `delay_secs` stands, so unattended builds never wedge. Picks queue if several fire at once. Modal mounts above Overlays so it's always clickable.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA (~8 min)
1. **Media** — toggle grid/list; new bin; drag asset in; collapse, switch projects, collapse persists; Generated group shows cleaned proxies separately.
2. **Audio** — select 1 clip → "Clean Audio (1 selected)"; run → "Processing…" holds at full opacity, toast names cleaned clips.
3. **Timer** — trigger a task_continue → modal pops; pick 1 min; wake-up lands in ~1 min with the note attached.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — 2026-09-08 v1.0.16 (Muse image budget + Clean Audio static master + selection cleanup) — 12:06 bundle

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `4845e11` — fix(muse): budget inline images + drop orphan function_call_output · fix(video): Clean Audio static-gain master, stronger fallback denoise, per-selection cleanup, button rename + Processing… state

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (binary 41.3 MB, 12:05)
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg` (~17.9 MB, 12:06)

**What changed:**
1. **Muse `400 Invalid upload request.`** — root cause (probed live against api.meta.ai with the real key): the endpoint rejects any request whose INLINE data-URL images total more than ~18 MB (7× a 1.9 MB screenshot OK, 8× → this 400, 9× → 413 payload_too_large). History resends every image every turn, so a chat with a few full-res screenshots was permanently dead — not a model bug. Fix in `meta_provider.rs`: `images_within_budget` keeps the NEWEST images whole up to `MUSE_IMAGE_BUDGET_BYTES` (12 MB); older ones degrade to a text stub. Model already saw them.
2. **Muse `No function call found for function call output with call_id`** — the headless (task_continue) path sends a last-40 history window that can slice a call/result pair. `build_muse_input` now tracks emitted `function_call` ids and drops any `function_call_output` whose call isn't in the window.
3. **Clean Audio master (the "raised noise floor / reverb swelling until I talk" bug)** — single-pass `loudnorm` is a gain RIDER: on a noise-only head it pushed gain toward -16 LUFS (pumping room tone + reverb) then ducked when speech arrived. Replaced with `acompressor` 2:1 above -24 dB (only pulls peaks down) → measured STATIC gain to -16 LUFS integrated (`measure_lufs`, loudnorm print_format=json) → `alimiter` -1.5 dBTP. A/B on the reference clip with a 4 s silent head: old chain widened head→speech by +17 dB of noise lift; new chain preserves the isolation's floor exactly.
4. **Fallback denoise** (no voice model): `afftdn nr=12 nf=-40 tn=1` + `agate` 2:1 soft expander (threshold 0.008, knee 4). Pink-noise-over-speech test: head -48 → -69 dB, speech level unchanged.
5. **Per-selection cleanup** — `video_audio_enhance` accepts `clips[]` (timeline ids; linked V+A partners follow via `linked_ids`). `enhance_source` masters each distinct source once; only targeted clips get `audioAsset` repointed. `asset` still cleans the whole source. `enhance_clips` reloads the composition after the manifest rewrite.
6. **Audio panel button** — "Clean Audio" / "Clean Audio (N selected)" driven by `s.selection`; while running shows "Processing…" with a spinning icon at FULL opacity (`.ve-btn.busy:disabled { opacity: 1 }`) instead of the dim disabled look. Hint line explains select-to-target. Status line counts cleaned clips.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA (~8 min)
1. **Muse** — in the long "Video Editor Dev" chat (527k tok, several screenshots) attach a new screenshot and send: reply streams, no 400. Trigger a task_continue wake-up: no call_id 400.
2. **Audio** — import A-roll with a quiet head, Clean Audio (nothing selected): head stays quiet, no swell before speech; -16 LUFS integrated on export.
3. **Selection** — select 2 clips → button reads "Clean Audio (2 selected)" → only those 2 play cleaned (Inspector → Audio asset), others original.
4. **Button** — during processing reads "Processing…" with spinner, full opacity.

---

# Stage build — 2026-09-07 v1.0.16 (local audio suite + chat timeline + create polish)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `761a6bc` — feat(video): local audio suite (normalize+denoise, track trims, keyframe ducking) + chat timeline layout + create button polish

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app`
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg` (~17.2 MB)

**What changed:**
1. **Create button** — welcome form restyled: 38px input + button, 10px radius, disabled until a name is typed.
2. **Auphonic removed** — credential command, keychain path, UI, and engine enum all gone. Audio is local-only.
3. **Normalize** — `audio.normalizeDb` (default -3 dBFS): two-pass peak measure + gain with limiter safety on export; Clean A-roll writes it into the composition.
4. **Noise reduction** — `audio.denoise` 0..1 slider (afftdn, strength-mapped) with **Preview denoise** rendering an 8s audition sample to .cache for listening before Clean A-roll commits.
5. **No auto-ducking** — sidechain graph deleted; Duck defaults off; all audible clips mix flat. Quick action rewritten to manual keyframes.
6. **Track volumes** — per-track trim sliders in the Audio panel (`audio.trackGain`), applied in export graph + preview.
7. **Keyframe toggle** — dB lane under any audio/video track: click to add, drag (shift = fine), double-click to delete. Timeline seconds, shared by UI + agent (`update_clip {audio:{keyframes:[{t,db}]}}`), rendered + previewed by one shared evaluator.
8. **Chat layout** — finished turns now persist + render the ordered timeline (text and tool cards interleaved as streamed) instead of regrouping all cards at the bottom.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA (~10 min)
1. **Audio** — move denoise slider, Preview denoise, listen; Clean A-roll; export and confirm peaks + cleanup.
2. **Tracks** — trim A2, toggle keyframes, add/drag a diamond, export with the dip.
3. **Chat** — long agent turn keeps streamed order after finishing.
4. **Create** — welcome form: button disabled until typed, creates cleanly.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — 2026-09-07 ~09:02 PDT — v1.0.16 (review feedback round 2)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `66b0031` — feat(video): review UX — static note header, Apply Feedback button, gapless audio handoff

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41553704 bytes), mtime **Sep 7 09:02**
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg`, **18013969 bytes (~17.2 MB)**, mtime **Sep 7 09:02**

**What changed (feedback round):**
1. **Inspector: Title unlinked** — review notes show a static "Review note · time range" header instead of the linked Title field. The Feedback textarea is the single editor (timeline labels + tooltips read from it).
2. **Apply Feedback button** — sticky footer at the bottom of the Inspector (Scene view + any selection) while open R1 notes exist. One click flushes the save, sends the apply prompt with all open notes + timestamps through the shared agent path, and flips the dock to the Agent tab. Button shows the open-note count, disables with "Working…" mid-turn.
3. **Agent send path shared** — new `sendVideoPrompt`/`stopVideoTurn` in AgentDock; dock composer + Stop delegate to it (same context, history, compact, live-timeline reload lifecycle). Dead code removed.
4. **Gapless audio handoff** — outgoing audible elements keep a ≤350 ms tail past each cut until every audible owner under the playhead is confirmed rolling (unpaused, buffered, on-position); tail runs before mute is applied so cuts never mute the cover early. Pre-roll warms every incoming clip: exact-arrival play with room, seek-parked hold-at-head for clips near source 0.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA for this build (~8 min)
1. **Note header** — select an R1 note → header reads "Review note · range", no Title input; editing Feedback updates the lane label.
2. **Apply Feedback** — with 2+ open notes, Inspector footer shows "Apply Feedback (2)" → click → dock flips to Agent, turn runs, notes resolve to hidden.
3. **Audio** — play across 3+ cuts → no ~1 s silence at clip starts; pause/resume still frame-accurate.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — 2026-09-07 ~06:17 PDT — v1.0.16 (review lane + agent vision)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `da012a7` — feat(video): review lane (R1 notes) + agent canvas vision (video_look)
- `fa5ad78` — chore: bump version 1.0.15 → 1.0.16 (review lane + agent vision)

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41,553,704 bytes), mtime **Sep 7 06:17**
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg`, **18,013,736 bytes (~17.2 MB)**, mtime **Sep 7 06:17**

**What changed:**
1. **Review mode** — third tool button (💬, `R`) next to Select/Razor. Entering Review reveals the **R1 lane** on top; it stays visible while notes exist. Click-drag on R1 drops a timestamped note (plain click = 2s note); Inspector's **Feedback for the agent** section edits text + resolved state. Notes never touch renders (excluded from duration, visuals, audio; razor/close-gaps skip them).
2. **Agent vision** — new `video_look {project, time|times}` tool renders the real canvas (grade + LUT + overlays) at any timecode(s, max 4) and the model **sees the frames as images** in its next message — wired on Anthropic (merged into tool_result content), OpenAI/OpenRouter (image_url passthrough), and Meta (input_image translation) loops, interactive + headless. First frame also lands in the Video tab preview. Delete spent frames with `delete_file` on `Video/<project>/.cache/frame-*.jpg`.
3. **Agent sees feedback three ways** — `video_project` exposes `reviewNotes`/`reviewNotesResolved`, every dock message carries open notes inline, and tool instructions say to read/act/resolve them (resolve = `video_edit update_clip {hidden:true}`).

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA for this build (~10 min)
1. **Review** — open a project, hit `R`, drag on the R1 lane → note appears, type feedback in Inspector → agent dock message shows the note; ask agent to act → it resolves via `hidden:true`.
2. **Vision** — ask agent "look at 12s and tell me what you see" → `video_look` runs, reply references the actual frame; first frame shows in Video tab preview.
3. **Cleanup** — `delete_file` on a `frame-*.jpg` → gone; export unaffected.
4. **No render pollution** — export with open notes → notes absent from output, duration unchanged.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

---

# Stage build — 2026-09-04 ~23:02 PDT — v1.0.15 (Spark variant knob)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `3767917` — feat(agents): per-agent model variant knob (Muse Spark reasoning effort); v1.0.15

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41,284,968 bytes), mtime **Sep 4 23:02**
- dmg: `dmg/AYGENT_1.0.15_aarch64.dmg`, **17,966,773 bytes (~17.1 MB)**, mtime **Sep 4 23:02**

**What changed:** Agent edit tab gets a second full-width Variant dropdown (Auto, minimal, low, medium, high, xhigh, max) shown when Provider is Muse Spark / OpenAI / OpenRouter. Stored per-agent, sent as `reasoning:{effort}` on `/responses`; chat model line shows `model: muse-spark-1.3 · high`. Other providers hide it.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

# Stage build — 2026-09-04 ~19:40 PDT — v1.0.14 (sidebar overflow fix)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `cbe89e6` — fix(video): sidebar overflow — fields shrink inside 300px panel; v1.0.14

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41,296,408 bytes), mtime **Sep 4 19:40**
- dmg: `dmg/AYGENT_1.0.14_aarch64.dmg`, **17,952,514 bytes (~17.1 MB)**, mtime **Sep 4 19:40**

**What changed:** Graphics-panel color rows, Type/Layout selects, and the Bright HUD/ELI5 seg buttons were spilling past the 300px sidebar. All inputs/selects/seg buttons now shrink + truncate (ellipsis) inside the panel; `\`ve-row2/3\`` children get `min-width: 0`; swatch hex inputs flex-shrink.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

# Stage build — 2026-09-04 ~17:20 PDT — v1.0.13 (video editor round 2)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, pushed to `origin/staging`):**
- `5c78c04` — fix(video): clamp caption line ends to next start (no co-showing); vertically center seg/dock-tab/scene-preset/quick/welcome buttons
- `d15c856` — feat(video): structured brand style fields, stock Bright HUD prefill + ELI5 dark pack
- `1bd8d39` — feat(video): transcript editor — full scrollable lines, per-word times, merge/split, editable words
- `ad844a3` — feat(video): delete project from top bar; v1.0.13

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41,295,560 bytes), mtime **Sep 4 17:20**
- dmg: `dmg/AYGENT_1.0.13_aarch64.dmg`, **17,950,575 bytes (~17.1 MB)**, mtime **Sep 4 17:20**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this build adds (since v1.0.12)
1. **Caption overlap fix** — line ends clamp to the next start; consecutive captions can't co-show.
2. **Button alignment** — seg/dock-tab/scene-preset/quick/welcome buttons vertically centered.
3. **Brand style fields** — Graphics panel: stock Bright HUD prefill (accent #00cafc, white panels, black ink), one-tap ELI5 dark pack (#0a0e15/#00e6ff), 9 color swatches, type/layout/motion fields. Agent reads a BRAND block; caption builds use panel size/weight/accent/width-cap.
4. **Transcript editor** — Captions panel: full scrollable lines with timecodes, per-word editable text + times, merge lines, split at any word, add/delete words. Saves to transcript.json; sections to captions.lines (builds respect them).
5. **Delete project** — trash button in the top bar next to the project picker (confirm dialog; removes Video/\<name>/, original footage untouched).

## Smoke QA for this build (~12 min)
1. **Delete** — open a scratch project, hit the trash icon in the top bar → confirm → project gone from the picker, welcome screen shows.
2. **Transcript** — transcribe, open Captions → editor lists every line with timecodes → double-click a word, fix spelling, Save → rebuild captions → fixed word renders.
3. **Merge/split** — checkbox 2 lines → Merge → one section; ✂ between words → two sections; rebuild → matches arrangement.
4. **Brand** — Graphics panel shows Bright HUD stock values; switch to ELI5 dark → agent's next graphic uses dark tokens.
5. **Captions** — rebuild overlay → consecutive lines never co-show; keyword accent follows the panel.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — 2026-09-04 ~15:06 PDT — v1.0.12 (video Hyperframes rebuild)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commit:** `b365023` — feat(video): Hyperframes graphics+captions rebuild, linked clips, style guides (on `staging`, pushed to `origin/staging`)

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41,206,264 bytes), mtime **Sep 4 15:06**
- dmg: `dmg/AYGENT_1.0.12_aarch64.dmg`, **17,925,860 bytes (~17.1 MB)**, mtime **Sep 4 15:06**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this build adds
1. **Linked A/V clips** — V1 video auto-creates a linked A1 audio follower; split/delete/duplicate/drag keep them together; Inspector shows the link.
2. **Hyperframes captions** — `video_build_captions` renders transcript words (through the A-roll cuts) to a transparent caption composition in the project's style → T1 overlay clip. ASS/drawtext caption path removed.
3. **Hyperframes graphics overlays** — `video_render_overlay` renders one approved graphic per call to a transparent V3 clip (plan-first protocol enforced in tool instructions). Graphics panel with instructions + style-guide picker (.md/.txt/.rtf/.pdf/.docx text extraction).
4. **Backend** — new `video_hyperframes.rs` (toolchain via provisioned runtime, overlay staging/render/register), simplified Captions model, audio-dedupe on export, `video_pick_style_guide` command.

## Smoke QA for this build (~10 min)
1. **Video tab** — open a project, import media, cut A-roll on V1 → linked A1 follower appears and moves with it.
2. **Captions** — transcribe, then build captions → T1 overlay appears; toggle Captions on/off in preview + export.
3. **Graphics** — ask agent for a lower third → it replies with a PLAN first; approve → V3 overlay clip lands.
4. **Style guide** — pick a .md guide in Graphics panel → agent's next graphic matches it.
5. **Export** — render landscape → no doubled audio, captions + graphics composited.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — 2026-09-03 ~09:43 PDT — v1.0.11 (web_search + Brave Search + Playwright MCP)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **10 warnings, 0 errors** (dead-code only: telegram/browser/exec — same set as prior builds). Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commit:** `b225bd7` — feat: web_search tool (DDG, no key) + Brave Search connector + Playwright MCP entry (on `staging`, pushed to `origin/staging`)

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 39,435,304 bytes), mtime **Sep 3 09:43**
- dmg: `dmg/AYGENT_1.0.11_aarch64.dmg`, **17,250,699 bytes (~16.5 MB)**, mtime **Sep 3 09:43**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This build supersedes the Aug 21 11:49 build (`d2181bf`).** Same version number (1.0.11) — all additions are non-breaking.

## What this build adds
1. **`web_search` tool (no key, DuckDuckGo)** — new builtin alongside `fetch_url`: search runs on the privileged side (same mediated-reach model), returns title + url + snippet. Wired everywhere: exec arm + schema, Tools registry (`builtin.search`, default ON), local-model allowlist + prompt, `AGENT_SYSTEM` mention, dashboard button whitelist, UI summary line (`🔍 query`) + skill `BASE_TOOLS`.
2. **Brave Search connector** — new `brave` entry in the Connections catalog (Research): `X-Subscription-Token` auth, 4 read-only tools (web/news/images/videos). The upgrade path when DDG ranking isn't enough.
3. **Playwright MCP entry** — new `playwright` catalog entry: Microsoft's `@playwright/mcp --isolated` via the provisioned Node (no system Node/Playwright needed); enable step downloads Chromium (~170MB) via the CLI's `install-browser` → `playwright-core`. MCP UI is data-driven so no UI changes were needed.

## Smoke QA for this build (~10 min)
1. **web_search** — ask a cloud agent "what's current on X" → `🔍` tool line renders, results come back as title/url/snippet; ask it to read one → `fetch_url` follows up.
2. **Brave** — paste a `BSA…` key under Connections → Research card connects; `brave_search_web` + one vertical (news/images/videos) return real hits.
3. **Playwright** — enable Playwright in MCP → package installs + Chromium downloads (~170MB); agent can visit a page / click / type via `mcp__playwright__*` tools.
4. **Local models** — on a tool-capable local agent, "search the web for X" → `web_search` executes (allowlist + prompt wired).
5. **whoami** — "what can you do?" lists `web_search` under Built-in, Brave under Connected accounts, Playwright under MCP servers.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — 2026-08-21 11:49 PDT — v1.0.11 (REBUILD: RAM-pool sizing + kv-trim fallback)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **10 warnings, 0 errors** (dead-code only: telegram/browser/exec — same set as prior builds). Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (on `staging`, HEAD `d2181bf`):**
- `9ac5f34` — local models: run up to RAM-pool size via offload-aware verdicts, small quants, q8 KV, safe compaction
- `d2181bf` — fix(local): kv-trim fallback — clear cache + full re-decode when partial `seq_rm` is unsupported (fixes Gwen's mid-chat "kv trim" error)

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 39,271,416 bytes), mtime **2026-08-21 11:49**
- dmg: `dmg/AYGENT_1.0.11_aarch64.dmg`, **17,201,835 bytes (~16.4 MB)**, mtime **2026-08-21 11:49**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

**This 11:49 rebuild supersedes both the 11:19 build (`9ac5f34`) and the 09:22 build (`3baacee`).** Same version number — it folds in `d2181bf`, the fix for the mid-chat "kv trim" error Gwen hit on the 11:19 bundle.

## What this build adds
1. **Offload-aware verdicts** — fit assessment now accounts for partial GPU offload: a model bigger than the Metal working set but within total RAM gets a 🟡 partial verdict (runs with some layers on CPU) instead of a hard ❌. Verdicts reflect what the runtime will actually do.
2. **Efficient quant tier** — catalog surfaces an **Efficient** tier (smaller quants — IQ3/Q3-class) on large repos, so a 27B repo offers a pick that fits comfortably where IQ4 is marginal.
3. **q8 KV cache + flash attention** — KV cache quantized to q8_0 (halves KV memory vs f16) + flash attention enabled on the local path; longer usable context in the same working set.
4. **Safe compaction** — compaction on local agents preserves the system prompt; long chats keep their identity/instructions after Compact.
5. **kv-trim fallback (`d2181bf`, new in this rebuild)** — some llama.cpp cache configs (e.g. quantized q8 KV) don't support partial `seq_rm` (trimming only part of a sequence's KV cache). When the trim fails mid-chat, we now clear the cache and do a full prompt re-decode instead of erroring the turn. Slower on that one turn, but the turn completes.

## Smoke QA for this build (~8 min)
1. **🟡 partial allowed** — download/assess a **16GB model on the 24GB Mac** → verdict is 🟡 partial (offload), download proceeds, model loads and streams (some layers CPU-side).
2. **Efficient tier** — open a **27B repo** in the HF catalog → the **Efficient** tier is visible and offers a smaller quant that fits.
3. **Compaction keeps system prompt** — run a long local chat, hit **Compact** → agent still knows who it is / follows its system prompt after compaction.
4. **q8 KV headroom** — on the 27B IQ4_XS agent, usable context is larger than the 09:22 build (q8 KV ≈ half the KV memory); no Decode -3.
5. **kv-trim fallback (the rebuild fix)** — chat **2+ turns** on Gwen's model → no "kv trim" error; every turn streams to completion (turn 2 may prefill slower if the fallback fires — expected).

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — 2026-08-21 09:22 PDT — v1.0.11 (real-fit context window)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **10 warnings, 0 errors** (dead-code only: telegram/browser/exec — same set as 1.0.10). Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commit:** `3baacee` — fix(local): real-fit context window — weights+KV+overhead vs Metal working set; v1.0.11 (on `staging`)

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 39378152 bytes), mtime **2026-08-21 09:22**
- dmg: `dmg/AYGENT_1.0.11_aarch64.dmg`, **17209834 bytes (~16.4 MB)**, mtime **2026-08-21 09:22**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this build fixes
**Root cause:** assessment said 128k but ignored KV. A 27B IQ4_XS GGUF (16GB) at 128k needs ~58GB KV + 16GB weights = 75GB vs ~16GB Metal working set on a 24GB Mac (24 * 2/3) => `Decode Error -3`. The predictor only counted weights + 1.3GB overhead, so it showed ⚡ great.

1. **`hardware.rs`** — `working_set_gb()`, `kv_per_token_gb()` (scales 7B ~0.25MB to 27B ~0.45MB), `max_context_tokens()` (weight+KV+1.5GB vs working set, snapped to 2k/4k/8k/16k...), `recommended_context() = min(advertised, max_that_fits)`, `predict_with_ctx()` now includes KV at the advertised window. Retroactive: already-downloaded models auto-cap via `local_context_budget()` reading the real file size on next run.
2. **`lib.rs`** — `local_context_budget()` file-size-aware (actual GGUF bytes + `parse_params()` → `recommended_context()`); all 3 catalog paths score with `predict_with_ctx(..., m.context_tokens)`.
3. **`local_provider.rs`** — `gpu_layers_for()` now uses `hardware::working_set_gb()` + `kv_gb_for()` so runtime agrees with prediction; `catalog.rs` `parse_params` made pub.

**Result for your Qwen3.8-27B IQ4_XS (16GB) on 24GB:** advertised 32k (128k yarn) → **auto-set 8192** (2k-4k on tighter, 32k+ would OOM). For 16k+ pick IQ3_M/Q3_K_L (~11-13GB) from the same repo.

## Smoke QA for this build (~6 min)
1. **The fix** — on your Qwen3.8-27B IQ4_XS agent, chat a turn → streams fine, no Decode -3; header shows ~8k context (not 128k).
2. **Catalog badge** — HF search/browsing for that model now shows `👍 Fits at ~8k (advertised 32k would OOM)` instead of `⚡ great`.
3. **Retroactive** — your already-downloaded 16GB GGUF auto-caps to ~8k on next launch (drop to 4k in Settings if you kept 128k).
4. **Long context still works** — on a 7B model, full 32k still available.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 19, 2026, 19:05 PDT — v1.0.10 (REBUILD #2 → promoted with recall/RAG on top)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors** (dead-code warnings only). Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**This rebuild SUPERSEDES both prior v1.0.10 builds** — the 17:32 build (3 commits) and the 18:26 rebuild (6 commits, never shipped to these notes). Same version number — all additions are non-breaking fixes/features.

**Commits folded in (all on `staging`, HEAD `f6470e5`):**
- `86940db` — fix(local): SIGABRT crash on long local-model chats — size llama.cpp n_batch to the prompt
- `758afbb` — perf(local): partial GPU offload — kill the CPU cliff
- `42d8b90` — perf(local): KV-cache reuse across turns — persistent llama sessions; v1.0.10
- `5889490` — fix(local): rescue tool calls emitted inside an unclosed `<think>` block
- `e13e23a` — fix(local): Compact works for local-model agents — summarize in-process
- `692fe34` — feat(catalog): unblock uncensored/abliterated models in HF search + lookup
- `f6470e5` — fix(local): evict old model sessions + weights when switching models **(new vs. 18:26)**
- `791ad98` — feat(local): recall memory tool + RAG auto-inject + few-shot tool examples **(landed after this bundle; included in the promoted production build)**

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 39,334,296 bytes), mtime **Aug 19 19:05**
- dmg: `dmg/AYGENT_1.0.10_aarch64.dmg`, **17,208,677 bytes (~16.4 MB)**, mtime **Aug 19 19:05**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this rebuild adds vs. 18:26
1. **Model-switch eviction (`f6470e5`)** — switching a local agent to a different GGUF now evicts the old model's persistent sessions AND unloads the old weights before loading the new model. Previously both models' weights could sit in memory simultaneously → memory error / failed load on the second model.

## Carried from 18:26 (still in this bundle)
- **Think-rescue (`5889490`)** — tool calls emitted inside an unclosed `<think>` block are rescued and executed.
- **Local Compact (`e13e23a`)** — Compact works on local-model agents; summary generated in-process.
- **Catalog unblock (`692fe34`)** — HF search/lookup no longer filters uncensored/abliterated models.

## Carried from 17:32 (still in this bundle)
- **Partial GPU offload (`758afbb`)** — oversized GGUFs offload as many layers as fit instead of dropping to all-CPU.
- **KV-cache reuse (`42d8b90`)** — persistent llama sessions across turns; turn 2+ prefill near-instant.
- **SIGABRT fix (`86940db`)** — n_batch sized to the prompt; long local prompts don't abort.

## Smoke QA for this build (~12 min)
**New in this rebuild:**
1. **Model switch** — on a local agent, load GGUF A, chat a turn, switch to GGUF B, chat again → **second model loads and runs, no memory error**; then switch back → still fine.

**Carried:**
2. **Think-rescue** — on Gwen (local agent), ask "read your memory" → the `whoami`/tool call **executes** instead of being stuffed into the Thoughts bar.
3. **Local Compact** — hit **Compact** on a long local-model conversation → runs, context% drops, summary seed is coherent.
4. **Catalog unblock** — HF search for **"uncensored"** / **"abliterated"** → results return, a model downloads successfully.
5. **KV-cache reuse** — long local chat: turn 2+ prefill near-instant.
6. **Partial offload** — oversized GGUF partially offloads instead of all-CPU.
7. **Long chat past the window** — no crash (SIGABRT fix holds with sessions).
8. **Regenerate / edit history mid-chat** — coherent output, no stale-cache garbage.
9. **Embeddings** — embed/index a long note on the local path → completes, no abort.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 19, 2026, 17:32 PDT — v1.0.10 (local perf: partial GPU offload + KV-cache reuse)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 10 warnings** (dead-code only, telegram/browser/exec — same set as v1.0.9). Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commits (both on `staging`):**
- `758afbb` — perf(local): partial GPU offload — kill the CPU cliff
- `42d8b90` — perf(local): KV-cache reuse across turns — persistent llama sessions; v1.0.10

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app`, mtime **Aug 19 17:32**
- dmg: `dmg/AYGENT_1.0.10_aarch64.dmg`, **17,197,325 bytes (~16.4 MB)**, mtime **Aug 19 17:32**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this build adds
1. **Partial GPU offload (`758afbb`)** — a GGUF too big for full Metal offload now offloads as many layers as fit instead of silently falling back to all-CPU (the "CPU cliff"). Oversized models get proportionally faster instead of 10x slower.
2. **KV-cache reuse / persistent sessions (`42d8b90`)** — the llama context persists across turns in a conversation, so turn 2+ reuses the KV cache instead of re-decoding the whole prompt from scratch. Long local chats: the prompt phase on follow-up turns should be near-instant.

## Smoke QA for this build (~8 min)
1. **KV-cache reuse (the headline)** — long local-model chat: turn 1 normal, then **turn 2+ must be much faster** — prompt/prefill phase near-instant, only generation takes time.
2. **Partial offload** — load a GGUF too big for full GPU offload → it **partially offloads** (faster than before) instead of dropping to all-CPU.
3. **Regenerate** — regenerate a response mid-chat → works, output coherent (cache invalidation on divergence).
4. **Edit history mid-chat** — edit an earlier message and continue → works, no stale-cache garbage in the reply.
5. **Idle then resume** — leave a local chat idle ~5 min, send a new turn → still works (session survives or rebuilds cleanly).
6. **Long chat past the window** — push a local chat past the context window → still **no crash** (v1.0.9 SIGABRT fix holds with sessions).
7. **Embeddings** — embed/index a long note on the local path → completes, no abort.

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 19, 2026, 15:23 PDT — v1.0.9 (local-model SIGABRT fix)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, **0 errors, 10 warnings** (dead-code only, telegram/browser/exec). Both bundles produced (.app + dmg). Unsigned stage build — sign/notarize at promotion.

**Commit:** `86940db` — fix(local): SIGABRT crash on long local-model chats — size llama.cpp n_batch to the prompt

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app`, mtime **Aug 19 15:23**
- dmg: `dmg/AYGENT_1.0.9_aarch64.dmg`, **~16 MB**, mtime **Aug 19 15:23**

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## What this build fixes
**Root cause:** llama.cpp was initialized with a fixed `n_batch` smaller than long prompts; feeding a prompt past ~2048 tokens tripped a `GGML_ASSERT` → **SIGABRT**, killing the whole app mid-chat. `n_batch` is now sized to the actual prompt, so long local-model prompts decode instead of aborting.

## Smoke QA for this build (~5 min)
1. **The fix** — on a llama.cpp agent, run a LONG chat (paste enough to push the prompt past **~2048 tokens**, or accumulate turns) → response streams normally, **no crash / no SIGABRT**.
2. **Embeddings on a long note** — embed/index a long note on the local path → completes, no abort.
3. **Short local turn** — a normal-length local chat still streams fine (no regression from batch sizing).

## Regression pass (carry-over)
1. **Launch** — agents + conversations all present.
2. **One chat turn with tools** — read/write a file in the agent folder.
3. **Context meter** — cloud turn shows context% + $; local turn tracks context, no $.
4. **whoami** — tools list renders as a clean table.
5. **Browser** — open a page in the in-app browser, agent read of the page.
6. **Cmd+Q** — quits cleanly, daemon gone from Activity Monitor.

---

# Stage build — Aug 13, 2026, 17:11 PDT — v1.0.4 (signed, notarized, reissued — current production)

**Status:** ✅ Signed + notarized + stapled. `cargo tauri build` exit 0, **6 warnings, 0 errors** (dead-code only). Apple Developer ID signed + notarized + stapled + verified.

**Version:** `1.0.4` (`src-tauri/tauri.conf.json` + `Cargo.toml` + `ui/package.json` + `Cargo.lock` in lockstep). Tag `v1.0.4` → `2044884` (reissued). `staging` tip `9ea239d` == `main` `2044884` content.

**Commits folded into this v1.0.4 (reissue on top of original v1.0.4):**
- `2b796d5` — harness: uncap large file writes (provider 64k + honest truncation — fixes Stanislawski 20KB routes.ts red-herring owner bug)
- `edb84ac` — chore: bump version 1.0.3 → 1.0.4 (uncap large file writes)
- `9473288` — chore: update Cargo.lock (v1.0.4 release build)
- `9ea239d` — feat: per-agent GitHub PAT in shell (ephemeral GITHUB_TOKEN, no keychain overwrite) — **Pro Mode shell now injects the agent's Connections PAT per-process (GITHUB_TOKEN/GH_TOKEN + per-PID `GIT_CONFIG credential.helper`), host shell & macOS Keychain untouched**
- `2044884` — promote: staging → production (v1.0.4 — per-agent GitHub PAT in shell) `tag: v1.0.4` retagged (forced) to `2044884`

**Artifacts (signed + notarized):**
- app: `src-tauri/target/release/bundle/macos/AYGENT.app` → `/Applications/AYGENT.app` (`Contents/MacOS/aygent` 38,741,936 bytes), `spctl -a -t install` → `accepted source=Notarized Developer ID`
- dmg: `target/release/bundle/dmg/AYGENT_1.0.4_aarch64.dmg` **16,921,708 bytes (~16.1 MB)**, mtime **Aug 13 17:11**, `notarytool submit --wait` → `01bf1c9b-c364-4779-bfaa-33921336c9da Accepted`, `stapler staple` dmg + app
- staged site file: `~/AYGENT/Cleo/masonleebuild/product-files/AYGENT-1.0.4-macOS.dmg` (16.9 MB staged), uploaded to Supabase bucket `products/aygent/...` via `node scripts/publish-release.js 1.0.4` → `product_releases 1.0.4 recorded`, emailed `1/1 owners`
- backup: `~/.aygent-backups/AYGENT.app.20260813-171130/` + `.old` (rollback ready)

**Pro Mode fix (9ea239d):** Shell `git` no longer uses the global `osxkeychain` entry. `exec.rs` (`GLOBAL_DB` + `inject_github_env` + `spawn_for_agent`/`run_for_agent`) + `lib.rs` `install_db` + `exec_shell_tool` now resolve `connections::resolve_github_push_token(db, agent_id)` and inject `GITHUB_TOKEN`/`GH_TOKEN`/`GIT_CONFIG credential.helper` **per-process only** (Pro-Mode gated, fail-open if no enabled GitHub connection). Your personal `security find-internet-password -s github.com` is untouched.

**Harness fix (2b796d5):** Large file writes (>20KB, e.g. `routes.ts`) were clipped before reaching GitHub — provider 64k uncap + honest truncation error surfaced instead of silent vanish.

**To run the new build:** Quit and reopen `/Applications/AYGENT.app` (running process is old binary until relaunch — intended). Verify: `PlistBuddy CFBundleShortVersionString` → `1.0.4`, `spctl` → `Notarized Developer ID`.

---

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

---

# Stage build — 2026-09-07 ~11:39 PDT — v1.0.16 (Create button reset fix, rebuild)

**Status:** ✅ Built clean. `cargo tauri build` exit 0, dead-code warnings only (same set). Both bundles produced (.app + dmg).

**Commit (on `staging`, pushed):** `14a7f1a` — fix(video): scope .ve button reset to non-.ve-btn so Create keeps padding

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41247736 bytes), mtime **Sep 7 11:39**
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg`, **17942691 bytes (~17.1 MB)**, mtime **Sep 7 11:39**

**Root cause of the repeat failure:** the last two bundles were packaged off a stale `ui/dist` that still contained the unscoped `.ve button{...padding:0}` reset — so the Create button fix never actually shipped. `dist` rebuilt from source (verified `.ve button:not(.ve-btn)` in shipped CSS) before this build.

**What changed:** generic reset now only strips background/border/padding on non-`.ve-btn` buttons; `.ve-btn`/`.ve-create-btn` padding + height survive. Create button renders unclipped.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA (~2 min)
1. **Create** — welcome screen: + Create button fully legible, disabled until a name is typed, creates cleanly.

---

# Stage build — 2026-09-07 ~13:40 PDT — v1.0.16 (cleaned-audio proxy playback + re-clean sweep fix)

**Status:** ✅ Built clean. `cargo tauri build` exit 0 (two sequential builds, second includes sweep fix). Both bundles produced (.app + dmg).

**Commits (on `staging`, pushed):**
- `7df98fd` — fix(video): cleaned-audio proxy playback (full-length .cleaned.mov, single-element preview) + stable denoise floor + cleanup button wrap
- `b6bd3dc` — fix(video): re-clean no longer deletes the proxy it just wrote (sweep keeps same-rel file; drop orphan wav)

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41383208 bytes), mtime **Sep 7 13:40**
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg`, **17966285 bytes (~17.1 MB)**, mtime **Sep 7 13:40**

**What changed:**
1. Cleanup on video sources now builds a full-length `.cleaned.mov` proxy (picture stream-copied, cleaned audio apad-padded to full video duration, AAC) instead of a short `.wav` — preview plays one element, no dual-clock drift through cuts.
2. Denoise retuned (nr 0..18, nf=-30, no afftdn noise-tracking) — stable floor, no pumping. Preview chain matches cleanup exactly.
3. Re-clean stale sweep no longer deletes the file it just wrote (same-rel guard); intermediate wav removed once baked into proxy.
4. Cleanup buttons wrap (`.ve-btn-row` verified in shipped `ui/dist` CSS).
5. Export graph untouched (already handles duration via apad/atrim + peak normalize).

**user-facing bug hit on the 13:27 bundle:** re-clean failed with `ffprobe failed: ... .cleaned.mov: No such file` because the sweep deleted the fresh proxy. Fixed in `b6bd3dc`, included in this 13:40 bundle.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA (~2 min)
1. **Re-clean A-roll** — succeeds, produces `.cleaned.mov` in Generated, no ffprobe error.
2. **Playback** — smooth through cuts and full track; no choppiness.
3. **Bypass toggle** — A/B cleaned vs original works.
4. **Buttons** — Clean / Re-clean buttons wrap inside the panel, no overflow.

---

# Stage build — 2026-09-08 ~10:31 PDT — v1.0.16 (neural voice cleanup + live blend)

**Status:** ✅ Built clean. `cargo tauri build` exit 0 (dead-code warnings only, same set). Both bundles produced (.app + dmg).

**Commit (on `staging`, pushed):** `654b00a` — feat(video): neural voice cleanup (DeepFilterNet3, optional download) + live Cleanup amount blend

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41384312 bytes), mtime **Sep 8 10:30**
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg`, **17964760 bytes (~17.1 MB)**, mtime **Sep 8 10:31**

**What changed:**
1. **Neural isolation** — DeepFilterNet3 via uv (torch 2.5.1 + torchaudio 2.5.1 pinned; torchaudio file I/O bypassed via ffmpeg pipes after verifying backend failures). API verified live against AUDIO REFERENCE, not just docs.
2. **Measured EQ** — first curve was +6dB too bright vs the Auphonic reference; also caught `equalizer` as a silent no-op in provisioned ffmpeg. Final: `highpass=80,lowpass=12000,treble=-4` — within 1–4dB/band of Auphonic.
3. **Live Cleanup amount** (`audio.cleanMix`, default 1) — equal-power crossfade, preview + export matched, no re-clean. Replaces bypass checkbox.
4. **Deleted:** `video_audio_audition` (schema, dispatch, fn), Preview button, `denoise`/`normalizeDb`/`cleanEnabled` everywhere, two-pass peak normalize on export. Zero back-compat per plan.
5. **Optional download** — Audio panel offers one-time voice-model install (~90MB); without it Clean uses a light nr=6 fallback that can't go robotic.
6. Chain: mono extract → DF3 isolate → EQ → dynamic loudnorm −16 LUFS → full-length `.cleaned.mov` proxy (unchanged).

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA (~3 min)
1. **Download voice model** — Audio panel → Download → READY, status flips to neural.
2. **Clean A-roll** — succeeds, toast names the engine; produces `.cleaned.mov`.
3. **Blend live** — drag Cleanup amount 0/50/100 → preview follows instantly, no re-clean; export matches.
4. **Natural sound** — full clean sounds like close-mic'd voice, not robotic.

---

# Stage build — 2026-09-08 ~11:05 PDT — v1.0.16 (self-contained voice toolchain)

**Status:** ✅ Built clean. `cargo tauri build` exit 0. Both bundles produced (.app + dmg).

**Commit (on `staging`, pushed):** `74a9223` — fix(video): voice download provisions its own uv toolchain (self-contained, no MCP needed)

**Artifacts:** `AYGENT-Stage/src-tauri/target/release/bundle/`
- app: `macos/AYGENT.app` (`Contents/MacOS/aygent` 41440568 bytes), mtime **Sep 8 11:04**
- dmg: `dmg/AYGENT_1.0.16_aarch64.dmg`, **17978743 bytes (~17.1 MB)**, mtime **Sep 8 11:05**

**What changed:** Download-voice-model button provisions everything itself under `runtime/voice/` (pinned uv 0.12.2 tarball via async reqwest + system tar, managed Python, DF3 weights). Zero MCP dependency, zero new cargo deps. `voice_status` also reports `uv` presence.

**Prod is untouched.** Quit any running AYGENT first — an open window is still the OLD build. Relaunch from the Stage bundle.

## Smoke QA (~3 min)
1. **Download voice model** — works standalone with Blender MCP off; toolchain + model land under `runtime/voice/`.
2. **Clean A-roll** — neural isolation, toast names engine.
3. **Blend live** — Cleanup amount 0/50/100, preview + export match.
