// AYGENT — SPARK CHROME. The single source of truth for how a Spark LOOKS.
//
// The lesson (Mason, 2026-08-08): asking the model to hand-write good CSS from a
// prose template is unreliable — it shipped serif Times, unstyled buttons, and a
// vertical mess. So we take styling OUT of the model's hands: the model writes
// only STRUCTURE (semantic HTML with known class names), and AYGENT injects a
// complete, guaranteed-good stylesheet at RENDER time — for both the inline chat
// preview and the saved-library render, so preview == saved.
//
// If a Spark author DOES ship their own full document (they included their own
// <style> or a <!doctype>), we respect it and only ensure a sane base font +
// reset are present — a user asking for "a specific look" still gets it.

// The AYGENT default design system. Applied to EVERY spark unless the author
// shipped their own <style>. Tokens mirror the app (theme.css) but resolve to
// concrete values (the iframe is isolated — it can't read the app's CSS vars).
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
*{box-sizing:border-box}
html,body{margin:0}
body{
  background:var(--bg); color:var(--text);
  font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text","Segoe UI",system-ui,Roboto,Helvetica,Arial,sans-serif;
  font-size:15px; line-height:1.5; -webkit-font-smoothing:antialiased;
  padding:20px; max-width:640px; margin:0 auto;
}
h1{font-size:22px;font-weight:800;letter-spacing:-.01em;margin:0 0 2px}
h2{font-size:17px;font-weight:700;margin:18px 0 8px}
h3{font-size:15px;font-weight:700;margin:14px 0 6px}
p{margin:0 0 12px}
.sub,.muted{color:var(--muted);font-size:13px}
.sub{margin:0 0 18px}
.label{display:block;font-size:12px;font-weight:600;color:var(--muted);text-transform:uppercase;letter-spacing:.06em;margin:14px 0 6px}
.card{background:var(--surface);border:1.5px solid var(--line);border-radius:var(--radius);padding:var(--pad);box-shadow:var(--shadow);margin:0 0 14px}
.grid{display:grid;gap:var(--gap);grid-template-columns:repeat(auto-fit,minmax(120px,1fr))}
.row{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:9px 0;border-bottom:1px solid var(--line)}
.row:last-child{border-bottom:none}
.row .k{color:var(--muted);font-size:14px}
.row .v{font-weight:700;font-variant-numeric:tabular-nums}
.stat{font-size:30px;font-weight:800;letter-spacing:-.02em;font-variant-numeric:tabular-nums}
/* Inputs */
input,select,textarea{
  font:inherit; width:100%; padding:11px 13px; color:var(--text); background:var(--bg);
  border:1.5px solid var(--line); border-radius:var(--radius-sm); outline:none;
}
input:focus,select:focus,textarea:focus{border-color:var(--accent)}
.input-money{display:flex;align-items:center;gap:8px;border:1.5px solid var(--line);border-radius:var(--radius-sm);padding:0 13px;background:var(--bg)}
.input-money span{color:var(--muted);font-weight:600}
.input-money input{border:none;padding:11px 0;background:none}
/* Buttons */
button,.btn{
  font:inherit; font-weight:600; cursor:pointer; padding:11px 16px;
  border-radius:var(--radius-sm); border:1.5px solid var(--accent);
  background:var(--accent); color:var(--accent-ink); transition:filter .12s,transform .04s;
}
button:hover,.btn:hover{filter:brightness(1.08)}
button:active,.btn:active{transform:translateY(1px)}
button.secondary,.btn.secondary{background:var(--surface);color:var(--text);border-color:var(--line)}
/* Segmented control (e.g. tip % options) */
.seg{display:grid;grid-auto-flow:column;grid-auto-columns:1fr;gap:8px}
.seg button{background:var(--surface);color:var(--text);border:1.5px solid var(--line)}
.seg button[aria-pressed="true"],.seg button.active{background:var(--accent);color:var(--accent-ink);border-color:var(--accent)}
/* Stepper */
.stepper{display:inline-flex;align-items:center;gap:0;border:1.5px solid var(--line);border-radius:var(--radius-sm);overflow:hidden}
.stepper button{background:var(--surface);color:var(--text);border:none;border-radius:0;width:38px;padding:9px 0}
.stepper .val{min-width:44px;text-align:center;font-weight:700;font-variant-numeric:tabular-nums}
table{width:100%;border-collapse:collapse}
th,td{text-align:left;padding:9px 10px;border-bottom:1px solid var(--line)}
th{font-size:12px;color:var(--muted);text-transform:uppercase;letter-spacing:.06em}
`;

/** Does the author's html look like a COMPLETE document with its own styling?
 *  If so we don't impose our design system (they asked for a specific look). */
function isFullDocument(html: string): boolean {
  const h = html.toLowerCase();
  return h.includes("<!doctype") || h.includes("<html") || h.includes("<style");
}

/**
 * Wrap a Spark's html into a complete, well-styled document for the iframe.
 * - If the author shipped a full document (their own <style>/<html>), we return
 *   it as-is but PREPEND a tiny reset+font so a bare document never renders in
 *   Times/serif.
 * - Otherwise (the common case: the model wrote body structure with our class
 *   names), we wrap it in a full document with the AYGENT design system.
 */
export function wrapSparkHtml(html: string): string {
  const body = html ?? "";
  if (isFullDocument(body)) {
    // Respect the author's document; just guarantee a sane base if they omitted
    // a font (prevents the serif-Times default in a bare <html> shell).
    const inject = `<style>html,body{font-family:-apple-system,BlinkMacSystemFont,"SF Pro Text","Segoe UI",system-ui,sans-serif}</style>`;
    if (/<head[^>]*>/i.test(body)) return body.replace(/<head([^>]*)>/i, `<head$1>${inject}`);
    if (/<html[^>]*>/i.test(body)) return body.replace(/<html([^>]*)>/i, `<html$1><head>${inject}</head>`);
    return inject + body;
  }
  return `<!doctype html><html><head><meta charset="utf-8">` +
    `<meta name="viewport" content="width=device-width,initial-scale=1">` +
    `<style>${SPARK_BASE_CSS}</style></head><body>${body}</body></html>`;
}
