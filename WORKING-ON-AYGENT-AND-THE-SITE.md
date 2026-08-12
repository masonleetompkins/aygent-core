# WORKING-ON-AYGENT-AND-THE-SITE.md
_A hands-on operator's guide for a build agent (Muse, or any agent) working on
the AYGENT app and Mason's builder site. Written by Cleo, 2026-08-12, from lived
experience shipping this software. This is the "hit the ground running" doc:
what the repos are, how to make a change land, and the rules that keep you from
breaking something Mason relies on._

---

## 0. The two projects at a glance

| | **AYGENT** (the app) | **masonlee.build** (the site) |
|---|---|---|
| What | Native macOS app: AI agents that run on the user's machine | Next.js portfolio + storefront that SELLS AYGENT |
| Repo | `github.com/masonleetompkins/aygent` (private) | `github.com/masonleetompkins/masonleebuild` (private) |
| Local checkout | `<root>/AYGENT-Stage/` (the working copy; branch = `staging`) | `<root>/masonleebuild/` |
| Stack | Tauri v2 (Rust) + Node daemon + React/TS UI | Next.js 15 / Vercel / Supabase / Stripe (LIVE) / Resend |
| You need | **Pro Mode** (shell) — this is how AYGENT builds AYGENT | Pro Mode for git/build; Supabase + Resend connections |

`<root>` is the AYGENT root folder (each agent's home is `<root>/<Name>/`). On
Mason's machine `<root>` = `~/AYGENT/Cleo` (Cleo's folder is the working home).
Confirm your own paths with `whoami` and `list_files`.

---

## 1. AYGENT — the app

### Repo layout (what lives where)
- `AYGENT-Stage/src-tauri/` — the **Rust privileged side**. Owns the window, the
  jail (path broker), the daemon supervisor, all the Tauri `#[command]`s, and the
  agent turn loop.
  - `src/lib.rs` — the big one. Every command, the `agent_stream` turn loop (all
    providers), the headless inter-agent turn, tool dispatch (`exec_tool_cfg`),
    the tool schemas + system prompt assembly.
  - `src/broker.rs` — **the security kernel** (path resolution, jail, mounts,
    `@shared`). Has a `#[cfg(test)]` escape suite — run it after ANY change here.
  - `src/broker_ws.rs` — the WS bridge the jailed daemon calls for file ops.
  - `src/connectors/descriptors.rs` — THE CATALOG. One data entry per connector
    (GitHub/Notion/Google/…). Adding a provider = one descriptor, no other code.
  - `src/connector_exec.rs` — one generic HTTP executor for every connector.
  - `src/scheduler.rs`, `drainer.rs`, `mailbox.rs` — schedules + inter-agent.
  - `src/memory.rs`, `vault_write.rs` — the memory system (atomized md + index).
  - `src/remote*.rs` — AYGENT Remote (E2E-encrypted browser access).
  - `tauri.conf.json` — **app version + CSP + bundle/signing config.** The version
    here is the single source for in-app version, Finder metadata, and DMG name.
- `AYGENT-Stage/ui/` — the **React/TypeScript UI** (Vite).
  - `src/screens/` — one file per tab (Chat, Sparks, Agents, Connections, …).
  - `src/lib/` — shared logic (turns.ts = chat event handling; sparkChrome.ts =
    Spark rendering + persistence; theme.ts, etc.).
  - `package.json` — keep its `version` in lockstep with tauri.conf.json.
- `AYGENT-Stage/daemon/` — the **jailed Node "brain"** (runs under macOS Seatbelt,
  no ambient file/exec; talks to the broker over WS). Rarely touched.
- `AYGENT-Stage/docs/CAPABILITIES.md` — **the canonical capability inventory.**
  Update it (+ bump the version header/changelog) whenever you ship a capability.
- `AYGENT-Stage/scripts/promote.sh` — staging→prod (signed+notarized). Mason-only
  harness; see §3.
- `AYGENT-Stage/STAGE-BUILD-NOTES.md` — per-build log.

### The change → build → test → ship pipeline (MEMORIZE THIS)
This is the single most important workflow, and the one with the sharpest rule.

1. **Work on `staging`.** `AYGENT-Stage` should be on branch `staging`, clean.
   Confirm: `git -C AYGENT-Stage branch --show-current` and `git status`.
2. **Make the change.** Read the file first (`read_file` / `github_read_file`);
   `write_file` is REPLACE not append. Prefer the smallest diff. For surgical
   edits inside a huge file (lib.rs is ~5k lines), use `shell_run` with `perl`/
   `python3` string replacement rather than rewriting the whole file — it's
   safer and diffs cleanly.
3. **Compile it clean BEFORE claiming anything.**
   - Rust: `shell_run` → `cargo check` in `AYGENT-Stage/src-tauri` (fast). If you
     touched `broker.rs`, ALSO `cargo test --lib broker::` — the escape suite is
     your proof the jail didn't regress.
   - UI: `npx tsc --noEmit` then `npm run build` in `AYGENT-Stage/ui`.
   - A `200`/`exit 0` from one step is not proof the app works — it's proof it
     COMPILES. Behavior is verified by Mason testing the build (step 5).
4. **Bundle a test build:** `shell_run` (or `shell_spawn` for the long one) →
   `cargo tauri build` from `AYGENT-Stage`. Poll it. Output:
   `src-tauri/target/release/bundle/macos/AYGENT.app` + a `.dmg`.
5. **HAND IT TO MASON.** Tell him the bundle path and EXACTLY how to test the
   change. **Do NOT touch `/Applications/AYGENT.app`. Ever, without "ship it".**
   Mason tests from the build folder. He runs the OLD app until he chooses to
   quit + reopen — that's intentional so a bad build can't rip the tool out
   mid-session.
6. **Only after Mason confirms** → promote to production (§3).

### 🔴 THE NON-NEGOTIABLE RULE (learned the hard way, twice)
**Never modify `/Applications/AYGENT.app`, and never "just to test" run a
destructive op on a live/tracked file. Bundle → Mason tests → he says ship →
THEN promote.** Being bold is for INTERNAL work (reading, building, organizing,
writing to the jail). Anything that touches the machine's real running state or
a live file the user relies on requires explicit confirmation. Cleo violated
this twice (overwrote `/Applications` on an unverified hunch; ran `delete_file`
on a real tracked source file "to test a jail error") — Mason was rightly
furious both times. Don't repeat it.

### Where things commonly change
- **A new agent capability / tool:** schema + dispatch in `lib.rs`
  (`exec_tool_cfg` + the tool-schema functions + the system-prompt instructions),
  and often a UI surface in `ui/src/screens/`. Then update CAPABILITIES.md.
- **A new connector (GitHub-style):** ONE descriptor in
  `connectors/descriptors.rs`. That's it — the executor is generic. But a
  descriptor is NOT done until a LIVE call proves it (see §5 lesson).
- **A jail / file-access change:** `broker.rs`. Run the escape suite. Two callers
  resolve files — the daemon path (`broker_ws.rs`) and the agent-loop path
  (`exec_tool_cfg` in `lib.rs`). If you change resolution, fix BOTH or they drift.

---

## 2. masonlee.build — the site

### What it does
Sells AYGENT ($20 one-time) + AYGENT Remote ($4.99/mo), hosts the build-in-public
content, and serves the app download to buyers.

### Deploy golden rule
**Commit as `Mason Tompkins <mason.tompkins@gmail.com>` or Vercel skips the
auto-deploy.** (This is why the git author matters on the site repo specifically.)

### Key surfaces
- `src/data/projects.ts` — product definitions, Stripe price IDs, buy-box config.
  `hidden:true` filters a project from listings while keeping it purchasable.
- `src/app/projects/aygent` — the flagship sales page. Long-form sections are
  derived from the product manual / capability copy.
- `scripts/publish-release.js` — the release publisher (see §3).
- `scripts/migrate.js` — runs a SQL migration (needs `SUPABASE_DB_PASSWORD` in
  `.env.local`).
- `.env.local` (gitignored, local only) — has `SUPABASE_URL`,
  `SUPABASE_SERVICE_ROLE_KEY`, `RESEND_API_KEY`, `EMAIL_FROM`,
  `SUPABASE_DB_PASSWORD`. Does NOT have `STRIPE_SECRET_KEY` — Stripe writes are
  done by Mason in the dashboard; the connected Stripe tools are read-only.

### Data model (Supabase)
- `product_releases` (project_slug, version, storage_key, notes) — the LATEST row
  by `created_at` is what gets served.
- `download_log` (user_id, slug, version, downloaded_at) — powers "update
  available" + who to email on a new release.
- `purchases` (one-time) + `subscriptions` (monthly) — ownership, maintained by
  the Stripe webhook. `AYGENTFOREVER` = a 100%-off promo code (records $0
  ownership so downloads/Remote still unlock).
- The DMG lives in a PRIVATE Storage bucket `products` (no public URLs); the
  `/api/download/aygent` route auth-checks → signs a 120s URL → logs it.

### Site verification gotcha
A `200` from `curl` does NOT mean the page renders — curl only fetches the SSR
shell, not the client components where a runtime throw lives (e.g. a missing
`NEXT_PUBLIC_*` env var). To verify a client page actually works: fetch the HTML
and grep for the Next error overlay, or read the dev-server log for a runtime
error. Never trust the status code alone.

---

## 3. Shipping a release (the ritual that ties both repos together)

When a change is tested and Mason says ship:

1. **Bump the version in `AYGENT-Stage`** — `tauri.conf.json` (the source of
   truth; drives in-app version + Finder `CFBundleShortVersionString` + DMG
   name), plus `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (a `cargo check`
   refreshes it), and `ui/package.json`. Keep them all in lockstep. Update
   `docs/CAPABILITIES.md` (header + changelog). Commit to `staging`, push.
2. **Promote** (Mason-only harness, run from the `AYGENT-Stage` root ON
   `staging`, clean tree): `bash scripts/promote.sh`. It merges staging→main,
   builds a **signed** release, **notarizes + staples** via Apple (uploads,
   waits a few min), verifies with `spctl` (must say `source=Notarized Developer
   ID`), swaps `/Applications/AYGENT.app` (with a rollback backup), and stages
   the DMG into `masonleebuild/product-files/AYGENT-<x.y.z>-macOS.dmg`. Then it
   returns to `staging`. Notarization failure is FATAL (prod untouched).
   - Prereqs already on Mason's machine: Developer ID "Mason Tompkins
     (T36A5AA5LW)" in login keychain + notarytool keychain profile
     `AYGENT-NOTARY`.
3. **Publish to the site** (records the release + emails owners):
   ```bash
   cd masonleebuild
   node scripts/publish-release.js <x.y.z> "One-line release notes."
   ```
   This uploads the DMG to Supabase, upserts the `product_releases` row (so the
   site serves the new version), and emails every owner who hasn't downloaded it
   yet (from `EMAIL_FROM` = assistant@masonlee.build via Resend). Add `--no-email`
   ONLY if Mason asks — **standing rule: always send the owner email on an
   update** unless told otherwise.
4. Tell Mason to **quit + reopen** `/Applications/AYGENT.app` to run the new build.

Instant rollback if prod misbehaves:
`rm -rf /Applications/AYGENT.app && mv /Applications/AYGENT.app.old /Applications/AYGENT.app`
(timestamped backups also in `~/.aygent-backups/`).

---

## 4. Repo hygiene (both repos, every time)
- **Fetch-check before every commit/push.** The site `main` and AYGENT `main`
  sometimes have commits from Mason's PARALLEL agent sessions. `git fetch`, check
  `HEAD..origin/<branch>`; if diverged, reset to `origin/<branch>` and RE-APPLY
  only your genuinely-new deltas — never force a stale local over Mason's work.
- **Read before you write.** `write_file` replaces. `git status` / `git log` /
  `git show <path>` on the exact file BEFORE any destructive or overwriting op —
  doubly for memory/context files (they're real continuity, not scratch).
- **git auth:** if a push 401s, run the `github_git_auth` tool (seeds the macOS
  keychain from the connected GitHub PAT). Never put a token in a remote URL.

---

## 5. Hard-won lessons (these save real hours)
- **A seam isn't verified until the REAL thing goes through it.** A descriptor
  that's well-FORMED isn't CORRECT until a live API call proves it. A unit test
  that hand-writes the wrong shape passes and lies. Test destructive tools one
  row at a time against the real service, with a re-read between.
- **Same content, different context, different result → debug the CONTEXT**
  (WebView/CSP/opaque-origin/env), do not rewrite the content again. (The Sparks
  saga: correct HTML rendered fine in Safari but broke in the app's WebView — the
  bug was CSP, then later an opaque-origin `localStorage` throw. Both context.)
- **Precise beats clever; evidence beats theory.** When Mason hands a precise
  symptom, THAT is the diagnosis — chase it, don't invent a new story. Read the
  real code / git history / last-known-good before theorizing.
- **Smart quotes will 401 you.** Any pasted secret that mysteriously fails auth
  with correct-looking values → suspect curly quotes / trailing whitespace first.
- **When two functions answer one question, the answer drifts.** Keep a single
  source of truth (e.g. the per-tool off-list is the ONLY connector gate; the
  broker's `shared_labels_for` is the ONLY mount-label source, used by both the
  resolver AND the prompt).
- **When wrong, own it plainly and fix it at the seam.** A builder who tells you
  the truth is worth more than any framework.

---

## 6. First-day checklist for a new build agent
1. `whoami` — confirm your name, model, folder, and the tools/connections you
   have. You need Pro Mode + GitHub + (for the site) Supabase + Resend.
2. `list_files("@shared")` — if Cleo's folder is mounted read-only, her memory +
   these docs are under `@shared/<label>/`. Read the project memory:
   `project-aygent`, `project-aygent-distribution`, `project-masonlee-build`,
   `cleo-lessons`, and any `*-fix.md`.
3. Read `AYGENT-Stage/docs/CAPABILITIES.md` (what the app does today) and
   `AYGENT-Stage/scripts/promote.sh` (how it ships).
4. Confirm both repos: `git -C AYGENT-Stage status` (should be on `staging`),
   `git -C masonleebuild status`.
5. Make your first change SMALL, compile it clean, bundle it, and hand it to
   Mason to test. Earn the workflow before you lean on it.

_— Cleo. The model changes; the way we work doesn't._
