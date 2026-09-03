#!/usr/bin/env bash
# promote.sh — AYGENT staging → production (Mason's personal harness, not a user
# feature). Merges staging→main, builds a SIGNED + NOTARIZED release, swaps it
# into /Applications with instant-rollback backup, and stages the notarized DMG
# artifact for the masonlee.build download. Design: Atlas self-hosted-build doc
# (§3). Core safety property: this replaces the binary ON DISK; it does NOT
# relaunch the running app. Mason's open stable AYGENT keeps running the OLD code
# until he quits + reopens — so even a bad promote can't rip the tool out
# mid-session.
#
# SIGNING (added 2026-08-10): production builds are Developer ID signed +
# Apple-notarized + stapled, so a downloaded copy opens with a clean double-click
# (no Gatekeeper wall). This is REQUIRED for anything buyers install. The script
# refuses to promote if the signing identity or notary profile is missing —
# better to stop than to ship prod unsigned.
#
# PREREQS (one-time, already done on Mason's machine):
#   - "Developer ID Application: Mason Tompkins (T36A5AA5LW)" in the login keychain
#   - notarytool keychain profile "AYGENT-NOTARY"
#       xcrun notarytool store-credentials AYGENT-NOTARY \
#         --apple-id <id> --team-id T36A5AA5LW --password <app-specific-pw>
#     (paste the app-specific password with NO quotes / straight quotes — a smart
#      quote from a rich-text field 401s. Mason 08-10.)
#
# USAGE (run from the repo root of the STAGING checkout, on the staging branch):
#   bash scripts/promote.sh
#
# Override the identity/profile if they ever change:
#   APPLE_SIGNING_IDENTITY="Developer ID Application: … (TEAMID)" \
#   AYGENT_NOTARY_PROFILE="AYGENT-NOTARY" bash scripts/promote.sh
set -euo pipefail

APP_NAME="AYGENT.app"
PROD_APP="/Applications/${APP_NAME}"
BACKUP_DIR="${HOME}/.aygent-backups"
STAMP="$(date +%Y%m%d-%H%M%S)"

# Signing config — env-overridable, sensible defaults for this machine.
SIGN_ID="${APPLE_SIGNING_IDENTITY:-Developer ID Application: Mason Tompkins (T36A5AA5LW)}"
NOTARY_PROFILE="${AYGENT_NOTARY_PROFILE:-AYGENT-NOTARY}"

# Where the site repo lives (for staging the buyer artifact). Best-effort: if it
# isn't there, we skip that step and just tell Mason how to do it.
SITE_REPO="${AYGENT_SITE_REPO:-$(dirname "$(git rev-parse --show-toplevel 2>/dev/null)")/masonleebuild}"  # sibling to AYGENT-Stage (was: ~/AYGENT/Cleo/masonleebuild — stale Cleo path, fixed 2026-08-18 for Muse)

say() { printf "\n\033[1m▶ %s\033[0m\n" "$*"; }
warn() { printf "\033[33m⚠ %s\033[0m\n" "$*"; }
die() { printf "\n\033[31m✗ %s\033[0m\n" "$*" >&2; exit 1; }

# 0. Sanity: git repo, clean tree, on staging.
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not a git repo"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" = "staging" ] || die "expected to be on 'staging', on '${BRANCH}'. Checkout staging first."
if ! git diff --quiet || ! git diff --cached --quiet; then
  die "uncommitted changes on staging. Commit them first (the agent should have)."
fi

# 0a. CAPABILITIES drift guard — version in tauri.conf.json MUST match docs/CAPABILITIES.md header.
# CAPABILITIES is the shipped user truth; if it lags, buyers see stale docs. Fail fast here
# rather than publish a DMG whose “what's new” is wrong.
say "Checking CAPABILITIES.md version sync"
TAURI_VER="$(grep -oE '"version"[[:space:]]*:[[:space:]]*"[0-9]+\.[0-9]+\.[0-9]+"' src-tauri/tauri.conf.json | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
CAP_VER="$(grep -oE '# AYGENT [^\(]*\(v[0-9]+\.[0-9]+\.[0-9]+\)' docs/CAPABILITIES.md | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
if [ -z "$TAURI_VER" ] || [ -z "$CAP_VER" ]; then
  die "could not parse versions (tauri=$TAURI_VER cap=$CAP_VER) — ensure tauri.conf.json and docs/CAPABILITIES.md headers exist"
fi
if [ "$TAURI_VER" != "$CAP_VER" ]; then
  die "version drift: tauri.conf.json is $TAURI_VER but docs/CAPABILITIES.md header is $CAP_VER — update docs/CAPABILITIES.md (header + Changelog + Last updated) to $TAURI_VER before promoting"
fi
printf "  version %s — CAPABILITIES in sync\n" "$TAURI_VER"

# 0b. Signing preflight — FAIL FAST rather than ship prod unsigned.
say "Preflight: signing identity + notary profile"
security find-identity -v -p codesigning 2>/dev/null | grep -qF "${SIGN_ID}" \
  || die "signing identity not found in keychain:
    ${SIGN_ID}
  Create it: Xcode > Settings > Accounts > Manage Certificates > + Developer ID Application"
xcrun notarytool history --keychain-profile "${NOTARY_PROFILE}" >/dev/null 2>&1 \
  || die "notary profile '${NOTARY_PROFILE}' not found / not valid.
  Create it: xcrun notarytool store-credentials ${NOTARY_PROFILE} --apple-id <id> --team-id T36A5AA5LW --password <app-specific-pw>"
printf "  identity: %s\n  notary:   %s\n" "${SIGN_ID}" "${NOTARY_PROFILE}"

# 1. Merge staging → main and push (source-of-truth promote).
say "Merging staging → main and pushing"
git fetch origin
git checkout main
git pull --ff-only origin main || die "main diverged — resolve by hand"
git merge --no-ff staging -m "promote: staging → production (${STAMP})" || die "merge conflict — resolve, then re-run"
git push origin main || die "push failed — run github_git_auth in AYGENT (or check your PAT)"

# On ANY failure after we've checked out main, get back to staging so the Dev
# agent keeps working there.
back_to_staging() { git checkout staging >/dev/null 2>&1 || true; }

# 2. Build the SIGNED release (app + dmg). Exporting APPLE_SIGNING_IDENTITY makes
#    Tauri codesign both bundles with the Developer ID cert + hardened runtime
#    (entitlements come from HardenedRuntime.plist via tauri.conf.json).
say "Building SIGNED release (cargo tauri build) — a few minutes"
pushd src-tauri >/dev/null
if ! APPLE_SIGNING_IDENTITY="${SIGN_ID}" cargo tauri build; then
  popd >/dev/null; back_to_staging; die "release build FAILED — prod untouched, back on staging"
fi
popd >/dev/null

# 3. Locate the produced .app + .dmg.
BUILT_APP="$(find src-tauri/target/release/bundle/macos -maxdepth 1 -name "${APP_NAME}" -type d | head -n1)"
[ -n "${BUILT_APP}" ] || { back_to_staging; die "built .app not found — check the build output"; }
BUILT_DMG="$(find src-tauri/target/release/bundle/dmg -maxdepth 1 -name "*.dmg" | head -n1)"
[ -n "${BUILT_DMG}" ] || { back_to_staging; die "built .dmg not found — is the 'dmg' target in tauri.conf.json?"; }

# 4. NOTARIZE + STAPLE. We notarize the DMG (covers the app inside it) via the
#    keychain profile, then staple the ticket onto BOTH the dmg and the .app.
#    We do this manually (notarytool + stapler) rather than Tauri's build-time
#    notarize because Tauri wants the APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID (or
#    API-key) env trio, not a keychain profile — and the profile is what Mason
#    set up. A notarization failure is FATAL (prod stays untouched).
say "Notarizing (uploading to Apple, waiting for the verdict) — a few minutes"
if ! xcrun notarytool submit "${BUILT_DMG}" --keychain-profile "${NOTARY_PROFILE}" --wait; then
  back_to_staging; die "notarization FAILED — prod untouched.
  Inspect: xcrun notarytool log <submission-id> --keychain-profile ${NOTARY_PROFILE}"
fi
say "Stapling the notarization ticket"
xcrun stapler staple "${BUILT_DMG}" || { back_to_staging; die "staple (dmg) failed"; }
xcrun stapler staple "${BUILT_APP}" || { back_to_staging; die "staple (app) failed"; }

# 4b. Verify Gatekeeper will accept it (the real proof a buyer's copy opens
#     clean). Must say 'source=Notarized Developer ID'. FATAL if not.
say "Verifying notarization (spctl)"
SPCTL_OUT="$(spctl -a -vvv -t install "${BUILT_APP}" 2>&1 || true)"
printf "%s\n" "${SPCTL_OUT}"
echo "${SPCTL_OUT}" | grep -q "source=Notarized Developer ID" \
  || { back_to_staging; die "spctl did NOT report a notarized Developer ID — refusing to ship. prod untouched."; }

# 5. Swap the notarized .app into /Applications with rollback backup. NEVER
#    delete prod until the new copy is safely staged.
say "Swapping ${PROD_APP} (with rollback backup)"
mkdir -p "${BACKUP_DIR}"
if [ -d "${PROD_APP}" ]; then
  cp -R "${PROD_APP}" "${BACKUP_DIR}/${APP_NAME}.${STAMP}" || { back_to_staging; die "backup failed — prod untouched"; }
  rm -rf "${PROD_APP}.old"
  mv "${PROD_APP}" "${PROD_APP}.old"
fi
if ! cp -R "${BUILT_APP}" "${PROD_APP}"; then
  [ -d "${PROD_APP}.old" ] && mv "${PROD_APP}.old" "${PROD_APP}"
  back_to_staging; die "install failed — rolled back to previous prod"
fi
# Now that prod is the notarized build, spctl on it should also pass.
spctl -a -t install "${PROD_APP}" >/dev/null 2>&1 || warn "spctl on installed prod reported issues — check manually"

# 6. Stage the BUYER ARTIFACT for the site: the NOTARIZED + STAPLED .dmg, named
#    the way scripts/publish-release.js expects (AYGENT-<version>-macOS.dmg). A
#    DMG is what a Mac buyer expects (drag-to-Applications), and the ticket is
#    stapled so the downloaded copy verifies offline. We DON'T publish here (that
#    uploads to Supabase + emails owners) — we just place the exact file so the
#    publish step is a single clean command. Version comes from the config.
VERSION="$(grep -oE '"version"[[:space:]]*:[[:space:]]*"[0-9]+\.[0-9]+\.[0-9]+"' src-tauri/tauri.conf.json | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
if [ -n "${VERSION}" ] && [ -d "${SITE_REPO}/product-files" ]; then
  DEST_DMG="${SITE_REPO}/product-files/AYGENT-${VERSION}-macOS.dmg"
  say "Staging buyer artifact: ${DEST_DMG}"
  # Copy the STAPLED dmg under the versioned name the publish script + account-
  # page download expect. (Remove any stale zip from the pre-signing era.)
  rm -f "${SITE_REPO}/product-files/AYGENT-${VERSION}-macOS.zip"
  rm -f "${DEST_DMG}"
  cp "${BUILT_DMG}" "${DEST_DMG}" \
    && printf "  wrote %s (%s MB)\n" "${DEST_DMG}" "$(du -m "${DEST_DMG}" | cut -f1)" \
    || warn "could not stage the buyer dmg — copy ${BUILT_DMG} manually (see publish step below)"
else
  warn "site repo product-files not found at ${SITE_REPO}/product-files (or no version parsed) — skipping buyer-dmg staging"
fi

# 6b. AUTO-PUBLISH (default when the agent drives a promote). If
# AYGENT_PUBLISH_NOTES is set, run publish-release.js right now with those
# notes: upload (upsert) + release row + owner emails. This is what makes "push
# to main" mean "buyers actually get it" — without it the notarized DMG sits in
# product-files/ and nobody is told. Manual runs WITHOUT the env var keep the
# old behavior (print the command, do nothing), so a human promote never emails
# owners by surprise. AYGENT_PUBLISH_NO_EMAIL=1 adds --no-email (silent publish:
# upload + release row, no emails).
if [ -n "${AYGENT_PUBLISH_NOTES:-}" ]; then
  if [ -d "${SITE_REPO}" ] && [ -f "${SITE_REPO}/scripts/publish-release.js" ]; then
    say "Publishing release ${VERSION} (upload + release row + owner emails)"
    PUBLISH_ARGS=("${VERSION}" "${AYGENT_PUBLISH_NOTES}")
    [ -n "${AYGENT_PUBLISH_NO_EMAIL:-}" ] && PUBLISH_ARGS+=("--no-email")
    if ! (cd "${SITE_REPO}" && node scripts/publish-release.js "${PUBLISH_ARGS[@]}"); then
      back_to_staging; die "publish-release FAILED — prod IS live in /Applications, but buyers were not notified. Re-run by hand: cd ${SITE_REPO} && node scripts/publish-release.js ${VERSION} \"notes\""
    fi
  else
    warn "AYGENT_PUBLISH_NOTES was set but site repo not found at ${SITE_REPO} — skipping auto-publish (run it by hand)"
  fi
fi

# 7. Back to staging.
git checkout staging

say "DONE. Production is now the SIGNED + NOTARIZED promoted build."
cat <<EOF

  ── YOU (Mason) ──────────────────────────────────────────────────────────────
  1. QUIT and REOPEN /Applications/AYGENT.app to run the new production build.
     (Your currently-open app is still the old build until you do — intended.)

  2. PUBLISH to buyers (uploads the zip to Supabase + records the release +
     emails owners on older versions):
       cd "${SITE_REPO}"
       node scripts/publish-release.js ${VERSION:-<x.y.z>} "What changed."
     Add --no-email to skip the owner emails.

  Rollback (instant) if the new prod misbehaves:
    rm -rf "${PROD_APP}" && mv "${PROD_APP}.old" "${PROD_APP}"
  Or restore a timestamped backup from: ${BACKUP_DIR}/

EOF
