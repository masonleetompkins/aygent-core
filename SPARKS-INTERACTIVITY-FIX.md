# Sparks: make them real mini-apps (interactivity + persistence)

Mason's four symptoms, root-caused from source (2026-08-12, Cleo):

1. **Buttons hover but clicks do nothing.**
2. **Checklists invisible / not selectable.**
3. **State doesn't survive a tab switch.**
4. **Sparks are purely cosmetic — no real data/actions.**

## Root cause (precise)

Every Spark renders in an iframe with:
```
sandbox="allow-scripts allow-popups allow-forms"
src={blobUrl}          // a blob: URL = OPAQUE origin under this sandbox
```
(`ui/src/screens/Sparks.tsx`, and the live chat preview.)

**Opaque origin ⇒ `localStorage`/`sessionStorage`/`cookie`/`IndexedDB` THROW on
access.** Models routinely touch `localStorage` at the top of their `<script>`
(restore a checklist, a counter). That throw is uncaught → **the whole script
aborts before any `addEventListener` runs.** CSS `:hover` still works (no JS), so
the Spark *looks* alive but every interaction is dead. Checkboxes a script builds
never appear. → **#1, #2.**

**No persistence layer + a fresh blob URL is minted on every `html` change /
remount** (`useSparkBlobUrl`), so the iframe reloads from zero each visit.
→ **#3.** And there's no data/file bridge at all → **#4.**

## Why we CANNOT just add `allow-same-origin`

A blob: URL created by the parent carries the PARENT's origin. Per the HTML spec
(web.dev "Play safely in sandboxed IFrames"): a same-origin frame with BOTH
`allow-same-origin` + `allow-scripts` can **reach into `parent.document` and
remove its own sandbox attribute** — a full jail escape. That would break
AYGENT's entire isolation model. Rejected.

## The fix — keep the sandbox opaque, add two safe layers

Sandbox stays `allow-scripts` (fully isolated, no escape). We inject a small
runtime into the wrapped document (in `sparkChrome.ts`, before the Spark's own
markup) that fixes both real problems:

### A. Storage shim (fixes #1, #2 immediately)
Define `localStorage` / `sessionStorage` as in-memory objects on `window` so the
Spark's script never throws. Backed by a plain object; `sessionStorage` is
memory-only, `localStorage` is memory + synced to disk via the bridge (B).
This is what brings buttons/checkboxes to life — zero security change.

### B. postMessage persistence bridge (fixes #3, #4)
Opaque frames may still `postMessage` to `parent`. The injected runtime exposes:
- a synchronous `localStorage` (writes debounce-flush to the host), and
- `window.spark.get(key)/set(key,val)/list()` async helpers,
both marshalled over `postMessage`. The parent (`Sparks.tsx` + the chat preview)
validates `event.source === iframe.contentWindow`, resolves the CURRENT slug, and
calls new jailed Rust commands that read/write `Sparks/<slug>/state.json` (KV) and
`Sparks/<slug>/data/*` — all through the broker (same jail as every file tool).
On load, the host seeds the frame with the saved KV blob so state is restored
before the Spark script runs. → real persistence + a tiny per-Spark DB.

### C. Per-Spark folder
`spark_save` already writes `Sparks/<slug>/index.html` + `spark.json`. State lands
beside it as `Sparks/<slug>/state.json`; larger assets under `Sparks/<slug>/data/`.
The folder-per-spark structure Mason wanted already exists — we just use it.

## Surfaces to touch
- `ui/src/lib/sparkChrome.ts` — inject the runtime (shim + bridge) into the wrap;
  export a hook that wires the parent side of the bridge.
- `ui/src/screens/Sparks.tsx` — wire the bridge to the selected slug + new commands.
- Chat inline preview (wherever spark_preview renders) — same bridge, slug = the
  preview slug (state persists once saved).
- `src-tauri/src/lib.rs` — `spark_state_get` / `spark_state_set` (jailed KV) and
  optionally `spark_data_write`/`spark_data_read` for the data/ subfolder.
- Docs: `docs/CAPABILITIES.md` Sparks section + the spark_preview tool blurb
  (tell the model it now has `window.spark` persistence).

## Security invariant (unchanged)
Frame stays opaque + `allow-scripts` only: no `parent.document`, no files, no net.
The ONLY new reach is a narrow, validated postMessage KV channel the HOST mediates
and jails to `Sparks/<slug>/`. Same mediated-reach model as fetch_url / the path
broker — the Spark never touches disk directly.
