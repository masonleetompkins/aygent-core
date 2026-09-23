// Geometry self-check for the dashboard grid.
//
// The UI has no test runner and adding one at 2am is a bigger decision than
// this change deserves — but drag math that's wrong by one cell is exactly the
// kind of bug that survives a demo and ruins a week. So: compile the PURE
// module with the esbuild that Vite already ships, and assert against it.
//
//   node scripts/check-grid.mjs
import { build } from "esbuild";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const out = join(mkdtempSync(join(tmpdir(), "grid-")), "grid.mjs");
await build({
  entryPoints: ["src/components/dashboard/gridDrag.ts"],
  outfile: out, format: "esm", bundle: true, logLevel: "silent",
});
const g = await import(pathToFileURL(out).href);

let failed = 0;
function check(name, cond, detail = "") {
  if (cond) { console.log(`  ok   ${name}`); }
  else { console.log(`  FAIL ${name}${detail ? ` — ${detail}` : ""}`); failed++; }
}
const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);

console.log("\ngrid geometry");

// --- clamping -------------------------------------------------------------
check("a card can't be dragged off the right edge",
  eq(g.clamp({ x: 11, y: 0, w: 4, h: 3 }), { x: 8, y: 0, w: 4, h: 3 }));
check("negative coords snap back to the origin",
  eq(g.clamp({ x: -5, y: -3, w: 4, h: 3 }), { x: 0, y: 0, w: 4, h: 3 }));
check("width is capped at the grid width",
  g.clamp({ x: 0, y: 0, w: 99, h: 3 }).w === g.GRID_COLS);
check("a card can't be shrunk below the minimum",
  eq(g.clamp({ x: 0, y: 0, w: 0, h: 0 }), { x: 0, y: 0, w: g.MIN_W, h: g.MIN_H }));

// Resize pins the LEFT edge — the card must not slide left to make room.
check("resizing past the right edge truncates width, keeps x",
  eq(g.clampResize({ x: 9, y: 0, w: 8, h: 3 }), { x: 9, y: 0, w: 3, h: 3 }));

// --- pixel → cell ---------------------------------------------------------
{
  // 12 cols, 11 gaps of 16 in a 1000px grid.
  const W = 1000, cw = g.colWidth(W);
  check("one column of travel moves exactly one column",
    g.pxToCells(cw + g.GAP, 0, W).cx === 1, `got ${g.pxToCells(cw + g.GAP, 0, W).cx}`);
  check("a tiny nudge doesn't move anything",
    g.pxToCells(4, 4, W).cx === 0 && g.pxToCells(4, 4, W).cy === 0);
  check("one row of travel moves exactly one row",
    g.pxToCells(0, g.ROW_H + g.GAP, W).cy === 1);
}

// --- overlap resolution ---------------------------------------------------
{
  // B sits under A. Drag A onto B: B must be pushed below A, not vanish.
  const items = [
    { id: "a", x: 0, y: 0, w: 6, h: 3 },
    { id: "b", x: 0, y: 3, w: 6, h: 3 },
  ];
  const moved = items.map((i) => (i.id === "a" ? { ...i, y: 3 } : { ...i }));
  const r = g.resolve(moved, "a");
  const a = r.find((i) => i.id === "a"), b = r.find((i) => i.id === "b");
  check("a displaced card moves below the dragged one", b.y >= a.y + a.h,
    `a.y=${a.y} b.y=${b.y}`);
  check("no two cards overlap after resolve", !g.overlaps(a, b));
}
{
  // Cascade: dropping onto A must push B, which pushes C.
  const items = [
    { id: "a", x: 0, y: 0, w: 12, h: 2 },
    { id: "b", x: 0, y: 2, w: 12, h: 2 },
    { id: "c", x: 0, y: 4, w: 12, h: 2 },
  ];
  const r = g.compact(g.resolve(items.map((i) => (i.id === "c" ? { ...i, y: 0 } : { ...i })), "c"));
  const pairs = [["a","b"],["a","c"],["b","c"]];
  check("a cascading push settles with no overlaps",
    pairs.every(([x, y]) => !g.overlaps(r.find(i=>i.id===x), r.find(i=>i.id===y))));
}

// --- compaction -----------------------------------------------------------
{
  // A hole at the top must close, or the dashboard drifts down forever.
  const r = g.compact([{ id: "a", x: 0, y: 7, w: 4, h: 3 }]);
  check("a lone card floats to the top", r[0].y === 0, `y=${r[0].y}`);
}
{
  // Side-by-side cards must NOT stack — they don't overlap horizontally.
  const r = g.compact([
    { id: "a", x: 0, y: 0, w: 6, h: 3 },
    { id: "b", x: 6, y: 0, w: 6, h: 3 },
  ]);
  check("side-by-side cards both stay on row 0",
    r.every((i) => i.y === 0));
}

// --- diffing --------------------------------------------------------------
{
  const before = [{ id: "a", x: 0, y: 0, w: 4, h: 3 }, { id: "b", x: 4, y: 0, w: 4, h: 3 }];
  const after = [{ id: "a", x: 0, y: 0, w: 4, h: 3 }, { id: "b", x: 5, y: 0, w: 4, h: 3 }];
  const d = g.changed(before, after);
  check("only the moved card is written", d.length === 1 && d[0].id === "b");
  check("a no-op drag writes nothing", g.changed(before, before).length === 0);
}

console.log(failed === 0 ? "\nall grid checks passed\n" : `\n${failed} CHECK(S) FAILED\n`);
process.exit(failed === 0 ? 0 : 1);
