# Sparks rendering — CSP investigation + fallback plan

## Confirmed facts
- Spark html + CSS is CORRECT (renders perfectly in Safari, file:// — no CSP there).
- Fails styled-less INSIDE the app's WKWebView iframe (Chat inline + Sparks tab).
- Root cause: the app's CSP strips the spark's inline <style>/<script>.
  Tauri v2 injects a nonce'd CSP from tauri.conf.json; a sandboxed iframe
  (no allow-same-origin, opaque origin) inherits the embedder CSP in WebKit.

## Attempt log
1. srcDoc + wrapSparkHtml design system  -> stripped (CSP).
2. blob: URL via useSparkBlobUrl + frame-src blob:  -> still stripped
   (sandboxed opaque frame still got the embedder CSP).
3. CSP: style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'
   https://cdn.jsdelivr.net  (commit 78b3b10)  <-- CURRENT, testing.

## FALLBACK (Secondary Road) if #3 STILL fails
The bulletproof approach: give the spark frame its OWN document served with its
OWN permissive CSP header, independent of the app CSP. Options, in order:

A. Tauri custom URI scheme / asset protocol for sparks:
   - Register a `spark://` (or use asset protocol) handler in Rust that serves
     the wrapped html with header `Content-Security-Policy: default-src 'self'
     'unsafe-inline' data: blob: https:` (permissive, but it's an ISOLATED
     origin — the sandbox still blocks reaching the app/files).
   - iframe src = spark://<id>. A real response with its own CSP is NOT subject
     to the embedder CSP. This is the cleanest, keeps the MAIN app CSP strict.

B. Serve via the existing local daemon/HTTP (127.0.0.1) with a permissive CSP
   header on the spark route; iframe points at http://127.0.0.1:<port>/spark/<id>.
   connect-src already allows ws://127.0.0.1; add http for frames.

C. LAST resort — a native WKWebView overlay (Mason's original idea). Heavy:
   native view layering + scroll sync + one-per-spark lifecycle in the chat log.
   Only if A and B both fail.

## Key open question if #3 fails
Does Tauri v2 apply the config CSP to blob/sandboxed subframes at all? If the
embedder CSP does NOT propagate to blob frames, then the real bug is elsewhere
(e.g. the built JS still stale, or the blob doc missing the <style>). Verify by
logging the actual wrapped html length + inspecting the frame — need a way to
see the rendered DOM, not guess.
