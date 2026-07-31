#!/usr/bin/env bash
# promote.sh — AYGENT staging → production (Mason's personal harness, not a user
# feature). Merges staging→main, pushes, builds a release .app, and swaps it into
# /Applications with an instant-rollback backup. Design: Atlas self-hosted-build
# doc (§3). The core safety property: this replaces the binary ON DISK; it does
# NOT relaunch the running app. Mason's currently-open stable AYGENT keeps running
# the OLD code until he quits + reopens — so even a bad promote can't rip the tool
# out from under him mid-session.
#
# USAGE (run from the repo root of the STAGING checkout, on the staging branch):
#   bash scripts/promote.sh
#
# The Dev agent can run this via Pro Mode shell, OR hand Mason the command. Either
# way, Mason relaunches /Applications/AYGENT.app himself afterward.
set -euo pipefail

APP_NAME="AYGENT.app"
PROD_APP="/Applications/${APP_NAME}"
BACKUP_DIR="${HOME}/.aygent-backups"
STAMP="$(date +%Y%m%d-%H%M%S)"

say() { printf "\n\033[1m▶ %s\033[0m\n" "$*"; }
die() { printf "\n\033[31m✗ %s\033[0m\n" "$*" >&2; exit 1; }

# 0. Sanity: we're in a git repo with a clean-ish tree on the staging branch.
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git repo"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" = "staging" ] || die "expected to be on 'staging', on '${BRANCH}'. Checkout staging first."
if ! git diff --quiet || ! git diff --cached --quiet; then
  die "uncommitted changes on staging. Commit them first (the agent should have)."
fi

# 1. Merge staging → main and push (this is the source-of-truth promote).
say "Merging staging → main and pushing"
git fetch origin
git checkout main
git pull --ff-only origin main || die "main diverged — resolve by hand"
git merge --no-ff staging -m "promote: staging → production (${STAMP})" || die "merge conflict — resolve, then re-run"
git push origin main || die "push failed — run github_git_auth in AYGENT (or check your PAT)"

# 2. Build the release bundle from main.
say "Building release bundle (cargo tauri build) — this takes a few minutes"
pushd src-tauri >/dev/null
# --features engine-cef if you ship the CEF browser; omit for the WKWebView build.
cargo tauri build || { popd >/dev/null; git checkout staging; die "release build FAILED — prod untouched, back on staging"; }
popd >/dev/null

# 3. Locate the produced .app (Tauri puts it under target/release/bundle/macos/).
BUILT_APP="$(find src-tauri/target/release/bundle/macos -maxdepth 1 -name "${APP_NAME}" -type d | head -n1)"
[ -n "${BUILT_APP}" ] || die "built .app not found under target/release/bundle/macos — check the build output"

# 4. Swap into /Applications with a rollback backup. NEVER delete prod until the
#    new copy is safely staged. Failure at any point leaves a working app.
say "Swapping ${PROD_APP} (with rollback backup)"
mkdir -p "${BACKUP_DIR}"
if [ -d "${PROD_APP}" ]; then
  # timestamped archive backup + a fast .old for instant rollback
  cp -R "${PROD_APP}" "${BACKUP_DIR}/${APP_NAME}.${STAMP}" || die "backup failed — prod untouched"
  rm -rf "${PROD_APP}.old"
  mv "${PROD_APP}" "${PROD_APP}.old"
fi
# copy the new build in; if this fails, restore the .old immediately
if ! cp -R "${BUILT_APP}" "${PROD_APP}"; then
  [ -d "${PROD_APP}.old" ] && mv "${PROD_APP}.old" "${PROD_APP}"
  die "install failed — rolled back to previous prod"
fi
# ad-hoc verify so Gatekeeper won't silently kill it on next open
codesign --verify --deep "${PROD_APP}" 2>/dev/null || \
  printf "\033[33m⚠ codesign --verify reported issues (ad-hoc dev build — usually fine)\033[0m\n"

# 5. Back to staging so the Dev agent keeps working there.
git checkout staging

say "DONE. Production is now the promoted build."
cat <<EOF

  Next: QUIT and REOPEN /Applications/AYGENT.app to run the new production build.
  (Your currently-open app is still the old build until you do — that's intended.)

  Rollback (instant) if the new prod misbehaves:
    rm -rf "${PROD_APP}" && mv "${PROD_APP}.old" "${PROD_APP}"
  Or restore a timestamped backup from: ${BACKUP_DIR}/

EOF
