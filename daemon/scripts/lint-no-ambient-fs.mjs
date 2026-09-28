// Import-ban lint (Atlas): the daemon must NEVER import fs/child_process/net
// for real work — all file I/O goes through the Rust broker, only Rust may spawn.
// The Seatbelt profile enforces this at the OS; this lint catches it at authoring
// time so we fail fast in the live loop instead of at the gate.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const BANNED = [
  /\bfrom\s+["']node:fs["']/,
  /\brequire\(\s*["']node:fs["']\s*\)/,
  /\bfrom\s+["']fs["']/,
  /\bfrom\s+["']node:child_process["']/,
  /\bfrom\s+["']child_process["']/,
  /\bfrom\s+["']node:net["']/,
];
// This lint script itself is allowed to use fs (it's a dev tool, not the daemon).
const ALLOW = ["scripts/"];

let violations = 0;
function walk(dir) {
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) { walk(p); continue; }
    if (!p.endsWith(".ts")) continue;
    if (ALLOW.some((a) => p.includes(a))) continue;
    const src = readFileSync(p, "utf8");
    for (const rx of BANNED) {
      if (rx.test(src)) {
        console.error(`[no-ambient-fs] BANNED import in ${p}: ${rx}`);
        violations++;
      }
    }
  }
}
walk("src");
if (violations) {
  console.error(`\n${violations} violation(s). The daemon has no ambient fs/exec — use ctx.broker.`);
  process.exit(1);
}
console.log("[no-ambient-fs] clean — daemon uses the broker, not raw fs.");
