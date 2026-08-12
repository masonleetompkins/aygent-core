// AYGENT — SPARK CHROME. The single source of truth for how a Spark LOOKS +
// how it PERSISTS + why its buttons actually work.
//
// The lesson (Mason, 2026-08-08, x3): the model CANNOT be trusted to produce good
// CSS — left to itself it ships serif Times, bare buttons, a vertical mess. Even
// "respect the author's <style>" backfired: the model emits a lazy full document
// with weak styles and our design system never applies. So the rule is now
// ABSOLUTE: we ALWAYS strip whatever <style>/<html>/<head>/<body> the model wrote,
// keep only its structural markup + <script>, and wrap it in AYGENT's design
// system. The model's job is STRUCTURE; AYGENT owns the LOOK, no exceptions.
// Beautiful out of the box, guaranteed.
//
// THE INTERACTIVITY FIX (Mason, 2026-08-12): Sparks' buttons didn't click,
// checklists were dead, and nothing survived a tab switch. Root cause: the iframe
// runs in an OPAQUE origin (sandbox="allow-scripts", blob: URL, no
// allow-same-origin — the correct fail-closed isolation; adding allow-same-origin
// to a same-origin blob would let a Spark reach parent.document and REMOVE its own
// sandbox = a jail escape, so we do NOT do that). In an opaque origin, ANY access
// to localStorage/sessionStorage/cookies THROWS a SecurityError — so a model's
// `localStorage.getItem(...)` on line 1 aborted the WHOLE script before a single
// addEventListener ran. CSS :hover still worked (no JS), so it LOOKED alive but
// every interaction was dead.
//
// The fix keeps the sandbox exactly as isolated and injects a small RUNTIME into
// the wrapped document BEFORE the Spark's markup:
//   1. a localStorage/sessionStorage SHIM (in-memory, never throws) so the script
//      survives — this alone brings buttons + checkboxes to life;
//   2. a postMessage BRIDGE + window.spark API that persists to the HOST, which
//      writes Sparks/<slug>/state.json through the jail. So state survives a tab
//      switch and Sparks become real mini-apps with a tiny per-Spark DB.
// The host seeds the saved state INTO the frame before the Spark runs, so a
// revisit restores instantly.

const SPARK_BASE_CSS = `
:root{
  --bg:#ffffff; --surface:#ffffff; --text:#0a0a0a; --muted:#5c5c5c; --faint:#9a9a9a;
  --line:#e6e6e6; --accent:#0a0a0a; --accent-ink:#ffffff;
  --radius:14px; --radius-sm:9px; --pad:16px; --gap:12px;
  --shadow:0 2px 8px rgba(0,0,0,.06),0 8px 24px rgba(0,0,0,.08);
}
@media (prefers-color-scheme: dark){
  :root{ --bg:#0b0b0c; --surface:#161618; --text:#f4f4f5; --muted:#a1a1aa; --faint:#6b6b70;
    --line:#2a2a2e; --accent:#ffffff; --accent-ink:#0a0a0a; --shadow:0 0 0 1px rgba(255,255,255,.04),0 8px 28px rgba(0,0,0,.5); }
}
*,*::before,*::after{box-sizing:border-box}
html{ -webkit-text-size-adjust:100%; }
/* Force the whole tree onto the system sans stack so a stray inline/browser
   default can never fall back to Times. !important on the root is the belt +
   suspenders that the last build lacked. */
*{
  font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text","Segoe UI",system-ui,Roboto,Helvetica,Arial,sans-serif !important;
}
html,body{margin:0}
body{
  background:var(--bg); color:var(--text);
  font-size:15px; line-height:1.5; -webkit-font-smoothing:antialiased;
  padding:20px; max-width:640px; margin:0 auto;
}
h1{font-size:22px;font-weight:800;letter-spacing:-.01em;margin:0 0 2px}
h2{font-size:17px;font-weight:700;margin:18px 0 8px}
h3{font-size:15px;font-weight:700;margin:14px 0 6px}
p{margin:0 0 12px}
.sub,.muted{color:var(--muted);font-size:13px}
.sub{margin:0 0 18px}
label,.label{display:block;font-size:12px;font-weight:600;color:var(--muted);text-transform:uppercase;letter-spacing:.06em;margin:14px 0 6px}
.card{background:var(--surface);border:1.5px solid var(--line);border-radius:var(--radius);padding:var(--pad);box-shadow:var(--shadow);margin:0 0 14px}
.grid{display:grid;gap:var(--gap);grid-template-columns:repeat(auto-fit,minmax(120px,1fr))}
.row{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:9px 0;border-bottom:1px solid var(--line)}
.row:last-child{border-bottom:none}
.row .k{color:var(--muted);font-size:14px;text-transform:none;letter-spacing:0;font-weight:400;margin:0}
.row .v{font-weight:700;font-variant-numeric:tabular-nums}
.stat{font-size:30px;font-weight:800;letter-spacing:-.02em;font-variant-numeric:tabular-nums}
/* Inputs */
input,select,textarea{
  font-size:15px; width:100%; padding:11px 13px; color:var(--text); background:var(--bg);
  border:1.5px solid var(--line); border-radius:var(--radius-sm); outline:none;
}
input:focus,select:focus,textarea:focus{border-color:var(--accent)}
.input-money{display:flex;align-items:center;gap:8px;border:1.5px solid var(--line);border-radius:var(--radius-sm);padding:0 13px;background:var(--bg)}
.input-money span{color:var(--muted);font-weight:600}
.input-money input{border:none;padding:11px 0;background:none}
/* Checkboxes/radios: let the accent theme them + give them room to be tappable. */
input[type=checkbox],input[type=radio]{width:auto;accent-color:var(--accent);width:18px;height:18px;cursor:pointer;vertical-align:middle}
/* A checklist row: a tappable line with a checkbox + label. */
.check{display:flex;align-items:center;gap:10px;padding:10px 0;border-bottom:1px solid var(--line);cursor:pointer}
.check:last-child{border-bottom:none}
.check.done{color:var(--muted);text-decoration:line-through}
/* Buttons — real, filled, tappable (kills the bare grey pill default) */
button,.btn{
  font-size:15px; font-weight:600; cursor:pointer; padding:11px 16px; line-height:1;
  border-radius:var(--radius-sm); border:1.5px solid var(--accent);
  background:var(--accent); color:var(--accent-ink); transition:filter .12s,transform .04s;
}
button:hover,.btn:hover{filter:brightness(1.08)}
button:active,.btn:active{transform:translateY(1px)}
button.secondary,.btn.secondary{background:var(--surface);color:var(--text);border-color:var(--line)}
/* Segmented control (e.g. tip % options) */
.seg{display:grid;grid-auto-flow:column;grid-auto-columns:1fr;gap:8px;margin:4px 0 4px}
.seg button{background:var(--surface);color:var(--text);border:1.5px solid var(--line)}
.seg button[aria-pressed="true"],.seg button.active{background:var(--accent);color:var(--accent-ink);border-color:var(--accent)}
/* Stepper */
.stepper{display:inline-flex;align-items:center;border:1.5px solid var(--line);border-radius:var(--radius-sm);overflow:hidden}
.stepper button{background:var(--surface);color:var(--text);border:none;border-radius:0;width:40px;padding:9px 0;font-size:18px}
.stepper .val{min-width:46px;text-align:center;font-weight:700;font-variant-numeric:tabular-nums}
table{width:100%;border-collapse:collapse}
th,td{text-align:left;padding:9px 10px;border-bottom:1px solid var(--line)}
th{font-size:12px;color:var(--muted);text-transform:uppercase;letter-spacing:.06em}
`;

// The RUNTIME injected into every Spark BEFORE its own markup + script. It:
//  (a) shims localStorage/sessionStorage so an opaque-origin frame never throws
//      (the bug that killed every button/checkbox on load);
//  (b) hydrates them from state the host seeds via SPARK_STATE_SEED;
//  (c) mirrors localStorage writes AND exposes window.spark.{get,set,list,remove}
//      to the host over postMessage, which persists to Sparks/<slug>/state.json.
// This runs FIRST so the Spark's script sees a working localStorage on line 1.
const SPARK_RUNTIME = `
(function(){
  // Host seeds the saved KV blob here right before this document loads.
  var SEED = (window.__SPARK_STATE_SEED && typeof window.__SPARK_STATE_SEED === 'object') ? window.__SPARK_STATE_SEED : {};
  var store = {};
  try { for (var k in SEED) if (Object.prototype.hasOwnProperty.call(SEED,k)) store[k] = SEED[k]; } catch(e){}

  function post(msg){ try { parent.postMessage(Object.assign({__spark:true}, msg), '*'); } catch(e){} }

  // localStorage values are STRINGS. We keep them as strings in store and
  // persist each change to the host (debounced) so a tab switch restores them.
  function persistKey(key, value){ post({ kind:'set', key:key, value:value }); }
  function persistRemove(key){ post({ kind:'set', key:key, value:null }); }

  function makeStorage(persist){
    var mem = {};
    // Seed persistent storage from SEED (memory-only for sessionStorage).
    if (persist) { for (var k in store) mem[k] = String(store[k]); }
    var api = {
      getItem: function(k){ k=String(k); return Object.prototype.hasOwnProperty.call(mem,k)? mem[k] : null; },
      setItem: function(k,v){ k=String(k); v=String(v); mem[k]=v; if(persist) persistKey(k,v); },
      removeItem: function(k){ k=String(k); delete mem[k]; if(persist) persistRemove(k); },
      clear: function(){ for (var k in mem) { if(persist) persistRemove(k); delete mem[k]; } },
      key: function(i){ return Object.keys(mem)[i] || null; },
    };
    Object.defineProperty(api, 'length', { get: function(){ return Object.keys(mem).length; } });
    return api;
  }

  // Replace the throwing opaque-origin storage with a working shim. Wrapped in
  // try/catch because some engines make these read-only; defineProperty is the
  // reliable override in a sandboxed frame.
  try { Object.defineProperty(window, 'localStorage',   { value: makeStorage(true),  configurable: true }); } catch(e){}
  try { Object.defineProperty(window, 'sessionStorage', { value: makeStorage(false), configurable: true }); } catch(e){}

  // A richer async API for Sparks that want structured JSON, not just strings.
  // window.spark.get/set/remove persist through the same jailed channel.
  window.spark = {
    get: function(key){ return (key in store) ? store[key] : undefined; },
    all: function(){ return Object.assign({}, store); },
    set: function(key, value){ store[key] = value; post({ kind:'set', key:String(key), value:value }); },
    remove: function(key){ delete store[key]; post({ kind:'set', key:String(key), value:null }); },
  };
})();
`;

/** Pull just the usable parts out of whatever the model emitted:
 *  the inner <body> markup (or the whole string if it's already body-only),
 *  with any <style>, <head>, <!doctype>, <html>/<body> wrappers REMOVED. We keep
 *  <script> tags (the Spark's logic) and structural markup. This is what makes
 *  the design system unconditional — the model can't smuggle in its own CSS. */
function extractSparkBody(raw: string): string {
  let s = raw ?? "";
  // If there's a <body>…</body>, take its inner content.
  const bodyMatch = /<body[^>]*>([\s\S]*?)<\/body>/i.exec(s);
  if (bodyMatch) s = bodyMatch[1];
  // Strip doctype + html/head wrappers if any survived (body-less full doc).
  s = s.replace(/<!doctype[^>]*>/gi, "");
  s = s.replace(/<\/?html[^>]*>/gi, "");
  s = s.replace(/<head[\s\S]*?<\/head>/gi, "");
  s = s.replace(/<\/?body[^>]*>/gi, "");
  // Remove ALL <style> blocks — AYGENT owns the styling.
  s = s.replace(/<style[\s\S]*?<\/style>/gi, "");
  // Remove <link rel="stylesheet"> (no external CSS; keep script CDNs though).
  s = s.replace(/<link[^>]+rel=["']?stylesheet["']?[^>]*>/gi, "");
  return s.trim();
}

/** Wrap a Spark's html into a complete, always-AYGENT-styled document for the
 *  iframe. The design system ALWAYS applies — the model's markup is treated as
 *  body structure only. `state` (if given) is seeded into the frame so the Spark
 *  restores its saved KV before its script runs. */
export function wrapSparkHtml(html: string, state?: Record<string, unknown>): string {
  const body = extractSparkBody(html || "");
  // Seed must be embedded as a JS literal BEFORE the runtime + the Spark script.
  // JSON.stringify with </script guarded so an embedded string can't break out.
  const seedJson = JSON.stringify(state || {}).replace(/<\//g, "<\\/");
  return `<!doctype html><html><head><meta charset="utf-8">` +
    `<meta name="viewport" content="width=device-width,initial-scale=1">` +
    `<style>${SPARK_BASE_CSS}</style>` +
    `<script>window.__SPARK_STATE_SEED=${seedJson};</script>` +
    `<script>${SPARK_RUNTIME}</script>` +
    `</head><body>${body}</body></html>`;
}

// The app runs under a strict CSP (default-src 'self'; no style-src 'unsafe-inline').
// A `srcDoc` iframe inherits the PARENT's origin/CSP, so the spark's inline <style>
// gets BLOCKED inside AYGENT (it renders serif/unstyled) even though the exact same
// html renders perfectly in Safari (no CSP). Fix: load the wrapped document from a
// `blob:` URL instead of srcDoc. A blob URL is a SEPARATE origin, so the app's CSP
// does not apply to it and the spark may use its own inline styles freely. Sandbox
// stays allow-scripts (no same-origin) so the spark still can't reach the app/files
// — the storage shim + postMessage bridge give it persistence WITHOUT weakening the
// jail (see the header note: allow-same-origin on a blob would be a jail escape).
// This React hook returns a stable blob URL for the given html + seed state and
// revokes it on change/unmount (no leaks). Import React lazily to keep this module
// UI-agnostic.
import { useEffect, useState } from "react";

export function useSparkBlobUrl(html: string, state?: Record<string, unknown>): string | null {
  const [url, setUrl] = useState<string | null>(null);
  // Serialize the seed for the effect dep so a state change re-mints the doc.
  const seedKey = JSON.stringify(state || {});
  useEffect(() => {
    const doc = wrapSparkHtml(html || "", state);
    const blob = new Blob([doc], { type: "text/html" });
    const u = URL.createObjectURL(blob);
    setUrl(u);
    return () => { URL.revokeObjectURL(u); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [html, seedKey]);
  return url;
}

// The message shape a Spark posts to persist state. The parent validates
// event.source === iframe.contentWindow before honoring it, so no other frame
// can write another Spark's state.
export type SparkStateMsg = { __spark: true; kind: "set"; key: string; value: unknown };

export function isSparkStateMsg(data: unknown): data is SparkStateMsg {
  return !!data && typeof data === "object"
    && (data as { __spark?: unknown }).__spark === true
    && (data as { kind?: unknown }).kind === "set"
    && typeof (data as { key?: unknown }).key === "string";
}
