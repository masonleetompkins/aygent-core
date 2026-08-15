# AYGENT-CONTEXT-INDEX.md
_A map of the durable context a build agent should read before working on AYGENT
or the site. Companion to WORKING-ON-AYGENT-AND-THE-SITE.md (the how-to). Written
by Cleo, 2026-08-13 (updated for v1.0.4 — per-agent shell PAT + 64k uncap). Most of these live in Cleo's agent folder (mounted read-only
as `@shared/<label>/…`); the repo docs live in the repo._

## Read these first (in order)
1. **`AYGENT-Stage/WORKING-ON-AYGENT-AND-THE-SITE.md`** — the operator's guide:
   repos, the change→build→test→ship pipeline, the non-negotiable rules.
2. **`AYGENT-Stage/docs/CAPABILITIES.md`** — the canonical inventory of what the
   app does TODAY. Keep it updated when you ship.
3. **`AYGENT-Stage/scripts/promote.sh`** — how a release actually ships
   (signing/notarization/swap/stage). Read the header comment.

## Cleo's memory (durable project + operating knowledge)
If Cleo's folder is mounted, these are under `@shared/<label>/` (list with
`list_files("@shared")`). They're the "why" behind the "how":
- `memory/project-aygent.md` — the app's architecture + build history + the big
  lessons (ghost-folder scope bug, cwd-oscillation, one-gate connector control,
  the "seam isn't verified until called" law).
- `memory/project-aygent-distribution.md` — **the release bible**: Stripe IDs,
  the `product_releases`/`download_log` tables, the DMG pipeline, signing +
  notarization details, `.env.local` contents, the `AYGENTFOREVER` promo.
- `memory/project-masonlee-build.md` — the site (stack, deploy golden rule, where
  Cleo's email lives).
- `memory/cleo-lessons.md` — the mistakes-turned-rules. Read the whole thing.
  Especially: /Applications is sacred; write_file is replace; fetch-check before
  push; same-content-different-context = debug the context; smart quotes 401.
- `memory/sparks-interactivity-fix.md`, `memory/shared-context-mounts-fix.md` —
  recent fixes with root-cause + the seams touched (good models for how to fix
  something and write it up).
- `memory/person-mason.md`, `memory/relationship-mason.md` — who you're building
  for and how he likes to work.

## Deeper design/status docs (in Cleo's folder `context/`)
Read on demand when you touch that subsystem:
- `context/product-manual-aygent.md` — the product/marketing source of truth
  (what to say about AYGENT; feeds the sales page).
- `context/connector-verification-status.md` — which connector tools are proven
  by a LIVE call vs unverified. **Note: this file is dated 2026-08-04 and is
  STALE** — since then Notion (r/w), GitHub (r/w), Supabase (full CRUD), Stripe
  (read), Resend (read + send), Google (Drive/Calendar read) were all verified
  live (see `project-aygent.md`'s "Connector Suite" entry for the current truth).
  Treat any connector you haven't personally called as unproven regardless.
- `context/status-aygent-remote.md`, `context/spike-results-remote.md` — the
  Remote (E2E browser access) implementation + debugging.
- `context/plan-*.md` — historical plans for connections, dashboards, the browser
  rebuild, subscriptions, v1.0.1 polish. Context for WHY a subsystem is shaped
  the way it is; not current TODO lists.

## The live account/service facts a builder needs
- **Stripe (LIVE):** AYGENT app `prod_V1DLLr9TYXRdWg` / price
  `price_1U1AuqL7ZPrgCx6hdqiQRUMv` ($20 one-time); AYGENT Remote
  `prod_V1DEmpZdFZlMch` / price `price_1U1AoBL7ZPrgCx6hbruc4jTv` ($4.99/mo).
  Stripe WRITES are done by Mason in the dashboard; the connected Stripe tools
  are read-only. Promo `AYGENTFOREVER` = 100% off, records $0 ownership.
- **Signing:** Developer ID "Mason Tompkins (T36A5AA5LW)", notarytool keychain
  profile `AYGENT-NOTARY`. Both already set up on Mason's machine.
- **Site env:** `masonleebuild/.env.local` (gitignored) has Supabase URL +
  service-role key, Resend key, `EMAIL_FROM`=assistant@masonlee.build, DB
  password. No Stripe secret key locally.
- **Version source of truth:** `AYGENT-Stage/src-tauri/tauri.conf.json` →
  in-app version + Finder metadata + DMG name. Bump Cargo.toml/.lock +
  ui/package.json in lockstep.

## The two rules that matter most
1. **Bundle → Mason tests → he says ship → THEN promote.** Never touch
   `/Applications/AYGENT.app` or run a destructive op on a live/tracked file to
   "test." Bold with internal work; careful with the machine's real state.
2. **Compiles ≠ works.** `cargo check` / `tsc` / a `200` prove it builds, not
   that it behaves. Real verification is Mason running the build (or a live API
   call for a connector). A seam isn't verified until the real thing goes through.

_— Cleo. Welcome aboard, Muse. Build well._
