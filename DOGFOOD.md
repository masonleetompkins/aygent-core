# DOGFOOD.md — Using AYGENT to build AYGENT (Mason's personal harness)

_This is NOT a shipping user feature. It's Pro Mode = "AYGENT with OpenClaw-grade
power," which you, as creator, use as your daily harness to build anything —
including AYGENT itself. Design: `projects/aygent/PRO-MODE-SHELL-PLAN.md` + Atlas
self-hosted-build doc._

## The one rule that keeps you safe
**The app hosting the Dev agent is never the app being rebuilt.** The Dev agent
edits source and *produces* a binary; it never `cargo tauri dev`s itself into
existence and never launches a GUI. Launching a new build is a deliberate act you
do yourself. This is enforced in the exec broker (argv denylist), not just by
prompt — the agent literally can't run `cargo tauri dev` or `open`.

---

## Step 0 — Build your first stable `AYGENT.app` (one time)

You don't have an installed `.app` yet — today it's `cargo tauri dev`. Produce the
real bundle and install it:

```bash
cd ~/Documents/aygent
git pull
cd src-tauri
cargo tauri build            # add --features engine-cef if you ship the CEF browser
```

The bundle lands at:
`src-tauri/target/release/bundle/macos/AYGENT.app`

Install it:
```bash
cp -R src-tauri/target/release/bundle/macos/AYGENT.app /Applications/
```

`/Applications/AYGENT.app` is now your **stable daily driver** — the app that
hosts the Dev agent. From here you rarely touch the terminal.

---

## Step 1 — Set up the Dev checkout (one time)

Keep prod (`main`) and dev (`staging`) from colliding by using a **separate
checkout** on a `staging` branch as the Dev agent's jail:

```bash
mkdir -p ~/AygentDev
git clone https://github.com/masonleetompkins/aygent ~/AygentDev/aygent
cd ~/AygentDev/aygent
git checkout -b staging
git push -u origin staging      # first time; needs push auth (Step 2)
```

Then in **stable AYGENT.app**: create a Pro Mode agent (e.g. **Cleo Dev**),
folder = `~/AygentDev/aygent`, toggle **⚡ Pro Mode** on.

---

## Step 2 — Authenticate git push/pull (one time per machine)

1. In AYGENT → **Connections** → connect GitHub with a PAT that has `repo` scope.
2. That's it — the app seeds the macOS Keychain git credential helper via the
   `github_git_auth` command (called from Connections). From then on `git push`
   and `git pull` work in any checkout with **no token in the repo and no token
   in the agent's shell env** (the child-env scrub can't leak what isn't there).

Verify (in the Dev agent's chat): _"run `git pull` and tell me what happened."_

---

## The daily loop (this is the whole thing)

You're in **stable `/Applications/AYGENT.app`**, talking to the **Dev agent**
whose folder is `~/AygentDev/aygent` on `staging`:

1. **Ask for a change.** _"Pull latest, then fix the browser tab-title bug."_
2. The agent works: `git pull` → edits source (jailed file tools) →
   `cargo build` → reads the **digest** (exit code, error/warning counts, the
   key `error[...]` lines, a short tail — never the 10k-line raw log) → fixes →
   rebuilds green → commits → `git push` (to `staging`).
3. **Produce a staging build to eyeball.** The agent runs
   `cargo tauri build`, which yields
   `~/AygentDev/aygent/src-tauri/target/release/bundle/macos/AYGENT.app`.
   Have it copy that to a stable spot:
   ```bash
   cp -R ~/AygentDev/aygent/src-tauri/target/release/bundle/macos/AYGENT.app ~/AYGENT-Staging.app
   ```
   **You** open `~/AYGENT-Staging.app` yourself (a second Dock icon) and check it.
   The agent CANNOT open it — if staging crashes on launch, your stable app is
   untouched, still hosting the agent that will fix it.
4. **Promote when staging is good.** The agent runs (or hands you) the promote
   script from the staging checkout:
   ```bash
   cd ~/AygentDev/aygent && bash scripts/promote.sh
   ```
   This merges `staging → main`, pushes, builds a release bundle, and swaps it
   into `/Applications/AYGENT.app` **with a rollback backup**. It replaces the
   binary on disk — it does NOT relaunch your running app. Your currently-open
   stable app keeps running the old code until you **quit and reopen**.

---

## Rollback (the agent knows this cold)

**Binary (instant)** — if the new production build misbehaves:
```bash
rm -rf "/Applications/AYGENT.app" && mv "/Applications/AYGENT.app.old" "/Applications/AYGENT.app"
```
Or restore a timestamped backup from `~/.aygent-backups/`.

**Source:**
```bash
git checkout main && git reset --hard origin/main
```

---

## What the agent CAN and CANNOT do (Pro Mode)

| Can | Cannot (blocked at the exec broker) |
|---|---|
| `git pull` / `git push` / `git status` / `git commit` | `cargo tauri dev` (opens a window) |
| `cargo build` / `cargo tauri build` / `cargo test` | `open` / `open -a` any `.app` |
| `npm install` / `npm run build` / `tsc` | `cargo run` (can open the app window) |
| any non-GUI command on your PATH, cwd pinned to the folder | anything outside the Agent Folder |

Secrets: your API keys + the broker token are **never** inherited by a spawned
process (env is scrubbed to a whitelist). The git PAT lives in the macOS Keychain
via git's own credential helper, not in the env or the repo.
