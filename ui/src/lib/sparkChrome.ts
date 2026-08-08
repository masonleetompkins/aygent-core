// AYGENT — SPARK CHROME. The single source of truth for how a Spark LOOKS.
//
// The lesson (Mason, 2026-08-08, x3): the model CANNOT be trusted to produce good
// CSS — left to itself it ships serif Times, bare buttons, a vertical mess. Even
// "respect the author's <style>" backfired: the model emits a lazy full document
// with weak styles and our design system never applies. So the rule is now
// ABSOLUTE: we ALWAYS strip whatever <style>/<html>/<head>/<body> the model wrote,
// keep only its structural markup + <script>, and wrap it in AYGENT's design
// system. The model's job is STRUCTURE; AYGENT owns the LOOK, no exceptions.
// Beautiful out of the box, guaranteed.

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
 *  body structure only. */
export function wrapSparkHtml(html: string): string {
  const body = extractSparkBody(html || "");
  return `<!doctype html><html><head><meta charset="utf-8">` +
    `<meta name="viewport" content="width=device-width,initial-scale=1">` +
    `<style>${SPARK_BASE_CSS}</style></head><body>${body}</body></html>`;
}
