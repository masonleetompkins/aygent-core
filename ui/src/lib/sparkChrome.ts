// AYGENT — SPARK CHROME. The single source of truth for how a Spark LOOKS +
// how it PERSISTS + why its buttons actually work.
//
// 2026-08-17 FIX: the reload loop was fixed (seed-once, postMessage live),
// but the tip calculator was STILL dead: its script ran as a bare <script>
// in the wrapped body. Any uncaught throw (e.g. getElementById→null →
// .addEventListener on null) kills the ENTIRE script and no error is
// visible inside an opaque blob iframe. Also WKWebView's blob iframes
// swallow <script> if the CSP/cross-origin handling stutters.
// Fix: wrap the Spark's markup script in a fault-tolerant boot harness
// that (a) defers until DOM is ready, (b) isolates throws per-listener,
// (c) surfaces the error banner inside the Spark so we SEE failures.

const SPARK_BASE_CSS = `
:root{
  --bg:#ffffff; --surface:#ffffff; --text:#0a0a0a; --muted:#5c5c5c; --faint:#9a9a9a;
  --line:#e6e6e6; --accent:#0a0a0a; --accent-ink:#ffffff;
  --radius:14px; --radius-sm:9px; --pad:16px; --gap:12px;
  --shadow:0 2px 8px rgba(0,0,0,.06),0 8px 24px rgba(0,0,0,.08);
}
*,*::before,*::after{box-sizing:border-box}
html{ -webkit-text-size-adjust:100%; }
*{ font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text","Segoe UI",system-ui,Roboto,Helvetica,Arial,sans-serif !important; }
html,body{margin:0}
body{ background:var(--bg); color:var(--text); font-size:15px; line-height:1.5; -webkit-font-smoothing:antialiased; padding:20px; max-width:640px; margin:0 auto; }
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
input,select,textarea{ font-size:15px; width:100%; padding:11px 13px; color:var(--text); background:var(--bg); border:1.5px solid var(--line); border-radius:var(--radius-sm); outline:none; }
input:focus,select:focus,textarea:focus{border-color:var(--accent)}
.input-money{display:flex;align-items:center;gap:8px;border:1.5px solid var(--line);border-radius:var(--radius-sm);padding:0 13px;background:var(--bg)}
.input-money span{color:var(--muted);font-weight:600}
.input-money input{border:none;padding:11px 0;background:none}
input[type=checkbox],input[type=radio]{width:auto;accent-color:var(--accent);width:18px;height:18px;cursor:pointer;vertical-align:middle}
.check{display:flex;align-items:center;gap:10px;padding:10px 0;border-bottom:1px solid var(--line);cursor:pointer}
.check:last-child{border-bottom:none}
.check.done{color:var(--muted);text-decoration:line-through}
button,.btn{ font-size:15px; font-weight:600; cursor:pointer; padding:11px 16px; line-height:1; border-radius:var(--radius-sm); border:1.5px solid var(--accent); background:var(--accent); color:var(--accent-ink); transition:filter .12s,transform .04s; }
button:hover,.btn:hover{filter:brightness(1.08)}
button:active,.btn:active{transform:translateY(1px)}
button.secondary,.btn.secondary{background:var(--surface);color:var(--text);border-color:var(--line)}
.seg{display:grid;grid-auto-flow:column;grid-auto-columns:1fr;gap:8px;margin:4px 0 4px}
.seg button{background:var(--surface);color:var(--text);border:1.5px solid var(--line)}
.seg button[aria-pressed="true"],.seg button.active{background:var(--accent);color:var(--accent-ink);border-color:var(--accent)}
.stepper{display:inline-flex;align-items:center;border:1.5px solid var(--line);border-radius:var(--radius-sm);overflow:hidden}
.stepper button{background:var(--surface);color:var(--text);border:none;border-radius:0;width:40px;padding:9px 0;font-size:18px}
.stepper .val{min-width:46px;text-align:center;font-weight:700;font-variant-numeric:tabular-nums}
table{width:100%;border-collapse:collapse}
th,td{text-align:left;padding:9px 10px;border-bottom:1px solid var(--line)}
th{font-size:12px;color:var(--muted);text-transform:uppercase;letter-spacing:.06em}
/* Visible error banner for script throws (otherwise silent in opaque iframe) */
.__sparkErr{margin:12px 0;padding:10px 12px;border:1px solid #f5a3a3;background:#fff1f1;color:#8a1c1c;border-radius:8px;font:12px ui-monospace,monospace;white-space:pre-wrap;word-break:break-word}
`;

export type SparkThemeVars = Record<string, string>;
export function getAppTheme(): { mode: string; accentOn: boolean; vars: SparkThemeVars } {
  if (typeof document === "undefined") return { mode: "light", accentOn: false, vars: {} };
  const root = document.documentElement;
  const mode = root.getAttribute("data-theme") || "light";
  const accentOn = root.getAttribute("data-accent") === "on";
  const cs = getComputedStyle(root);
  const names = ["--bg","--surface","--text","--muted","--faint","--line","--accent","--accent-rgb","--accent-ink","--elevation","--radius","--radius-sm"];
  const vars: SparkThemeVars = {};
  for (const n of names) { const v = cs.getPropertyValue(n).trim(); if (v) vars[n]=v; }
  if (!vars["--accent-ink"]) vars["--accent-ink"]= mode==="dark"||mode==="matrix" ? "#0a0a0a" : "#ffffff";
  return { mode, accentOn, vars };
}
function themeCss(vars: SparkThemeVars): string {
  if (!vars || Object.keys(vars).length===0) return "";
  return `:root{${Object.entries(vars).map(([k,v])=>`${k}:${v};`).join("")}}`;
}

const SPARK_RUNTIME = `
(function(){
  var SEED=(window.__SPARK_STATE_SEED&&typeof window.__SPARK_STATE_SEED==='object')?window.__SPARK_STATE_SEED:{};
  var store={}; try{for(var k in SEED) if(Object.prototype.hasOwnProperty.call(SEED,k)) store[k]=SEED[k];}catch(e){}
  function post(msg){ try{parent.postMessage(Object.assign({__spark:true},msg),'*');}catch(e){} }
  window.addEventListener('message',function(ev){
    try{
      var d=ev.data;
      if(!d||!d.__sparkTheme||!d.__sparkTheme.vars) return;
      var vars=d.__sparkTheme.vars, root=document.documentElement;
      for(var k in vars) if(Object.prototype.hasOwnProperty.call(vars,k)) root.style.setProperty(k, vars[k]);
      if(d.__sparkTheme.mode) root.setAttribute('data-theme', d.__sparkTheme.mode);
      if(d.__sparkTheme.accentOn) root.setAttribute('data-accent','on'); else root.removeAttribute('data-accent');
    }catch(e){}
  });
  function persistKey(k,v){ post({kind:'set',key:String(k),value:String(v)}); }
  function persistRemove(k){ post({kind:'set',key:String(k),value:null}); }
  function makeStorage(persist){
    var mem={}; if(persist){ for(var k in store) mem[k]=String(store[k]); }
    return {
      getItem:function(k){ k=String(k); return Object.prototype.hasOwnProperty.call(mem,k)?mem[k]:null; },
      setItem:function(k,v){ k=String(k); v=String(v); mem[k]=v; if(persist) persistKey(k,v); },
      removeItem:function(k){ k=String(k); delete mem[k]; if(persist) persistRemove(k); },
      clear:function(){ for(var k in mem){ if(persist) persistRemove(k); delete mem[k]; } },
      key:function(i){ return Object.keys(mem)[i]||null; },
      get length(){ return Object.keys(mem).length; }
    };
  }
  try{ Object.defineProperty(window,'localStorage',{value:makeStorage(true),configurable:true}); }catch(e){}
  try{ Object.defineProperty(window,'sessionStorage',{value:makeStorage(false),configurable:true}); }catch(e){}
  window.spark={ get:function(k){return (k in store)?store[k]:undefined;}, all:function(){return Object.assign({},store);}, set:function(k,v){store[k]=v; post({kind:'set',key:String(k),value:v});}, remove:function(k){delete store[k]; post({kind:'set',key:String(k),value:null});} };
  // Global error → post to parent + show banner (otherwise silent in blob iframe)
  window.addEventListener('error', function(ev){
    try{
      var msg = (ev.error && ev.error.stack) ? ev.error.stack : (ev.message || String(ev.error||'error'));
      parent.postMessage({__spark:true, kind:'error', message: msg.slice(0,2000)}, '*');
      var b=document.createElement('div'); b.className='__sparkErr'; b.textContent='Spark error: '+msg.slice(0,800); document.body && document.body.prepend(b);
    }catch(e){}
  });
})();
`;

function extractSparkBody(raw: string): string {
  let s = raw ?? "";
  const bodyMatch = /<body[^>]*>([\s\S]*?)<\/body>/i.exec(s);
  if (bodyMatch) s = bodyMatch[1];
  s = s.replace(/<!doctype[^>]*>/gi, "");
  s = s.replace(/<\/?html[^>]*>/gi, "");
  s = s.replace(/<head[\s\S]*?<\/head>/gi, "");
  s = s.replace(/<\/?body[^>]*>/gi, "");
  s = s.replace(/<style[\s\S]*?<\/style>/gi, "");
  s = s.replace(/<link[^>]+rel=["']?stylesheet["']?[^>]*>/gi, "");
  return s.trim();
}

/** Extract raw <script> bodies from the spark markup so we can re-emit them
 *  in a safe boot harness (avoids inline parse-abort when one throw kills all). */
function extractScripts(body: string): { withoutScripts: string; scripts: string[] } {
  const scripts: string[] = [];
  const withoutScripts = body.replace(/<script[^>]*>([\s\S]*?)<\/script>/gi, (_m, code) => {
    const srcMatch = /<script[^>]*\ssrc=["']([^"']+)["']/i.exec(_m);
    if (srcMatch) {
      // External CDN — keep as real script tag, don't inline
      scripts.push(`__EXTERNAL_SRC__:${srcMatch[1]}`);
    } else {
      scripts.push(code);
    }
    return "";
  });
  return { withoutScripts, scripts };
}

export function wrapSparkHtml(html: string, state?: Record<string, unknown>, themeVars?: SparkThemeVars, themeMode?: string, themeAccentOn?: boolean): string {
  const bodyRaw = extractSparkBody(html || "");
  const { withoutScripts, scripts } = extractScripts(bodyRaw);
  const seedJson = JSON.stringify(state || {}).replace(/<\//g, "<\\/");
  const tCss = themeCss(themeVars || {});
  const mode = themeMode || "light";
  const accentAttr = themeAccentOn ? ' data-accent="on"' : "";
  // Re-emit external scripts as real <script src> tags; inline scripts go through
  // a try/catch harness that also defers until DOM ready (the Spark markup is
  // already in the body above, so getElementById succeeds).
  let scriptTags = "";
  for (const code of scripts) {
    if (code.startsWith("__EXTERNAL_SRC__:")) {
      const src = code.slice("__EXTERNAL_SRC__:".length);
      scriptTags += `<script src="${src}"><\/script>`;
    } else {
      // Harness: run after DOM is parsed, surface any throw, don't let one Spark
      // bug kill the whole frame. Also flush an init ok post so host knows boot succeeded.
      const wrapped = `
(function(){
  function __sparkBoot(){
    try {
      ${code}
      try{ parent.postMessage({__spark:true, kind:'boot', ok:true}, '*'); }catch(e){}
    } catch(e){
      var msg = (e && e.stack) ? e.stack : String(e);
      try{ parent.postMessage({__spark:true, kind:'error', message: msg.slice(0,2000)}, '*'); }catch(_){}
      var b=document.createElement('div'); b.className='__sparkErr'; b.textContent='Spark boot error: '+msg.slice(0,800); document.body && document.body.prepend(b);
      console.error('[Spark]', e);
    }
  }
  if(document.readyState==='loading'){ document.addEventListener('DOMContentLoaded', __sparkBoot); } else { __sparkBoot(); }
})();
`;
      scriptTags += `<script>${wrapped}<\/script>`;
    }
  }
  return `<!doctype html><html data-theme="${mode}"${accentAttr}><head><meta charset="utf-8">` +
    `<meta name="viewport" content="width=device-width,initial-scale=1">` +
    `<style>${SPARK_BASE_CSS}</style>` +
    (tCss ? `<style id="__sparkTheme">${tCss}</style>` : "") +
    `<script>window.__SPARK_STATE_SEED=${seedJson};</script>` +
    `<script>${SPARK_RUNTIME}</script>` +
    `</head><body>${withoutScripts}${scriptTags}</body></html>`;
}

import { useEffect, useState, useRef } from "react";

export function useSparkBlobUrl(html: string, initialState?: Record<string, unknown>): string | null {
  const [url, setUrl] = useState<string | null>(null);
  const htmlRef = useRef<string>("");
  const seedKeyRef = useRef<string>("");
  const seedKey = JSON.stringify(initialState || {});
  const theme = getAppTheme();
  useEffect(() => {
    if (htmlRef.current === (html || "") && seedKeyRef.current === seedKey) return;
    htmlRef.current = html || "";
    seedKeyRef.current = seedKey;
    const doc = wrapSparkHtml(html || "", initialState, theme.vars, theme.mode, theme.accentOn);
    const blob = new Blob([doc], { type: "text/html" });
    const u = URL.createObjectURL(blob);
    setUrl(u);
    return () => { URL.revokeObjectURL(u); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [html, seedKey]);
  return url;
}

export function useSparkThemeSync(iframeRef: React.RefObject<HTMLIFrameElement | null>) {
  useEffect(() => {
    function push(){
      const iframe=iframeRef.current; if(!iframe?.contentWindow) return;
      const t=getAppTheme();
      try{ iframe.contentWindow.postMessage({__sparkTheme:t}, "*"); }catch{}
    }
    const obs=new MutationObserver(()=>push());
    obs.observe(document.documentElement,{attributes:true, attributeFilter:["data-theme","data-accent","style"]});
    const id=window.setTimeout(push,300), id2=window.setTimeout(push,900);
    return ()=>{ obs.disconnect(); window.clearTimeout(id); window.clearTimeout(id2); };
  }, [iframeRef]);
}

export type SparkStateMsg = { __spark: true; kind: "set"; key: string; value: unknown };
export function isSparkStateMsg(data: unknown): data is SparkStateMsg {
  return !!data && typeof data==="object"
    && (data as {__spark?:unknown}).__spark===true
    && (data as {kind?:unknown}).kind==="set"
    && typeof (data as {key?:unknown}).key==="string";
}
