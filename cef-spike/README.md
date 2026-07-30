# cef-spike — PHASE 0: CEF-rs toolchain de-risk (macOS / Apple Silicon)

**Status:** throwaway spike. Goal: prove `cef-rs` builds and renders one real
Chromium window on Mason's Apple-Silicon Mac, in a crate **fully separate** from
`aygent` so a broken CEF build can never brick the working app.

This lives inside the aygent **git repo** for one reason only — so it can be
committed/pushed to `origin/main` — but it sits at the repo top level
(`aygent/cef-spike/`), **outside `src-tauri/`**. aygent's cargo build only ever
looks at `src-tauri/`, and there is no root/workspace `Cargo.toml` globbing
sibling folders, so this spike is 100% inert to aygent's build and can never
affect aygent's `Cargo.lock` or brick the app. Its buildable crate is the
vendored upstream `cef-rs` checkout, which is git-ignored.

---

## What this spike is (and the engineering decision behind it)

The official `cef-rs` `cefsimple` example is genuinely multi-file interlocking
Rust (`main.rs`, `lib.rs`, `shared/mod.rs`, `shared/simple_app.rs`,
`shared/simple_handler/*`, `mac/mod.rs` ~10KB, `bin/cefsimple_helper.rs`) plus a
workspace with the `export-cef-dir` and `bundle-cef-app` tooling crates. Hand-
retyping that on Windows (where I cannot compile to verify) is the single biggest
risk to a Phase-0 spike: one wrong line and the spike "fails" for a reason that
has nothing to do with the actual toolchain.

**So this spike vendors the upstream example verbatim** by cloning `cef-rs` at an
**exact pinned tag** and running its own `cefsimple` example unmodified. That is
the correct move for a de-risk spike: we are testing whether *the real cef-rs
toolchain* (framework download, build, macOS bundle, helper subprocess, DYLD)
works on Mason's machine — not whether I can retype 20KB of Rust correctly.

The `bootstrap.sh` script in this folder does the clone-at-pinned-tag + runs the
exact README commands. `run.sh` is the copy-paste one-shot.

**Pinned version:**
- cef-rs tag: **`export-cef-dir-v151.0.0+151.3.11`** (commit `bab38d9`)
- `cef` crate version: **`151.0.0+151.3.11`**
- CEF build: **151.3.11** → Chromium **151.x**
- (These are the crate/workspace `version` in cef-rs `Cargo.toml` at that tag.)

**Great bonus discovered from the source:** the upstream `cefsimple` already
defaults its start URL to `https://www.google.com/`, and it accepts a
`--url=<URL>` command-line switch. So the mission requirement ("load
google.com") is satisfied by the stock example with zero source edits.

---

## Files in this folder

```
cef-spike/
  README.md        <- this file (the full runbook + risks + Phase 1 roadmap)
  bootstrap.sh     <- clones cef-rs @ pinned tag into ./vendor/cef-rs (idempotent)
  run.sh           <- one-shot: bootstrap -> export framework -> env -> bundle -> open
  .gitignore       <- keeps the ~2GB build + ~170MB framework OUT of git
```

`vendor/cef-rs/` is created by `bootstrap.sh` on the Mac and is git-ignored (it's
a full upstream checkout; we don't commit it). If you'd rather commit a frozen
copy, see "Optional: freeze the vendor" at the bottom.

---

## PREREQUISITES (macOS)

- Apple Silicon Mac (arm64). cef-rs supports macOS ARM64. ✅
- **Xcode Command Line Tools**: `xcode-select --install` (needs `clang`,
  `libclang` for bindgen, and the macOS SDK). A full Xcode is safest.
- **Rust** (stable, recent — the example uses `edition = "2024"`, so you need a
  Rust toolchain new enough for edition 2024: **Rust 1.85+**). Check:
  `rustc --version`. If old: `rustup update stable`.
- **cmake** (the `cef` build-util / dll-sys build uses it): `brew install cmake`.
- **git**, **curl** (preinstalled).
- ~**3–4 GB free disk**: the CEF framework is ~170 MB compressed but the extracted
  framework + the debug build artifacts are multiple GB.
- Network: the framework download is ~hundreds of MB; on a slow link the first
  `export-cef-dir` run can take several minutes. This is expected.

---

## THE EXACT MAC COMMANDS (copy-paste ready)

Everything runs from this spike folder on the Mac. Mason's Mac path to the repo
is `~/Documents/aygent`, so this spike is at
`~/Documents/aygent/cef-spike` (the aygent repo root on the Mac is
`~/Documents/aygent`).

```sh
# 0. Go to the spike folder
cd ~/Documents/aygent/cef-spike

# 1. Clone cef-rs at the exact pinned tag into ./vendor/cef-rs (idempotent)
bash bootstrap.sh

# 2. Fetch/extract the shared CEF framework (~hundreds of MB; minutes on first run)
cd vendor/cef-rs
cargo run -p export-cef-dir -- --force "$HOME/.local/share/cef"

# 3. Set the macOS env vars (EXACTLY as the cef-rs README specifies for macOS)
export CEF_PATH="$HOME/.local/share/cef"
export DYLD_FALLBACK_LIBRARY_PATH="$DYLD_FALLBACK_LIBRARY_PATH:$CEF_PATH:$CEF_PATH/Chromium Embedded Framework.framework/Libraries"

# 4. Build + bundle the cefsimple example into a proper macOS .app
#    (bundle-cef-app builds the app, the helper subprocess, and lays out the
#     .app so the Chromium Embedded Framework + helpers are in the right place)
cargo run --bin bundle-cef-app -- cefsimple -o target/bundle

# 5. Run it — a real Chromium window should open on https://www.google.com/
open target/bundle/cefsimple.app
```

### One-shot equivalent
From the `cef-spike` folder: `bash run.sh`
(It does steps 1–5 above, in order, with the env exported inline.)

### Load a different URL (optional, proves the CDP-style arg plumbing)
The stock example reads a `--url=` switch. To pass args you run the binary inside
the bundle directly (so `open` arg-passing quirks don't bite):
```sh
"target/bundle/cefsimple.app/Contents/MacOS/cefsimple" --url=https://example.com
```
Default (no arg) = google.com, which is what Phase 0 wants.

---

## WHAT MASON SHOULD SEE (and how we know it's REAL Chromium, not WebKit)

- A native window titled/showing **google.com**, rendered by Chromium's Views/
  native path (the example uses CEF Views by default).
- **Proof it's real Chromium, not WKWebView/WebKit:**
  1. Right-click the page → the context menu is **Chromium's** ("Back", "Reload",
     "View page source", "Inspect") — WebKit's menu looks different and has no
     "Inspect" wired like this.
  2. Navigate to `chrome://version` (via `--url=chrome://version`): it shows a
     **Chromium 151.x** version string and the CEF version. WebKit has no
     `chrome://` scheme at all — this page literally cannot render under WKWebView.
  3. `chrome://gpu` renders (Chromium-only internal page).
  4. In Activity Monitor you'll see the **helper subprocess(es)**
     (`cefsimple_helper`) — Chromium's multi-process model. WKWebView shows
     `com.apple.WebKit.*` helpers instead.
  5. User-Agent at `https://www.whatismybrowser.com/` (or JS `navigator.userAgent`)
     reports **Chrome/151**, not Safari/WebKit.

Any ONE of #2/#3 is conclusive: `chrome://` pages only exist in Chromium.

---

## KNOWN RISKS / LIKELY FAILURE POINTS (so a failure is diagnosable)

Paste the exact error if any of these hit — each maps to a known cause:

1. **Framework download fails / is slow (step 2).**
   - Symptom: `export-cef-dir` hangs or errors mid-download, or
     `cef-dll-sys build.rs` re-downloads during step 4.
   - Cause: network / CDN. The download is large. Re-run step 2; it's idempotent
     with `--force`. If step 2 succeeded, step 4 should NOT re-download (that's
     the whole point of the shared install + `CEF_PATH`).

2. **`DYLD_FALLBACK_LIBRARY_PATH` wrong → dylib not found at runtime (step 5).**
   - Symptom: app bounces / crashes instantly; Console.app shows
     `Library not loaded: @rpath/Chromium Embedded Framework` or
     `dyld: ... Chromium Embedded Framework.framework/...`.
   - Cause: the env var must include the space-containing path
     `.../Chromium Embedded Framework.framework/Libraries`. It's quoted in step 3
     for exactly this reason. NOTE: `DYLD_*` vars are **stripped by macOS SIP**
     for system binaries, but they DO apply to our own bundled binary launched
     via `open`/direct exec, which is why the bundle also embeds the framework.
   - If it only fails under `open` but works via direct
     `Contents/MacOS/cefsimple`, that confirms a DYLD-env propagation issue and
     the bundle layout (not env) is the real fix — which `bundle-cef-app` handles.

3. **Helper subprocess signing / "app is damaged" / Gatekeeper (step 5).**
   - Symptom: macOS refuses to launch, "cannot be opened because the developer
     cannot be verified", or the helper subprocess is killed → white/blank window
     then crash.
   - Cause: the `.app` and its **helper** are ad-hoc/unsigned. For a LOCAL spike:
     `xattr -dr com.apple.quarantine target/bundle/cefsimple.app` clears the
     quarantine bit. If the helper still won't launch, ad-hoc sign locally:
     `codesign --force --deep -s - target/bundle/cefsimple.app`.
   - This is the #1 thing that becomes non-trivial for Phase 1 (a shipped app
     needs a real Developer ID + notarization for the app AND every helper). For
     the spike, ad-hoc/local is fine — but we WANT to see this here so we know
     it's coming.

4. **`libclang` / bindgen not found (step 2 or 4 build).**
   - Symptom: build error mentioning `libclang`, `clang-sys`, or bindgen.
   - Cause: Xcode CLT missing. Fix: `xcode-select --install`. If installed but
     still failing: `export LIBCLANG_PATH="$(xcode-select -p)/usr/lib"` (or the
     Homebrew LLVM path).

5. **`cmake` not found (build).**
   - Symptom: build error from the `cmake` crate / `sandbox` feature build.
   - Fix: `brew install cmake`.

6. **Edition 2024 / toolchain too old.**
   - Symptom: `error: edition 2024 is unstable` or `feature edition2024 required`.
   - Fix: `rustup update stable` (need Rust ≥ 1.85).

7. **Sandbox feature build issues on macOS.**
   - The example enables the `sandbox` feature by default, which pulls the CEF
     sandbox lib + requires the helper. If the sandbox build is the blocker,
     a fast diagnostic is to build the example with `--no-default-features`
     to isolate whether the failure is CEF-core vs. CEF-sandbox. (Keep sandbox
     ON for the real thing; this is only a triage lever.)

8. **Min macOS version.** CEF 151 targets a recent Chromium; it expects a modern
   macOS. If Mason is on an old macOS the framework may refuse to load — Console
   will say so. Unlikely on a current Apple-Silicon Mac.

**What "spike SUCCEEDS" looks like:** step 5 opens a window, `chrome://version`
shows Chromium 151.x, a `cefsimple_helper` process is alive in Activity Monitor.
**What "spike FAILS usefully" looks like:** any of the above with a concrete error
Mason pastes back — every one maps to a known fix above, which is the entire
point of doing this cheaply now.

---

## PHASE 1 ROADMAP (the punch-out embed into aygent — NOT this spike)

Once Phase 0 proves the toolchain, Phase 1 integrates CEF as aygent's **visible
browser surface**, replacing the wry/WKWebView child webview:

1. **Add `cef` to `aygent/src-tauri` as an internal engine crate** (or a sibling
   crate aygent depends on), gated behind a Cargo feature (`engine-cef`) so the
   WKWebView path stays intact and shippable during migration.
2. **Adopt the macOS multi-process app layout in aygent's Tauri bundle**: aygent's
   `.app` must ship the `Chromium Embedded Framework.framework` + a
   `<app> Helper.app` (and the GPU/Renderer/Plugin helper variants) with correct
   `Info.plist`s and entitlements. Port `bundle-cef-app`'s layout logic into
   aygent's `tauri.conf.json` bundling / a build step.
3. **Windowing: embed CEF into a Tauri window region** instead of a top-level CEF
   window. On macOS this means giving CEF a parent `NSView` (the region where the
   WKWebView child currently lives) — CEF supports windowed rendering into a
   parent view; we drive it from the same layout code that positions the current
   child webview.
4. **Wire CDP** to the agent-automation layer: launch CEF with
   `--remote-debugging-port` (or the CEF remote-debugging setting) so our existing
   CDP client attaches — this is WHY CEF was chosen (WKWebView can't). Verify our
   automation commands work against Chromium's CDP.
5. **Downloads:** implement CEF's `DownloadHandler` (`OnBeforeDownload` /
   `OnDownloadUpdated`) — this is the concrete fix for the wry `on_download`
   right-click-Save-Image bug. Prove right-click → Save Image writes a file.
6. **OAuth / embedded-detection:** confirm Google OAuth no longer trips
   embedded-browser detection (real Chrome UA + real Chromium).
7. **Signing/notarization:** Developer ID sign + notarize the app AND every
   helper (risk #3 above, productionized). This is a hard requirement before ship.
8. **Kill switch:** keep `engine-wkwebview` as a fallback feature until CEF is
   proven across the target sites, so we can revert instantly.

---

## Optional: freeze the vendor (commit a pinned copy instead of cloning)

If you want the exact upstream source committed into aygent's repo for
reproducibility (no dependency on GitHub being up), after `bootstrap.sh`:
```sh
rm -rf vendor/cef-rs/.git
# then remove the vendor/ ignore line from .gitignore and commit vendor/cef-rs
```
For Phase 0 we DON'T do this (keeps the commit small); the pinned tag in
`bootstrap.sh` is the reproducibility guarantee.
