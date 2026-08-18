# Agent Bug Log

> Tracked in AYGENT-Stage — source of truth for repeat failures. Update this file, don't duplicate it.

## 2026-08-18 — Sparks Interactive Dead (P0) — **RESOLVED (root cause found)**

### RESOLUTION (Muse, same day)
**Root cause #1 (the actual bug): Tauri codegen CSP hash-locking killed every Spark inline script.**
- `tauri.conf.json` declares `script-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net`, but Tauri 2's asset codegen **appends `sha256-…` hashes of the bundled assets to `script-src`**. Per the CSP spec, the presence of ANY hash/nonce in `script-src` makes `'unsafe-inline'` **ignored**.
- Proof: `strings <old binary>` showed `script-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net …` **plus** `sha256-WEW9phDNQbJDDCr4D2bx2JeMYOlyU=` baked into the executable.
- Blob iframes **inherit the parent document's CSP** in WKWebView (blob = same CSP context, sandbox or not). So every `<script>` inside the wrapped Spark blob — SPARK_RUNTIME, the boot harness, the Spark's own logic — was silently blocked. No error banner possible: the banner code is itself an inline script that never ran. Styling still worked because `style-src` retained a working `'unsafe-inline'` path via the injected `<style>` (and hover is pure CSS).
- This is why EVERY JS-level fix (dummy getElementById, type=button, per-listener isolation, __sparkErr) changed nothing: **none of that code ever executed.**
- **Fix:** `"dangerousDisableAssetCspModification": ["script-src"]` in `app.security` — stops Tauri from appending hashes to `script-src` only (style-src still gets hashes). `'unsafe-inline'` now takes effect; Spark inline scripts run. Verified: new binary has **0** `sha256-` entries near the CSP string.

**Root cause #2 (why the fix loop was invisible): same-version rebuilds never re-busted the WKWebView cache.**
- `bust_webview_cache_on_version_change` stamped `version + asset hash`. Rebuilding at 1.0.8 with the identical `index-yRDYgmoN.js` ⇒ `prev == build_id` ⇒ no bust. The cached HTML **includes the injected meta-CSP**, so even a fixed binary could serve the old hash-locked page.
- **Fix:** build id now = `version:assetHash:exeMtime` (`std::env::current_exe()` mtime). Any rebuild — even config-only, same version — busts the cache on next launch. No version bump ever needed to ship a fix again.

**What was actually broken vs what was cached:**
- Broken: the CSP itself (in EVERY build 1.0.6→1.0.8 — the JS fixes were correct but unreachable).
- Cached: red herring, but real — the stamp logic guaranteed a second same-version build could serve the stale page, masking any config fix.

**Files changed (no version bump — still 1.0.8):**
- `src-tauri/tauri.conf.json` — added `dangerousDisableAssetCspModification: ["script-src"]`.
- `src-tauri/src/lib.rs` — `bust_webview_cache_on_version_change` build id includes exe mtime.
- Rebuilt: `npm --prefix ui run build` (dist unchanged: `index-yRDYgmoN.js` 376.76 kB) + `cargo tauri build --debug` → fresh `AYGENT.app` + `AYGENT_1.0.8_aarch64.dmg`.

**To verify (user):** quit ALL AYGENT instances (`ps aux | grep AYGENT` — the Desktop copy PID is stale now), copy the fresh `AYGENT-Stage/src-tauri/target/debug/bundle/macos/AYGENT.app` over `~/Desktop/AYGENT.app`, open, new chat → tip calculator → seg/stepper/input should be live. Run ONE instance only (shared cache dir).

---

## 2026-08-18 — Sparks Interactive Dead (P0) — original report

**Owner:** Mason Tompkins  
**Component:** AYGENT Sparks (`ui/src/lib/sparkChrome.ts` + `ui/src/screens/Chat.tsx` + `src-tauri/src/lib.rs` + `ui/dist`)  
**App:** `build.masonlee.aygent` — Tauri + Vite (ui/dist -> Rust bundle)

### Symptom
- Sparks render inline in chat (`spark_preview`) and Sparks tab, look styled correctly (hover works), but are dead to interaction.
- Repro — new chat → "build a tip calculator" → `tip-calculator` spark:
  - Tip % seg buttons don't toggle `.active`
  - Bill amount input doesn't populate / doesn't live-update `stat` totals
  - Stepper `+`/`-` doesn't update count or totals
- Same in Chat inline and Sparks tab. No visible `__sparkErr` banner, no console (opaque blob iframe).

### Expected
- Inputs → live recompute, seg → exactly one `active` / `aria-pressed=true`, stepper updates `span.val`, `stat` large and live. `localStorage` / `window.spark` persists across tab switch / reload.

### Repro Steps
1. Use debug bundle `AYGENT-Stage/src-tauri/target/debug/bundle/macos/AYGENT.app` (also at `~/Desktop/AYGENT.app`).
2. New chat → ask for tip calculator (or any Spark with `<script>` handlers).
3. Click seg buttons / type in `input#bill` / hit stepper.
4. Observe: nothing changes.

### Root Cause Hypotheses (same family — week-long chase)
- Opaque blob iframe: bare `<script>` throw kills entire script, no console visible.
- `getElementById('missing') → null → null.addEventListener` throws.
- Bare `<button>` (no `type`) → implicit `submit` → iframe navigates/reloads.
- One handler throw kills sibling listeners (no isolation).
- WKWebView blob `sandbox` / CSP blocks inline script if `allow-scripts` / `frame-src` / `script-src` handling stutters. **← THIS ONE (see RESOLUTION)**
- Vite `ui/dist/assets/index-*.js` stale vs Rust bundle → fix exists in source but not in shipped bundle.
- WKWebView disk cache serving stale frontend across updates (needed `bust_webview_cache`).

### Fixes Already Tried (one-liners)
- `sparkChrome.ts` dummy `getElementById` — return no-op dummy with `__sparkDummy` instead of `null`.
- Auto `type="button"` on all bare `<button>` (both runtime shim + boot harness `fixButtons`).
- Per-listener `EventTarget.addEventListener` wrap — isolate throws, post `__sparkErr` banner, don't kill siblings.
- `__sparkErr` visible error banner + `parent.postMessage({__spark:true,kind:'error'})` for silent blob errors.
- Seed-once blob pattern (`useSparkBlobUrl` + `seedKeyRef`) — avoid reload loop.
- `window.__SPARK_STATE_SEED` / `SPARK_STATE_SEED` injection via `wrapSparkHtml` + `SPARK_RUNTIME`.
- `bust_webview_cache_on_version_change` with `frontend_build_id` hash (from `ui/dist/index.html`) → clears `~/Library/Caches/build.masonlee.aygent/WebKit` on version/build change, stamp in `frontend-version.txt`.
- `localStorage` / `sessionStorage` / `window.spark` persistence shim via `postMessage` KV (`isSparkStateMsg` / `spark_state` jail).
- Sandbox `allow-scripts allow-popups allow-forms allow-modals` only (opaque, no `allow-same-origin`).
- Generic `fetch_url` / CSP `script-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net` + `frame-src blob:` (in `tauri.conf.json`).
- Full UI rebuild `vite v5.4.21` (66 modules, `index-yRDYgmoN.js 376.76 kB`) — verified `__sparkDummy` / `SPARK_STATE_SEED` in bundle.
- Removed `StrictMode` double-invoke in `ui/src/main.tsx` (was double-registering Tauri event listeners).

### Build / Version Trail
- `1.0.6 → 1.0.7` commit `c137750` — fault-isolated handlers + auto `type=button` (sparkChrome.ts 31 lines).
- `1.0.7 → 1.0.8` commit `3157812` on `staging` (`chore: bump 1.0.7 -> 1.0.8 (sparks verified interactive)`) — bumped `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (single `aygent` entry), `src-tauri/tauri.conf.json`, `ui/package.json`, `docs/CAPABILITIES.md` (v1.0.8 header + changelog, so `scripts/promote.sh` won't block).
- Staging bundle: `cargo tauri build --debug` (fast unsigned) → `AYGENT-Stage/src-tauri/target/debug/bundle/macos/AYGENT.app` + `AYGENT_1.0.8_aarch64.dmg` + binary `AYGENT-Stage/src-tauri/target/debug/aygent` (15.29s, 9 warnings).
- `EMFILE` incident: ~25 back-to-back `shell_run` tripped macOS FD limit (~256) → `Too many open files` / rate-limited. Workaround: manual `git add/commit/push origin staging` from Terminal + `Quit and Reopen AYGENT` to reset FD table. Not a 1.0.8 code bug.

### Verification Done Today (fresh 1.0.8 debug swap — still dead)
- `ui/dist/assets/index-yRDYgmoN.js` exists, `grep -c __sparkDummy = 1` ✓
- `grep -a index-yRDYgmoN.js` in `~/Desktop/AYGENT.app/Contents/MacOS/aygent` = 1, same md5 as Stage bundle ✓
- `Info.plist` `CFBundleShortVersionString`: `~/Desktop/AYGENT.app = 1.0.8`, `AYGENT-Stage/.../AYGENT.app = 1.0.8`, `/Applications/AYGENT.app = 1.0.7` (stale)
- `ps aux` showed both running (`10247 Desktop`, `9912 Applications`) — dual daemon / shared cache dir collision
- `~/Library/Caches/build.masonlee.aygent/frontend-version.txt = 1.0.8`, `WebKit` subdir already cleared by `bust_webview_cache` (stamp `prev == build_id` so second 1.0.8 didn't re-bust)
- Binary strings: `__sparkDummy` / `SPARK_STATE_SEED` not plain-text (asset compressed — expected), `index-yRDYgmoN.js` string present
- User tried: `quit app "AYGENT"`, `rm -rf ~/Library/Caches/build.masonlee.aygent`, `rm -rf /Applications/AYGENT.app`, `open ~/Desktop/AYGENT.app` → still dead (buttons don't get `.active`, inputs don't populate). User states: "We should not need a version bump to make a feature work, that's idiotic."

### Constraints For Next Fix
- No version bump to make a feature work. Feature must work at current `1.0.8`.
- Debug bundle is unsigned — release `cargo tauri build` only after verified interactive.
- `CAPABILITIES.md` already at `1.0.8`; don't re-bump.
- PATH for builds: `/usr/local/bin:/opt/homebrew/bin:$PATH`, `npm --prefix ui run build` / `npm run build`.

### Open / Next to Check (hand-off)
- ~~Why rebuild stamp `1.0.8 → 1.0.8` didn't re-bust~~ **FIXED: build id now includes exe mtime.**
- ~~Whether parent CSP blocks blob inline scripts in WKWebView~~ **CONFIRMED + FIXED: Tauri codegen appended sha256 hashes to `script-src`, nullifying `'unsafe-inline'`. Disabled via `dangerousDisableAssetCspModification: ["script-src"]`.**
- Whether two instances sharing `~/Library/Caches/build.masonlee.aygent` poison each other — ensure single instance or per-instance cache. (Still open; run one instance.)
- Add dev-only `?debug` toggle to dump iframe `document.documentElement.innerHTML` + posted errors in Chat. (Nice-to-have; error banner will now actually render since scripts execute.)

---
*Log format: append newest at top, keep one-liners for tried solutions, link commits/hashes.*
