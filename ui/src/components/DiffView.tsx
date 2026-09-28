// AYGENT — streaming side-by-side diff view for code-editing tool calls
// (Mason 09-09). Old version on the left (deletions red), new version on the
// right (additions green). Streams live: `newText` grows with the tool args
// while the model writes, `before` lands async from tool_file_before.
// No deps — tiny LCS line diff with a prefix/suffix fast path.
import { useMemo } from "react";

export type BeforeSnap = { exists: boolean; content: string; truncated: boolean; binary: boolean };

export function isDiffable(name: string, before?: BeforeSnap | null): before is BeforeSnap {
  return (name === "write_file" || name === "github_write_file") && !!before && !before.binary;
}

export function splitLines(s: string): string[] {
  if (!s) return [];
  return s.split("\n");
}

// Line diff via LCS on the middle after stripping common prefix/suffix.
// DP capped: if oldMid * newMid > 250k cells, fall back to prefix/suffix only
// (pure del-run + add-run) so a huge paste can't hang the chat.
export type Op = { t: "same" | "del" | "add"; line: string };
export function lineDiff(oldLines: string[], newLines: string[]): Op[] {
  let pre = 0;
  while (pre < oldLines.length && pre < newLines.length && oldLines[pre] === newLines[pre]) pre++;
  let suf = 0;
  while (
    suf < oldLines.length - pre &&
    suf < newLines.length - pre &&
    oldLines[oldLines.length - 1 - suf] === newLines[newLines.length - 1 - suf]
  ) suf++;
  const ops: Op[] = oldLines.slice(0, pre).map((line) => ({ t: "same" as const, line }));
  const om = oldLines.slice(pre, oldLines.length - suf);
  const nm = newLines.slice(pre, newLines.length - suf);
  if (om.length * nm.length > 250_000) {
    for (const line of om) ops.push({ t: "del", line });
    for (const line of nm) ops.push({ t: "add", line });
  } else if (om.length || nm.length) {
    // LCS DP (lengths only, then backtrack with full table — capped above).
    const n = om.length, m = nm.length;
    const dp: Uint16Array[] = [];
    // Uint16 overflows past 65535 — fall back for absurd sizes (already capped).
    for (let i = 0; i <= n; i++) dp.push(new Uint16Array(m + 1));
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        dp[i][j] = om[i] === nm[j] ? (dp[i + 1][j + 1] + 1) : Math.max(dp[i + 1][j], dp[i][j + 1]);
      }
    }
    let i = 0, j = 0;
    while (i < n && j < m) {
      if (om[i] === nm[j]) { ops.push({ t: "same", line: om[i] }); i++; j++; }
      else if (dp[i + 1][j] >= dp[i][j + 1]) { ops.push({ t: "del", line: om[i] }); i++; }
      else { ops.push({ t: "add", line: nm[j] }); j++; }
    }
    while (i < n) { ops.push({ t: "del", line: om[i++] }); }
    while (j < m) { ops.push({ t: "add", line: nm[j++] }); }
  }
  for (const line of oldLines.slice(oldLines.length - suf)) ops.push({ t: "same", line });
  return ops;
}

export function diffCounts(ops: Op[]): { add: number; del: number } {
  let add = 0, del = 0;
  for (const o of ops) { if (o.t === "add") add++; else if (o.t === "del") del++; }
  return { add, del };
}

export type Row = { left: string | null; right: string | null; t: "same" | "del" | "add" | "mod" };

// One-shot counts for a before/after pair (used for the card bar without
// rendering the full row grid).
export function countDiff(before: BeforeSnap, after: string): { add: number; del: number } {
  const oldLines = before.exists ? splitLines(before.content) : [];
  return diffCounts(lineDiff(oldLines, splitLines(after)));
}
// Zip del-runs + following add-runs into paired rows so modifications sit
// side-by-side instead of stacked.
export function toRows(ops: Op[]): Row[] {
  const rows: Row[] = [];
  let i = 0;
  while (i < ops.length) {
    if (ops[i].t === "same") { rows.push({ left: ops[i].line, right: ops[i].line, t: "same" }); i++; continue; }
    const dels: string[] = [];
    while (i < ops.length && ops[i].t === "del") { dels.push(ops[i].line); i++; }
    const adds: string[] = [];
    while (i < ops.length && ops[i].t === "add") { adds.push(ops[i].line); i++; }
    const n = Math.max(dels.length, adds.length);
    for (let k = 0; k < n; k++) {
      const l = k < dels.length ? dels[k] : null;
      const r = k < adds.length ? adds[k] : null;
      rows.push({ left: l, right: r, t: l !== null && r !== null ? "mod" : l !== null ? "del" : "add" });
    }
    // add-only or del-only runs land here too (one side null).
  }
  return rows;
}

const RED_BG = "rgba(220,38,38,0.16)";
const GREEN_BG = "rgba(34,197,94,0.16)";
const RED_TX = "#ff7b72";
const GREEN_TX = "#7ee787";

export function DiffCounts({ add, del }: { add: number; del: number }) {
  return (
    <span style={{ display: "inline-flex", gap: 8, flexShrink: 0, fontFamily: "ui-monospace, monospace", fontSize: 12, fontWeight: 700 }}>
      <span style={{ color: GREEN_TX }}>+{add}</span>
      <span style={{ color: RED_TX }}>-{del}</span>
    </span>
  );
}

export function DiffView({ before, after, running }: { before: BeforeSnap; after: string; running?: boolean }) {
  const { rows, add, del } = useMemo(() => {
    const oldLines = before.exists ? splitLines(before.content) : [];
    const newLines = splitLines(after);
    const ops = lineDiff(oldLines, newLines);
    return { rows: toRows(ops), ...diffCounts(ops) };
  }, [before.content, before.exists, after]);

  // Cap rendered rows so a 10k-line file can't bloat the DOM (counts stay exact).
  const MAX_ROWS = 400;
  const shown = rows.slice(0, MAX_ROWS);
  const clipped = rows.length - shown.length;
  let oldNo = 0, newNo = 0;

  return (
    <div style={{ display: "flex", flexDirection: "column", minWidth: 0 }}>
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", fontSize: 10.5, fontWeight: 800, letterSpacing: 0.4, color: "var(--text-faint)", padding: "0 8px 4px 8px" }}>
        <span>BEFORE{before.exists ? "" : " (new file)"}{before.truncated ? " · first 32k chars" : ""}</span>
        <span>AFTER{running ? " · streaming…" : ""}</span>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", fontFamily: "ui-monospace, monospace", fontSize: 11.5, lineHeight: 1.55, border: "var(--border-width) solid var(--line)", borderRadius: 6, overflow: "hidden" }}>
        <div style={{ minWidth: 0, borderRight: "var(--border-width) solid var(--line)" }}>
          {shown.map((r, i) => {
            const has = r.left !== null;
            if (has) oldNo++;
            const no = has ? oldNo : null;
            return (
              <div key={i} style={{ display: "flex", gap: 6, padding: "0 8px 0 4px", background: has ? RED_BG : "transparent", minHeight: "1.55em", whiteSpace: "pre-wrap", wordBreak: "break-word", overflowWrap: "anywhere" }}>
                <span style={{ width: 30, flexShrink: 0, textAlign: "right", opacity: 0.45, userSelect: "none" }}>{no ?? ""}</span>
                <span style={{ flex: 1, minWidth: 0 }}>{r.left ?? ""}</span>
              </div>
            );
          })}
        </div>
        <div style={{ minWidth: 0 }}>
          {shown.map((r, i) => {
            const has = r.right !== null;
            if (has) newNo++;
            const no = has ? newNo : null;
            return (
              <div key={i} style={{ display: "flex", gap: 6, padding: "0 8px 0 4px", background: has && r.t !== "same" ? GREEN_BG : "transparent", minHeight: "1.55em", whiteSpace: "pre-wrap", wordBreak: "break-word", overflowWrap: "anywhere" }}>
                <span style={{ width: 30, flexShrink: 0, textAlign: "right", opacity: 0.45, userSelect: "none" }}>{no ?? ""}</span>
                <span style={{ flex: 1, minWidth: 0 }}>{r.right ?? ""}</span>
              </div>
            );
          })}
        </div>
      </div>
      <div style={{ display: "flex", gap: 8, alignItems: "center", paddingTop: 4, fontSize: 11, color: "var(--text-muted)" }}>
        <DiffCounts add={add} del={del} />
        <span>{rows.length} changed lines shown{clipped > 0 ? ` (first ${shown.length} of ${rows.length})` : ""}</span>
      </div>
    </div>
  );
}
