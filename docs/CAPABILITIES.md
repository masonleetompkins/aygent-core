# AYGENT — Capabilities (v1.0.16)

_The canonical reference for what AYGENT can do, as shipped in the signed,
notarized release. This is the source-of-truth capability doc: keep it in
lockstep with the app. Product/marketing copy lives in
`masonleebuild/aygent-copy.md`; the security/architecture contracts live in
`docs/CONTRACTS.md`. This file is the functional inventory._

**What AYGENT is:** a native macOS app where AI agents run on YOUR machine —
your files, your memory, your keys, your control. Not a rented chat window.
$20 one-time. macOS 13+. Developer ID signed + Apple-notarized.

---

## 1. Agents (the core unit)
- **Unlimited agents**, each a config profile with its own: name, icon, color,
  personality/soul (system prompt), model + provider, and **home folder**.
- **The Agent Rail** shows every agent; agents run **in parallel** (concurrent
  turns), so one can work on a schedule while you chat with another.
- Each agent is jailed to its **home folder** — a fail-closed OS-enforced
  security boundary (macOS Seatbelt), not a prompt convention. It can read,
  write, and organize only inside that folder (+ any read-only mounts granted).
- **Soul generation:** an agent can author its own SOUL.md from a short brief
  (uses its own provider/model).
- **Context mode:** `isolated` (default) or `shared:<pool>` — agents can share
  a context pool; `agents_sharing_folder` surfaces who shares a folder + Save
  Point history.
- **Read-only mounts (shared context):** mount another folder (or another
  agent's folder) read-only so a team of agents can pool knowledge. The agent
  is told about its mounts and reaches them through a virtual **`@shared/`**
  namespace: `list_files("@shared")` lists the shared folders by label,
  `list_files("@shared/<label>")` browses one, and
  `read_file("@shared/<label>/path")` reads a file — discoverable + unambiguous
  even when a shared file has the same name as one of the agent's own.

## 2. Memory (atomized + linked, plain files)
- Agents remember automatically: durable facts, project state, lessons —
  captured as **plain markdown files** in the agent's folder.
- **Atomized + linked**, Obsidian-style (`[[wikilinks]]`), improving recall
  while reducing token use. No vector-DB black box; every note is human-readable,
  editable, portable, and backup-able.
- **Retrieval:** semantic top-K + graph expansion over the derived index
  (embeddings run IN-PROCESS via compiled-in llama.cpp — a small nomic-embed
  GGUF auto-downloaded on first use; no Ollama, no external process).
- **Layers:** L1 episodic (daily notes), L2 durable atoms ("remember this" +
  novelty-dedup), auto-capture (salience-gated, per-agent toggle, default on).
- **Portable across models:** switch the model and the agent keeps its memory
  and identity — the memory is the agent's, not the model's.
- **Import memory:** one-click port of an existing vault/bundle into an agent.

## 3. Models — cloud or local (BYOK)
- **Cloud (bring your own key):** Anthropic, OpenAI, OpenRouter (hundreds of
  models), and **Muse (Meta)** — the Meta Responses-API model service, now a
  first-class selectable provider (dropdown in Agents/Onboarding + key row in
  Settings). Keys live in the macOS Keychain; conversations stay local.
- **Local:** download a curated GGUF (Qwen / Mistral / Kimi / Llama) in-app and
  run **fully offline**. Hardware predictor scores fit + speed per quant; the
  context window is auto-chosen from the model's real window capped to a
  memory-safe budget for the machine.
- **Per-agent model choice** — a frontier model for research, a free local
  model for journaling, simultaneously.
- Local tool-use is family-native (detected from the GGUF chat template); local
  models get the file tools + `whoami` + `recall` (memory search), with
  few-shot examples in the prompt (chat-only models get a single no-tools turn).
- **Local memory just works (1.0.10):** notes relevant to your message are
  auto-retrieved and injected each turn (RAG) — no tool call needed; `recall`
  covers explicit "search your memory" asks. Compact works offline too: the
  local model writes its own summary in-process.
- **Local speed (1.0.10):** persistent sessions reuse the KV cache across turns
  (follow-up prompts prefill near-instantly) and oversized models partially
  offload to GPU instead of dropping to CPU. Switching models frees the old
  one's memory. Power-user HF search is unfiltered — any GGUF you can find,
  you can run.

## 4. Chat — context meter, cost, and compaction (new in 1.0.1)
- **Live context-window meter:** the chat header shows how full the model's
  context window is as a **% bar next to the agent name** — amber at ≥75%, red
  at ≥90%, with an at-the-wall warning near the limit. No more silently blowing
  past the window and losing a long reply to a "prompt too long" error.
- **Running session cost:** a live **$ figure** for the conversation, computed
  from the model's real price. Under each message's timestamp: that turn's
  **tokens + $ cost**. (Cost is hidden for local models — they're free — but the
  context % still tracks, using the real memory-capped window.)
- **In-chat compaction:** a **⚡ Compact** button summarizes the earlier
  model-facing history into a compact seed so a long chat can keep going without
  hitting the wall — the **visible transcript is untouched**, only the token load
  resets. The summary is authored by the agent's own model.
- **All providers metered:** token usage is parsed from every provider's stream —
  Anthropic (`message_start`/`message_delta`), OpenAI + OpenRouter
  (`stream_options.include_usage`), Muse (`response.usage`), and local llama.cpp
  (real prompt/generated token counts + true capped window). Prices come from a
  curated table (`pricing.rs`); an unknown model shows tokens + % but no cost
  rather than a wrong number.
- **Chat polish (1.0.5):** conversation history tucks behind a **hamburger drawer** (no split-pane waste), scroll pins to the true bottom so tool cards never clip at the rounded frame, and per-keystroke pin stutter is gone. **OpenAI/OpenRouter parallel tools (1.0.6)** no longer 400 — the history guard now searches backwards so both parallel tool_results survive.

## 5. Tools — 94+ real-world capabilities
Base file tools (always on, jailed): `read_file`, `write_file`, `list_files`,
`rename_file`, `delete_file`. Plus:

- **Built-in tools** (per-agent toggle): `generate_pdf` (styled PDF),
  `transcribe_audio` (Whisper), `fetch_url` (HTTPS fetch → readable text; the
  daemon has no network, the privileged side fetches), `web_search` (no-key web
  search via DuckDuckGo → title + url + snippet; use it to find what's current,
  then `fetch_url` to read a hit in depth).
- **`whoami`:** an agent reports its own identity, model + provider, context
  mode, and full granted capability set — read-only, no-arg. Works on all
  provider paths incl. tool-capable local models. Assembled from the same source
  as the real grant, so it can't drift.
- **`task_continue`:** an agent schedules its OWN follow-up turn ("check that
  build in 5 min" — it actually resumes and reports).
- **`send_message`:** async inter-agent messaging (guardrailed budget/chain).
- **Connections (registry-driven, credential attached privileged-side; the
  model never sees a token):**
  | Service | Tools | Notes |
  |---|---|---|
  | GitHub | ~17 | repos, PRs, issues, branches, commits, CI, merges, file read/write |
  | Google Workspace | ~17 | Calendar, Docs, Sheets, Slides, Drive (read + write) |
  | Notion | ~21 | pages, databases, blocks, comments (read + write) |
  | Slack | — | messaging |
  | Linear | — | issues |
  | Supabase | ~6 | select/insert/update/delete, RPC |
  | Stripe | ~6 | payments, subscriptions, customers, balance, invoices (read) |
  | Brave Search | 4 | keyed web search (web, news, images, videos) — better ranking than the built-in search |
  | Resend | ~6 | send email, domains/audiences, delivery status |
  | Cloudflare | — | infra |
  | Vercel | — | deploys |
  - Each connection is **per-agent**, with **per-tool on/off switches** and a
    read-only / read+write posture. Multiple accounts per provider (personal +
    work), at most one enabled per agent+provider.
  - **`github_read_file` (improved in 1.0.1):** now returns the **whole decoded
    file** (base64-decoded server-side, up to 200k chars) with a
    `# path · sha · size` header — instead of a truncated slice of the base64
    envelope. Works on private repos, no clone; the returned sha round-trips
    straight into `github_write_file`. Directory listings and >1MB blobs fall
    back gracefully.
- **Harness (1.0.4):** large file writes uncapped to **64k** with an honest truncation error instead of silent clipping — fixes the Stanislawski 20KB `routes.ts` red-herring.
- **Connections inline (1.0.5):** credential form renders inside the clicked card (no modal shuffle). **Save Points labeled “Checkpoint + timestamp”** so the timeline isn't a wall of baselines.
- **Provider reliability (1.0.6):** OpenAI/OpenRouter parallel `tool_calls` handling fixed — both results now reach the model instead of the second being dropped and 400'ing.
- **MCP servers:** one-click enable built-in catalog (Adobe Premiere Pro,
  Blender, Playwright browser automation) or add custom stdio servers; tools
  namespaced `mcp__<server>__<tool>`. Node/uv provisioned in-app (no system
  installs). Playwright installs Microsoft's `@playwright/mcp` plus its own
  Chromium build and can visit pages, click, type, and read the live web in an
  isolated profile (never touches your own browser).
- **Pro Mode (shell):** real `shell_run` / `shell_spawn` / `shell_poll` /
  `shell_write` / `shell_kill`, cwd-pinned to the agent's folder, env-scrubbed,
  **per-agent GitHub PAT** (new in 1.0.4): `git clone/push/pull` in shell now uses
  the agent's Connections GitHub token as ephemeral `GITHUB_TOKEN`/`GH_TOKEN` +
  per-PID `credential.helper` — host shell & macOS Keychain untouched (fail-open
  if no enabled GitHub connection). Gated behind an explicit consent screen. This
  is how AYGENT builds AYGENT.

## 6. Dashboards (per-agent, prompt-built, pull-only)
- Each agent has a dashboard you build by asking ("put my open PRs in a table,"
  "chart this month's Stripe revenue"). Modules: stat, list, table, markdown,
  chart, progress, timeline, actions, form, status, feed — on a drag/resize
  12-col grid with one-click Undo.
- **Safety invariant:** a dashboard NEVER runs an AI call on its own. Nothing
  polls/ticks; every card shows its age and only updates on **Refresh**, so it
  never silently spends tokens. Money-costing modules are labeled with cost;
  any shell-running module shows the exact command and waits for approval.

## 7. Sparks (interactive mini-apps, live in chat)
- Ask in plain English → the agent builds a real, self-contained **interactive
  mini-app** that renders live inline in chat in seconds (calculator, chart of
  your real data, sortable table, checklist, game).
- **Fully interactive + persistent (1.0.2):** buttons click, checkboxes toggle,
  and state **survives closing and reopening** — `localStorage` works normally
  and there is a `window.spark.get/set/all` API for structured JSON. State is
  saved to a jailed per-Spark store (`Sparks/<slug>/state.json`) via a narrow,
  host-mediated channel — great for to-do lists, trackers, saved settings.
- **Harness + theme sync (1.0.5):** fault-tolerant boot harness (`DOMContentLoaded` + try/catch + visible `__sparkErr` banner), seed-once blob lifecycle (no reload on live state writes), app-driven theme injection (`getAppTheme`/`wrapSparkHtml` + live `__sparkTheme` MutationObserver) — hover no longer fakes interactivity; buttons/inputs and light/dark/neutral/matrix + accent now match Settings in both Library and inline Chat. `spark_preview` auto-saves via the jailed broker to `Sparks/<slug>/index.html` + `spark.json` (live in chat AND Library on same slug, hot-swap on re-preview).
- **Seamless interactivity (1.0.7):** `sparkChrome.ts` runtime shims — dummy `getElementById` return for missing ids, auto `type="button"` on bare buttons, per-listener `EventTarget.addEventListener` wrap so one handler error shows a banner but doesn't kill siblings; helper `$` + `type="button"` recipe in `SPARKS_INSTRUCTIONS`. Sandbox stays opaque (`allow-scripts` only, `postMessage` KV only).
- Runs in a **locked-down sandboxed iframe** (opaque origin, `allow-scripts`
  only, no same-origin) — no file access, no network back to the machine/agent.
  Data is embedded at build time. Persistence is validated + jailed to the Spark
  and does **not** weaken the sandbox. Saves to a per-agent library; iterate by
  asking in chat.

## 8. Scheduling & teamwork
- **Schedules:** cron-style (daily/weekly at a time) + interval "heartbeat"
  turns; runs unattended while the app is open, with per-schedule fire/cost caps
  + a global pause kill-switch. Fire-now for testing.
- **Self-set wake-ups** (`task_continue`) and **inter-agent messages** with a
  budget/chain guardrail; the headless drainer runs recipient turns independent
  of any open chat pane.
- **Clean lifecycle (1.0.5):** `Cmd+Q` cleanly shuts down the daemon (no orphan `aygent` + no “quit unexpectedly” report); red-X hides the window instead of killing the process so schedules and the drainer keep running — Dock click reopens.

## 9. Save Points (rewindable everything)
- Every turn snapshots the agent's folder into a **Save Point** (a shadow git
  timeline per folder). Anything an agent does is rewindable — one botched tool
  call can't permanently destroy files.
- Timeline with undo/redo/rewind-to-here; configurable retention; purge. A Stop
  button always available mid-turn.

## 10. Onboarding (own your agent, one folder) — new in 1.0.5
- **3-step wizard:** Welcome → **Home** (pick or restore your AYGENT root folder; detects an existing root and shows agent/chat counts) → **Agent Setup** (name, icon, personality, model). Creates `<root>/<AgentName>/` + `context/` + `memory/` + `.aygent/` skeleton and flips the `root.json` pointer live (no restart; `writer::Db::repoint`). Works for first-run and new-machine restore.
- Replaces the old single-file picker + manual restart flow. The DB re-points live and the agent home is ready before the first turn.

## 11. Make it yours (themes)
- Four hand-built themes: Light, Dark, Neutral, Matrix. **Matrix + accent takeover (1.0.5):** accent replaces green everywhere (text/muted/faint/line/glow). New swatches **Royal blue #4169e1** + **Electric blue #00cafc** plus lighter purples. An **accent** color runs through the whole UI as lighting (retints outlines/shadows/glows), preset swatches or custom hex. Per-agent icons. The Dock icon persists the theme (via `NSWorkspace setIcon:forFile:`) and stays flat (no glow).
- Theme + accent follow you to **AYGENT Remote** and to **Sparks** (live sync).

## 12. Telegram (per-agent, optional) — new in 1.0.5
- **One bot per agent:** paste a BotFather token (stored in macOS Keychain under `telegram-bot-<agentId>`; never in DB/files). Per-agent `telegram_enabled`, `telegram_bot_username`, `telegram_allowed_chats` (CSV allowlist; empty = allow any) in `agent` table (`db.rs`/`repo.rs`). Inline validation via `getMe` shows the `@username` before enabling.
- **Polling + mailbox:** `telegram.rs` long-polls `getUpdates?timeout=25` per enabled agent (`spawn_one`/`spawn_all`), enqueues each inbound as `telegram:<chat_id>:<update_id>` into `mailbox` (deduped by origin), nudges the `drainer` — replies route back via `sendMessage` (clipped at 3500 chars). Backlog is skipped on boot (`max update_id + 1`). Allowlist is re-read live so edits apply immediately. ` /compact` and `/newsession` are handled locally in the pinned chat (no LLM stall) — compact summarizes the pinned `telegram-<id>` history, newsession wipes it.
- **UI:** per-agent Telegram card in `Agents.tsx` (enable, token field with Test, allowed chats, bot username); pinned `telegram-<id>` chat (always pinned, order 0) streams like any other conversation; `onboarding`/`agents_create` wiring + `AppHandle` spawn on enable.

## 13. AYGENT Remote (add-on, $4.99/mo)
- Use your local agents from **any browser you're logged into** (phone
  included). Pair the Mac once with an 8-char code.
- Live streamed replies, full history, same design/theme as the app.
- **End-to-end encrypted:** messages travel as encrypted envelopes through a
  relay that only ever sees ciphertext; nothing stored in the cloud. An
  online/offline toggle on the Mac is a real kill switch.
- Fully **optional** — the app is complete without it.

## 14. Security model (why "you own it" is real)
- **Jailed daemon:** the Node "brain" runs under a macOS Seatbelt profile that
  denies ambient file + exec. All file ops go through the privileged **path
  broker** (fail-closed, per-agent scope). All process spawns go through the
  **exec broker** (Pro Mode only, cwd-pinned, env-scrubbed).
- **Credentials** (provider keys, connection tokens) live in the macOS Keychain
  and are attached to outbound calls on the privileged side — the model/daemon
  never receives a raw token.
- **Local-first:** conversations, memory, and files live on the user's disk.
- Distribution: Developer ID signed + Apple-notarized + stapled (clean
  double-click install, no Gatekeeper wall).

---

## Changelog
- **1.0.16** (2026-09-09): **Video Editor tab + chat upgrades** — a new Video tab: agentic NLE with real playback, hardlink media import, timeline (linked V+A clips, razor/review lanes), transcript + Hyperframes captions/graphics overlays, color grade + LUTs, local neural dialogue cleanup (DeepFilterNet3, live blend) with per-selection Clean Audio, Premiere-style media bins, review notes the agent can see and act on, and canvas vision (`video_look`). Still new and rough around the edges — under active polish. Chat: **streaming side-by-side diff view** on every code edit (before/red left, after/green right, live +N/-N counts), **drag-and-drop file attach**, long-task **timer picker** (1/3/5/10/15 min), OpenCode-parity usage billing + accent context pill, per-agent Muse Spark variant knob. Fixes along the way: Muse image budget (no more 400s), sidebar overflow, Create-button clipping, Clean Audio static master + audible-only counts.
- **1.0.12** (2026-09-04): **Local-only setups no longer blocked by a phantom Anthropic-key check** — the chat "ready" gate was hardwired to `has_provider_key("anthropic")`, so an agent on a **local model** (or OpenAI/OpenRouter/Muse-only) saw "add an Anthropic key in Settings" and a disabled composer even though the backend needed no such key. The gate is now per-agent: `local` is always ready, cloud providers check **their own** key, and the hint names the real provider. Active-agent profile is re-read on screen change so switching provider in Settings takes effect immediately.
- **1.0.11** (2026-09-03): **Web search + Brave + Playwright** — `web_search` builtin (no-key DuckDuckGo search, wired on every provider path incl. local models and dashboard buttons); **Brave Search connector** (keyed web/news/image/video search); **Playwright MCP** one-click browser automation (isolated Chromium, provisioned Node, no system installs).
- **1.0.11** (2026-08-21): **Local models sized to your machine** — bump 1.0.10 → 1.0.11. **Real-fit context window** (`3baacee`): the fit predictor now counts weights + KV cache + overhead against the Metal working set, so a big model auto-caps to the context that actually fits instead of advertising 128k and dying with `Decode Error -3`; already-downloaded models retroactively auto-cap on next run. **Run up to RAM-pool size** (`9ac5f34`): offload-aware verdicts — a model bigger than the GPU working set but within total RAM gets a 🟡 partial verdict (some layers on CPU) and downloads instead of a hard ❌; new **Efficient** quant tier (IQ3/Q3-class) surfaces smaller quants on large repos; **q8_0 KV cache + flash attention** on the local path (~half the KV memory → longer usable context); safe compaction preserves the system prompt on local agents. **kv-trim fallback** (`d2181bf`): cache configs that can't trim a partial sequence (q8 KV) now clear + full re-decode instead of erroring the turn.
- **1.0.10** (2026-08-19): **Local models grow up** — bump 1.0.9 → 1.0.10. Fixes: SIGABRT crash on long local chats (n_batch sized to prompt, `86940db`); tool calls emitted inside an unclosed `<think>` rescued + executed (`5889490`); **Compact now works on local agents** (summary authored in-process by the local model, `e13e23a`); model-switch memory leak — old sessions/weights evicted so a smaller model no longer hits memory errors (`f6470e5`). Perf: **KV-cache reuse across turns** (persistent llama sessions — turn 2+ prefill near-instant, `42d8b90`); **partial GPU offload** (oversized GGUFs offload what fits instead of falling to all-CPU, `758afbb`). Features: **recall(query) memory tool + auto-injected memory (RAG) + few-shot tool examples** for local models (`791ad98`); HF search/lookup no longer blocks uncensored/abliterated models — your machine, your choice (`692fe34`).
- **1.0.9** (2026-08-18): **Muse Spark stall + FD jam fixes** — bump 1.0.8 → 1.0.9. Headroom 8192->32000 max_output_tokens; continue-spin guard + compact nudge.

- **1.0.8** (2026-08-18): **Sparks verified interactive** — staging hardening + WKWebView cache-bust verified (frontend_build_id hash in bust_webview_cache_on_version_change) so the `sparkChrome.ts` harness (dummy getElementById, auto type=button, per-listener EventTarget wrap + __sparkErr) actually ships; tip calculator and library Sparks now accept input + click in both inline Chat and Sparks tab without reload. Sandbox stays opaque (`allow-scripts` only, `postMessage` KV). Bump `1.0.7 → 1.0.8`.
- **1.0.7** (2026-08-18): **Sparks seamless interactivity** — fault-isolated handlers + auto `type="button"` so a single null `getElementById` or bare `<button>` no longer kills the whole Spark script; per-listener `EventTarget` wrap with `__sparkErr` banner + helper `$` shim (`sparkChrome.ts` `SPARK_RUNTIME`/`wrapSparkHtml`). Recipe now enforces `type="button"` + null-guarded `getElementById` (`lib.rs` `SPARKS_INSTRUCTIONS`). Sandbox stays opaque (`allow-scripts` only, `postMessage` KV only). Bump `1.0.6 → 1.0.7` (`c137750`).
- **1.0.6** (2026-08-17): **OpenAI/OpenRouter parallel tools 400 fix** — `build_openai_messages` now searches the full history for the matching assistant `tool_calls` (`out.iter().rev().any`) instead of only `out.last()`, so both parallel tool_results survive and no longer 400 with “must be followed by tool messages”. Bump `1.0.5 → 1.0.6` (`efe5ec3`).
- **1.0.5** (2026-08-17): **Onboarding v2 + 7-fix bundle + Sparks harness** — 3-step wizard (Welcome → Home with restore detection + counts → Agent Setup, `catalog.rs` curated families + `paths::init_agent_home`, live `Db::repoint` no-restart) (`ddb12b4`, `40f3708`); 7 fixes: hamburger history drawer (`Chat.tsx` absolute drawer), Matrix accent takeover (`4d813d6` — accent replaces green text/muted/faint/line/glow), Royal #4169e1 + Electric #00cafc swatches, inline Connections credential form, Save Points → “Checkpoint + timestamp”, clean quit (daemon `shutdown` on `RunEvent::ExitRequested`, red-X hides not kills, scheduler stays alive), Telegram per-agent (token in Keychain, `telegram.rs` `getUpdates` long-poll → `mailbox` → `drainer`, `telegram:<chat_id>` pinned chat + `/compact`/`/newsession`, `Agents.tsx` card) (`191ec1d` + `e9b1ac4`/`6db963d`), chat pin-to-true-bottom (`92a6e59`) + stutter removal (`3e09284`), Sparks harness pass + seed-once + theme sync (fault-tolerant boot + `spark.json` auto-save + `seedState` snapshot + `useSparkThemeSync` + jailed `state.json` bridge) (`5120e36`). Promoted `78fe848` (`tag: v1.0.5`).
- **1.0.4** (2026-08-13): **Per-agent GitHub PAT in shell** — Pro Mode `shell_run`/`shell_spawn` now injects the agent's Connections GitHub PAT as ephemeral `GITHUB_TOKEN` per-process (no global `osxkeychain` overwrite; host Keychain untouched). **Harness: uncap large file writes** to 64k with honest truncation (fixes 20KB `routes.ts` clip). Signed + notarized + stapled (16.9 MB DMG, `01bf1c9b Accepted`, `Notarized Developer ID`) → `masonleebuild/product-files/AYGENT-1.0.4-macOS.dmg` published via `publish-release.js` (release recorded, 1/1 owners emailed). Commits `2b796d5`, `edb84ac`, `9473288`, `9ea239d`, `2044884` (`tag: v1.0.4` reissued).
- **1.0.3** (2026-08-12): **Shared context is now discoverable + addressable.**
  Read-only mounts get a virtual `@shared/<label>/` namespace — an agent can
  `list_files("@shared")` to see shared folders, browse/read inside them, and
  reach a shared file even when its name collides with one of its own. The agent
  is also told in its prompt that it has shared access (previously a mounted
  folder was invisible — the agent was never told and could not enumerate it).
- **1.0.2** (2026-08-12): **Sparks are real interactive mini-apps** — fixed
  dead buttons/checklists (an opaque-origin `localStorage` throw was aborting the
  Spark script before any handler wired up) and added **jailed persistence** so
  state survives closing and reopening (`localStorage` + `window.spark` →
  `Sparks/<slug>/state.json`, host-mediated, sandbox unchanged).
- **1.0.1** (2026-08-11): Chat **context-window meter + running $ cost + per-turn
  tokens/cost** across all providers (Anthropic, OpenAI, OpenRouter, Muse, local);
  **in-chat context compaction** (⚡ Compact); **Muse (Meta)** surfaced as a
  selectable provider; **`github_read_file`** returns the full decoded file
  instead of truncated base64. (commits `d92f428`, `f11516a`, `6b1bd34`)
- **1.0.0** (2026-08-10): Signed/notarized launch — whoami tool, GFM tables,
  dashboards, sparks, remote.

_Last updated 2026-09-09 for 1.0.16. If you add a capability, add it here._
