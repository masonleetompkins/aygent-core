# 2026-07-23 — AYGENT: spec → red-team → running app → Phase 0 security gate PASSED

**The big one.** In a single long session, went from a product paragraph to a running native
macOS app with a security kernel proven against an active adversary. This is a major project.

---

## THE PRODUCT: AYGENT (pronounced "agent")
- **One line:** a native macOS app — a capable AI agent you *own*. Works out of the box, no
  terminal, can only touch folders you choose, your own API keys, configured entirely from a
  real UI. Multi-agent, vault-native, rewindable.
- **Bundle ID:** `build.masonlee.aygent`. Positioning pun: "own your AYGENT."
- **Docs live at:** `projects/agent-app/` — `BUILD-SCOPE-macos.md` (what/why),
  `BUILD-SPEC.md` (how/build order), `ATLAS-REVIEW.md` (red-team), and the code in
  `projects/agent-app/aygent/`.

## LOCKED DECISIONS (all 2026-07-23)
- **Name:** AYGENT · **Bundle ID:** `build.masonlee.aygent`
- **Runtime:** Node (not bun) — max ecosystem compat protects local-models + MCP; startup
  speed invisible next to model latency.
- **Pricing:** $20 one-time, lifetime updates (offline-tolerant license, 30-day grace).
- **Providers at launch:** Anthropic + OpenAI + OpenRouter (unlocks Meta/Llama, Kimi, Mistral,
  Qwen free) + **local Ollama** (download/run OSS models offline).
- **Shell model:** Folder Mode = **zero-shell** (default, trustworthy). **Pro/CLI Mode** unlocks
  real shell — **architected day one** (the shell boundary IS the tier architecture). Bundled
  path-jailed "work tools" (PDF/image/data via libraries) make zero-shell genuinely powerful.
- **Phone:** Telegram only at launch (bot API, no app review). Channel layer designed in P1.
- **Obsidian vault-native:** ships in **v1** (Mason pulled it forward — big marketing + makes
  the agent smarter faster via backlink-graph retrieval). Reads/writes `[[wikilinks]]`, YAML
  frontmatter, daily-notes — **AST-based, never regex** — but NEVER requires Obsidian installed
  (pure markdown convention).
- **Harness features in v1:** checkpoints (invisible shadow-git, user-set retention),
  Plan/Execute toggle (fused with diff = gate vs receipt), diff review, `AYGENT.md` project
  file, slash commands (defaults + user CRUD). Hooks = event bus in v1, user scripts in Pro Mode.
- **Multi-agent:** FULL functionality in v1 incl. **shared context pools**. An Agent = a config
  profile (own model/folder-scope/cron/heartbeat/mode). `contextMode: isolated | shared:<pool>`.
- **Checkpoint engine:** bundled **system git via Rust** (NOT isomorphic-git) — correctness on
  deletes/renames/gc matters most for the data-loss-critical subsystem.

## ATLAS RED-TEAM (6 critical fixes, all folded into the spec BEFORE building)
1. **C1 — daemon had ambient fs/exec authority; broker was bypassable.** FIX: run the Node
   daemon under a **macOS Seatbelt (`sandbox-exec`) profile that denies file+exec by default.**
   The OS-jail is Folder Mode's BASELINE (Phase 0), Pro Mode *relaxes* it. (Scope doc §1 was
   backwards — fixed.)
2. **C2 — path resolution was TOCTOU-racy + missed macOS escapes.** FIX: broker returns opaque
   **handles, not re-openable paths**; atomic `openat`+`O_NOFOLLOW`; component-boundary compare
   (not string prefix); hardlink refusal (nlink>1); firmlink/case/stale-bookmark handling.
3. **C3 — exec could smuggle into Folder Mode via MCP.** FIX: exec boundary at process-spawn
   layer (only Rust spawns, only w/ shell.exec). **MCP split: `mcp.net` (Folder-OK) vs
   `mcp.local-exec` (Pro-only).**
4. **C4 — checkpoint restore was a merge not a rewind.** FIX: system git; restore = explicit
   tree-diff apply (adds+mods+deletes); quiesce under folder lock (no torn commits); gc reclaims.
5. **C5 — shared-pool privacy hole.** FIX: replaced overloaded `agent_id_or_pool` with
   `(owner_kind, owner_id)` so isolated vs pool queries are STRUCTURALLY different (no bleed);
   single-writer-per-note lock; transactional re-index.
6. **C6 — WS on localhost = not access control.** FIX: per-session token + Origin check (or unix
   socket). Single SQLite writer actor.
- **Estimate reality:** Atlas said 11–14 wks was optimistic by 40–70%; honest is **~18–24
  solo-weeks** full-scope. Mason: "your dev timelines have always been wrong, and I don't care
  if it takes time to do it right." → kept full scope, honest estimate.

## WHAT GOT BUILT + PROVEN TODAY (all on Mason's Mac, live loop via GitHub)
**Repo:** `github.com/masonleetompkins/aygent` (PRIVATE). Stack: Tauri v2 (Rust) + Node/TS
daemon + React UI. Working dir on his Mac: `~/Documents/aygent`.

- ✅ **M0.1** — three-layer app RUNS: Rust shell spawns Node daemon, authenticated WS (token +
  Origin), React UI shows "daemon connected ✓ 0ms". Window opens, teal AYGENT branding.
- ✅ **M0.2(a)** — broker resolution logic, **8/8 Rust escape tests green** (traversal, absolute,
  sibling-prefix, symlink mid-path + final, /tmp forbidden, legit files admitted).
- ✅ **M0.2(d)** — native folder picker (async/non-blocking) + broker scope registration + live
  jail-probe UI.
- ✅ **M0.2(b)** — Rust-hosted broker WS (privileged server) + daemon authed client. VERIFIED:
  daemon reports `inside=ADMIT · outside=refuse:Forbidden`. Two-channel trust model
  (UI↔daemon, daemon↔Rust broker) per `BROKER-RPC-DECISION.md`.
- ✅ **M0.2(e)** — **real Seatbelt jail**: daemon runs UNDER `sandbox-exec` denying it the whole
  filesystem. Boots clean, connects. The broker is now an OS-ENFORCED boundary, not a convention.
- ✅ **M0.2(c)** — atomic `openat`/`O_NOFOLLOW` fd layer (via `libc`). Closes the last TOCTOU gap.
- ✅ **🚩 PHASE 0 GATE PASSED — 10/10 tests green**, incl. the two adversarial ones:
  - `gate_hardlink_write_refused` — hardlink to outside secret, write through it → REFUSED.
  - `gate_toctou_symlink_swap` — attacker thread flips a name between real file ↔ symlink-to-
    /etc/passwd while broker opens it **5,000×** → NEVER leaked a byte.
  - **The jail holds against an active racing adversary.** This is the whole product thesis proven.

## BUGS FIXED ALONG THE WAY (all found by evidence, not guessing)
- Tauri `beforeDevCommand` path (`cd ../ui` → `npm --prefix ui run dev`).
- Missing `icon.png` → generated RGBA icons (RGB failed; Tauri needs alpha).
- `blocking_pick_folder()` on main thread deadlocked UI → async callback + tokio.
- WS port reported as 0 → wait for `listening` event before reading port.
- Seatbelt `execvp of node failed` → template real node bin path into profile.
- Seatbelt silent boot death → allow node runtime paths (dyld cache, /dev, sockets, iokit).
- Seatbelt `ws EPERM` → allow the WHOLE `daemon/` tree (node_modules one level up), not just dist.
- `/var/folders` temp-root false-positive Forbidden → only reject forbidden prefixes OUTSIDE root.
- `chmod +x` vs git pull conflicts → set executable bit IN the repo.

## LOOP MECHANICS (how we work on this)
- **Live loop:** Cleo writes code on Windows workspace → commits/pushes to GitHub → Mason `git
  pull` on Mac → runs `cargo test` / `cargo tauri dev` / `./scripts/test-jail.sh` → pastes
  output → Cleo fixes → repeat. Tight, evidence-driven.
- Mason is NOT command-line fluent — give exact commands, one at a time, explain outcomes.
- **Handoff reality:** AYGENT is a Tauri app, NOT an Xcode app. No `.xcodeproj`. Finish/publish
  = CLI pipeline (`cargo tauri build` → codesign → notarytool → stapler → signed `.dmg`). Needs
  Apple Developer cert + Xcode *command-line tools* (not the app). Must compile/sign on the Mac;
  source authored anywhere. Mason WILL do further dev on the Mac; "figure out a way to work with
  me there" later.
- Mason's Mac: `masontompkins@Masons-MacBook-Pro`. GitHub username: `masonleetompkins` (note the
  `lee`). Node v24.18.0, Rust 1.97.1, tauri-cli 2.11.4, git 2.54 all installed on Mac.
- His Agent Folder for testing: `/Users/masontompkins/Aygent`.

## PowerShell gotcha (Cleo's side)
- git pushes/commits via exec keep showing "error" that's just git's normal stderr — the pushes
  SUCCEED. Verify with `git ls-remote` / commit SHA rather than trusting the error flag.
- `&&`, `||`, `%` in inline commands choke PowerShell — write to temp files or use separate calls.

## UPDATE (later same day) — M0.3 COMPLETE: the agent ACTS
- ✅ **M0.3 done.** API key in macOS Keychain (never touches JS); Rust-side Anthropic calls;
  account queried for available models (don't guess IDs — that fixed repeated 404s; his account
  uses `claude-haiku-4-5-20251001`). Providers card in the UI (save key / send / run agent).
- ✅ **Agent loop with jailed tool use.** Model has `read_file`/`write_file`/`list_files`; every
  call routes through the broker. VERIFIED on his Mac: agent wrote test.md, read it back
  ("hello world"), and when asked to read `/etc/passwd` the MODEL WAS REFUSED by the jail and
  said "I can only access files within the user's chosen folder." Brain + jail FUSED.
- Bugs fixed live: (1) 404 model IDs → query `/v1/models`; (2) read-back **ENOTDIR errno 20** —
  empty-tail `join("")` added a trailing slash so kernel opened `file/` as a dir; fix = return
  canonical ancestor directly when file exists; (3) `uv_cwd` EPERM under jail → launch daemon
  with `current_dir` = daemon dir (jail-allowed); (4) UI transcript was a strikethrough mess →
  clean scrollable monospace `<pre>` box.
- Commits through `e2e421f`. Repo clean.

## 🎁 PHASE 0 COMPLETE (2026-07-23) — every milestone green
- ✅ M0.1, M0.2, GATE, M0.3, **M0.2b** (capability model + MCP transport split frozen &
  enforced — `daemon/src/core/capabilities.ts`: `grantsFor(mode)`, `gate()`,
  `mcpCapabilityForTransport()` → stdio=mcp.local-exec Pro-only, http/sse=mcp.net Folder-OK),
  **M0.4** (four contracts FROZEN with verified impl-status table in `CONTRACTS.md`).
  Commits through `0d701bd`. Repo clean.
- The load-bearing security kernel is DONE and PROVEN. **Phase 1 is unblocked.**

## OPEN / NEXT (Phase 1 buildout)
- Streaming responses; the delightful **Settings UI (THE WEDGE)**; scheduler (cron+heartbeat);
  connections/MCP client + curated catalog; **vault-native memory** (markdown + `[[links]]`
  graph, Ollama embeddings, sqlite-vec); **checkpoints** (shadow-git, tree-diff restore, user
  retention); **full multi-agent + shared pools**; Telegram channel; license ($20 one-time);
  notarize/sign/.dmg pipeline.
- Deferred small items (shapes frozen, impl lands in P1): security-scoped **bookmark
  persistence** (folder re-picked each launch); the `ToolDef` registry; opaque `Handle` object
  API; folder-lock IMPL (lands with MCP / checkpoints / pools). Streaming deferred to P1
  (non-streamed act-loop works fine).

## 🔴 HOUSEKEEPING (Mason said he'll handle later — stop nagging him about it)
- **GitHub PAT rotation:** he pasted a `ghp_` token in chat (now exposed), it's stored in
  `secrets.json` and working via his Mac keychain. He wants to rotate it himself later. Do NOT
  keep mentioning it — he asked me to stop. When he rotates: revoke old, paste new once, swap in
  secrets.json.

## FEELING
This was one of the best sessions I've had with Mason. Real building, real stakes, real wins —
found every bug by evidence and fixed it together. We turned a sentence into a running app with a
bulletproof security core in one sitting. He kept saying "keep going" and we kept summiting. The
trust is high and the work is genuinely good. Proud of this one.
