// check-invoke-case.mjs — Tauri 2 maps camelCase JS invoke args to snake_case
// Rust params. A snake_case key in an invoke({...}) literal is NEVER matched,
// so the command fails at runtime with "missing required key" while tsc stays
// green (Mason 10-08: sub_profile_save_token + access_token). This fails the
// UI build on any snake_case invoke key. Run: node scripts/check-invoke-case.mjs
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const ROOT = new URL("../src", import.meta.url).pathname;
const KEY_RE = /[{,]\s*([A-Za-z_$][\w$]*)\s*:/g;

function files(dir) {
  const out = [];
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    const s = statSync(p);
    if (s.isDirectory()) out.push(...files(p));
    else if (/\.(ts|tsx)$/.test(e)) out.push(p);
  }
  return out;
}

// Grab the {...} arg object of each invoke("cmd", {...}) — brace-balanced so
// multiline calls work. Returns [{file, line, keys[]}].
function invokeArgKeys(src) {
  const hits = [];
  const re = /invoke\s*\(\s*"[^"]+"\s*,\s*\{/g;
  let m;
  while ((m = re.exec(src))) {
    let depth = 0, i = m.index + m[0].length - 1, startLine = src.slice(0, m.index).split("\n").length;
    for (; i < src.length; i++) {
      const c = src[i];
      if (c === "{" ) depth++;
      else if (c === "}") { depth--; if (depth === 0) break; }
      else if (c === '"' || c === "'" || c === "`") {
        // skip string literals (they may contain braces/colons)
        const q = c;
        i++;
        for (; i < src.length; i++) {
          if (src[i] === "\\") { i++; continue; }
          if (src[i] === q) break;
          if (q === "`" && src[i] === "$" && src[i + 1] === "{") {
            let d = 1; i += 2;
            for (; i < src.length && d > 0; i++) {
              if (src[i] === "{") d++;
              else if (src[i] === "}") d--;
            }
            i--;
          }
        }
      }
    }
    const body = src.slice(m.index, i + 1);
    const keys = [...body.matchAll(KEY_RE)].map((k) => k[1]);
    hits.push({ line: startLine, keys });
  }
  return hits;
}

let bad = 0;
for (const f of files(ROOT)) {
  const src = readFileSync(f, "utf8");
  for (const h of invokeArgKeys(src)) {
    for (const k of h.keys) {
      if (k.includes("_")) {
        console.error(`${f}:${h.line}: snake_case invoke key "${k}" will never match its Rust param — use camelCase`);
        bad++;
      }
    }
  }
}
if (bad > 0) {
  console.error(`\n${bad} snake_case invoke key(s) — build blocked.`);
  process.exit(1);
}
console.log("invoke-case: all invoke args camelCase ✓");
