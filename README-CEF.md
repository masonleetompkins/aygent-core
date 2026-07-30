# AYGENT — CEF engine (Phase 1) build + run runbook (macOS / Apple Silicon)

Phase 1 replaces the in-app browser's **visible surface** with native Chromium
(CEF 151.3.11 / Chromium 151.x) via `tauri-apps/cef-rs`, embedded into the Tauri
main window with the **Atrium punchout** technique. WKWebView stays compiled as a
fallback behind the `engine-cef` Cargo feature (the kill-switch).

Phase 0 (`cef-spike/`) already proved the toolchain on your Mac. Phase 1 reuses
that EXACT framework-export + bundle recipe.

---

## What got built (architecture)

- **`engine-cef` Cargo feature** (default OFF). OFF ⇒ a plain build compiles the
  working WKWebView browser and never pulls CEF. ON ⇒ the browser screen uses
  native Chromium. Instant revert: drop the flag.
- **`src-tauri/src/cef_engine.rs`** — CEF init (framework load, `execute_process`,
  `initialize` with `multi_threaded_message_loop`), a shared `AygentClient` with
  life-span / display / **download** handlers built via cef-rs's `wrap_*!` macros,
  a per-tab `Browser` registry, and CDP remote-debugging on a free port.
- **`src-tauri/src/cef_geometry.rs`** — the punchout: `setDrawsBackground:NO` on
  the Wry webview, one AYGENT-owned wrapper `NSView` per tab (behind the main
  webview), frame-syncing to the React pane rect, and the **`hitTest:` override**
  that returns `nil` when a React overlay is open so clicks fall through.
- **`src-tauri/src/bin/aygent_helper.rs`** — the CEF helper subprocess binary
  (GPU/Renderer/Plugin/Alerts re-exec this).
- **`browser.rs`** — every visible-surface command (`webview_open`,
  `webview_set_bounds`, `webview_hide`, `webview_hide_others`, `webview_navigate`,
  `webview_history`, `webview_close`) branches to CEF when the engine is live;
  otherwise runs the unchanged WKWebView path. The agent CDP client
  (`ensure_running`) attaches to CEF's remote-debugging port ⇒ **human + agent
  share ONE Chromium** (kills the two-browser desync).
- **New commands:** `set_browser_hittest(enabled)` (overlay hit-test toggle) and
  `browser_engine_info()` (reports `cef` vs `wkwebview`).
- **`Browser.tsx`** — derived overlay-open predicate → `set_browser_hittest`
  (a boolean derived from React state, NOT a drift-prone counter).
- **Native downloads** — CEF's `DownloadHandler` writes straight to
  `<agentFolder>/downloads/` and emits `browser:download`. This is the real fix
  for the whole WKWebView right-click-Save-Image saga (CEF is not sandboxed like
  WebKit's networking XPC process).

---

## Prerequisites (same as the Phase-0 spike)

- Apple Silicon Mac, Xcode CLT (`xcode-select --install`), Rust ≥ 1.85,
  **`brew install cmake ninja`** (BOTH — cef-dll-sys builds CEF's C wrapper via
  CMake + the Ninja generator; missing Ninja = `CMake was unable to find a build
  program corresponding to "Ninja"`), ~3–4 GB free disk.

---

## Build + run (EXACT commands)

```sh
cd ~/Documents/aygent

# 1. Export the CEF framework ONCE (identical to Phase-0 spike step 2). This is
#    the ~170MB shared framework the build + bundle both consume via CEF_PATH.
#    (If you already ran the spike, it's cached at ~/.local/share/cef.)
cd cef-spike && bash bootstrap.sh && cd vendor/cef-rs
cargo run -p export-cef-dir -- --force "$HOME/.local/share/cef"
cd ~/Documents/aygent

export CEF_PATH="$HOME/.local/share/cef"
export DYLD_FALLBACK_LIBRARY_PATH="${DYLD_FALLBACK_LIBRARY_PATH:-}:$CEF_PATH:$CEF_PATH/Chromium Embedded Framework.framework/Libraries"

# 2. Discard any stale Cargo.lock churn (the cef dep + its tree get added the
#    FIRST time you build WITH the feature; that's expected). If the lock fights
#    you, delete it and let cargo resolve:
#      git checkout -- src-tauri/Cargo.lock   # or:  rm src-tauri/Cargo.lock

# 3a. DEV build WITH the feature (fast iterate). Tauri builds AYGENT.app under
#     src-tauri/target/debug/bundle/macos/ — but dev normally runs the bare
#     binary, which will NOT have the CEF framework injected. For CEF you must
#     run the BUNDLED .app (helpers + framework), so use the release-ish bundle
#     path below, OR inject into the dev .app. Recommended: build the bundle.
#
#     ⚠️ CRITICAL: `cargo tauri` MUST be run from the src-tauri/ dir (where
#     Cargo.toml + tauri.conf.json live). Running it from the repo root gives
#     `could not find Cargo.toml` AND mangles beforeBuildCommand's `--prefix ui`
#     into `ui/ui/package.json`. So cd into src-tauri FIRST. All target/ +
#     scripts/ paths below are written RELATIVE TO src-tauri/ (../scripts, etc).
cd ~/Documents/aygent/src-tauri
cargo tauri build --features engine-cef --debug

#    The bundled app lands at (debug, relative to src-tauri/):
#      target/debug/bundle/macos/AYGENT.app
#    Release: drop --debug -> target/release/bundle/macos/AYGENT.app

# 3b. Build the helper bin explicitly (tauri build may not build extra [[bin]]s
#     with required-features unless asked). Still in src-tauri/:
cargo build --features engine-cef --bin aygent_helper           # debug
#   (release:  cargo build --release --features engine-cef --bin aygent_helper)

# 4. INJECT the CEF framework + 5 helper .apps into the bundle + ad-hoc sign.
#    Run from src-tauri/; the script + paths are relative to it.
bash ../scripts/cef-bundle.sh \
  "target/debug/bundle/macos/AYGENT.app" \
  "target/debug"

# 5. RUN. Launch the binary DIRECTLY (not `open`) so you see the [aygent][cef]
#    stderr logs — paste those back if anything fails.
"target/debug/bundle/macos/AYGENT.app/Contents/MacOS/AYGENT"
```

**Kill-switch (back to WKWebView instantly):** build with NO feature —
`cargo tauri build` (or `cargo tauri dev`). No CEF, no bundle step, old browser.

---

## What you should see

- The Browser screen renders a **native Chromium** tab (not WKWebView). Confirm:
  navigate a tab to `chrome://version` → shows **Chromium 151.x** (WKWebView
  cannot render `chrome://` at all — conclusive).
- In Activity Monitor: `AYGENT Helper (GPU)` / `(Renderer)` processes alive
  (Chromium's multi-process model).
- Multi-tab, address bar, Back/Forward/Reload, tab labels updating on nav, and
  persistent History all work as before — now backed by CEF.
- **Right-click an image → Save Image** (or any download) lands in
  `<agentFolder>/downloads/` and shows in the Downloads panel. No sandbox errors.
- The **agent** drives the **same tab you're watching** (unified CDP): hand off,
  give it a task, watch it act in your visible Chromium tab.
- **Overlays are clickable**: open History/Downloads, the permission card, or the
  agent pane — clicks on them land (the `[aygent][cef] hittest DISABLED` log
  fires when an overlay opens, `ENABLED` when it closes).
- Log tags to watch: `[aygent][cef] init … engine ready`, `browser created`,
  `url loaded`, `download begin/complete`, `hittest …`, `place wrapper …`.

---

## KNOWN RISK SEAMS (what you'll likely hit first + what it looks like)

1. **NSApplication / CefAppProtocol seam (THE big one).** CEF's macOS docs want
   `NSApp` to be a `CefAppProtocol` subclass (cefsimple's `SimpleApplication`).
   Tauri/Wry ALREADY own `NSApp` before our setup runs, so we can't swap it. We
   run CEF with `multi_threaded_message_loop = true` so CEF spins its OWN UI
   thread and does NOT need to pump events through Wry's NSApplication.
   - **If it crashes at init** with something about `sendEvent` /
     `isHandlingSendEvent` / a CefAppProtocol assertion, this is the seam. The
     fallback fix is to run CEF with `external_message_pump = true` and pump
     `do_message_loop_work()` from a Tauri timer, OR to subclass NSApp before
     Tauri boots. Paste the exact crash line.

2. **`dyld: Library not loaded: @rpath/Chromium Embedded Framework`** on launch.
   - The framework isn't in `Contents/Frameworks/` or the helper's resolver
     (`../../..`) can't find it. Re-run `scripts/cef-bundle.sh`; confirm
     `Contents/Frameworks/Chromium Embedded Framework.framework` exists and the
     5 `AYGENT Helper*.app` are present. Ensure the `DYLD_FALLBACK_LIBRARY_PATH`
     export (step 1) is set if launching the bare binary.

3. **Helper won't launch / white-then-crash / "app is damaged".** Ad-hoc signing
   or quarantine. `scripts/cef-bundle.sh` already `codesign --force --sign -` +
   `xattr -dr com.apple.quarantine`. If it still fails, sign the framework FIRST,
   then each helper, then the outer app (innermost-first), which the script does.

4. **Cargo.lock churn / edition mismatch.** The `cef` crate pulls a large tree on
   first feature build. If cargo complains about `edition2024` from a transitive
   dep, `rustup update stable` (need ≥ 1.85). Discard the lock and re-resolve if
   it fights (step 2).

5. **`libclang` / bindgen not found** during the cef build → `xcode-select
   --install` (same as spike risk #4). **`cmake` not found** → `brew install cmake`.

6. **Blank browser pane but app runs.** The punchout wrapper is behind the Wry
   webview but the webview isn't transparent, OR the wrapper frame is wrong.
   Check `[aygent][cef] main webview setDrawsBackground:NO` and `place wrapper`
   logs; confirm the React pane div is transparent at the tile coords.

7. **Clicks go to the browser through an overlay** (or overlays eat all clicks).
   The `set_browser_hittest` toggle. Watch the `hittest ENABLED/DISABLED` logs vs
   what overlay is open. The signal is a derived boolean in `Browser.tsx`
   (`overlayOpen`), so if it drifts it's a state bug there, not a counter leak.

---

## Deferred to Phase 2

- **Developer-ID signing + notarization** of the app AND every helper (Phase 1 is
  ad-hoc/local only — fine for your machine, required before distribution).
- Hardened-runtime notarization staple; DMG signing.
- OAuth/passkey/FedCM validation across target sites (CEF supports them; verify).
- Per-pixel partial-overlap hit-testing (Atrium's NSEvent mouse-monitor for
  toasts that cover only PART of the browser). Phase 1 uses whole-surface
  suspend, which is correct for our full-width overlays.
- Retiring the reqwest page-bridge download workaround once native CEF downloads
  are proven (kept as belt-and-suspenders for now; harmless under CEF).
```
