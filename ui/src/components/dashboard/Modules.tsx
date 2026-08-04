// DASHBOARD MODULE RENDERERS — the fixed vocabulary.
//
// This file is WHY "one design system" is structurally true rather than a
// guideline: the agent cannot author a component, only a spec that selects one
// of these. Every renderer reads CSS variables only (DESIGN.md change rule) —
// no hex, no shadows, no fonts. A module physically cannot drift off-theme.
//
// M1 renders stat / list / markdown. The other 8 kinds validate today and land
// in M4, so a spec written now stays valid then.
import type { CSSProperties, ReactNode } from "react";
import { Markdown } from "../Markdown";
import { Pill } from "../ui";

export type ModuleKind =
  | "stat" | "list" | "table" | "markdown" | "chart" | "progress"
  | "timeline" | "actions" | "form" | "status" | "feed";

export interface ModuleSpec {
  id: string;
  kind: ModuleKind;
  title: string;
  layout: { x: number; y: number; w: number; h: number };
  source: { kind: string; [k: string]: unknown };
  props?: Record<string, unknown>;
  actions?: { label: string; action: Record<string, unknown>; tone?: string }[];
}

export interface ModuleRow {
  id: string;
  spec: ModuleSpec;
  cached: unknown;
  fetched_at: number | null;
  error: string | null;
  is_paid: boolean;
  pending_approval: boolean;
}

/// The value a renderer should draw: cached data if we have it, else whatever
/// the Static source baked in. Pull-only means "no cache yet" is a NORMAL state,
/// not an error — a fresh Http module simply has nothing until you hit Refresh.
export function moduleData(row: ModuleRow): unknown {
  if (row.cached !== null && row.cached !== undefined) return row.cached;
  const src = row.spec.source as { kind: string; data?: unknown };
  if (src?.kind === "static") return src.data;
  return null;
}

// ---------------------------------------------------------------------------
// stat
// ---------------------------------------------------------------------------

function Stat({ row }: { row: ModuleRow }) {
  const d = moduleData(row) as any;
  const props = (row.spec.props ?? {}) as any;
  // Accept a bare number/string OR {value, delta, label} — the model reaches for
  // both, and rejecting the simple shape would be pedantry.
  const value = d && typeof d === "object" ? d.value : d;
  const delta = d && typeof d === "object" ? d.delta : undefined;
  const label = (d && typeof d === "object" ? d.label : undefined) ?? props.label;
  const unit = props.unit ?? "";
  const deltaNum = typeof delta === "number" ? delta : parseFloat(String(delta ?? ""));
  const hasDelta = Number.isFinite(deltaNum);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6, justifyContent: "center", flex: 1, minHeight: 0 }}>
      <div style={{ fontSize: 34, fontWeight: 700, lineHeight: 1.1, letterSpacing: "-0.02em" }}>
        {value === null || value === undefined || value === "" ? <Empty /> : `${value}${unit}`}
      </div>
      {(label || hasDelta) && (
        <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
          {hasDelta && (
            // Direction is carried by the arrow glyph as well as the color, so
            // it still reads for a colorblind user.
            <span style={{ fontSize: 13, fontWeight: 600, color: deltaNum >= 0 ? "var(--ok)" : "var(--danger)" }}>
              {deltaNum >= 0 ? "▲" : "▼"} {Math.abs(deltaNum)}
              {props.deltaUnit ?? "%"}
            </span>
          )}
          {label && <span style={{ fontSize: 13, color: "var(--text-muted)" }}>{label}</span>}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

function List({ row }: { row: ModuleRow }) {
  const d = moduleData(row);
  // Tolerate {items:[…]} as well as a bare array.
  const items: any[] = Array.isArray(d) ? d : Array.isArray((d as any)?.items) ? (d as any).items : [];
  if (items.length === 0) return <Empty label="No items" />;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 2, overflowY: "auto", flex: 1, minHeight: 0 }}>
      {items.map((it, i) => {
        const text = typeof it === "string" ? it : it?.text ?? it?.label ?? it?.title ?? JSON.stringify(it);
        const meta = typeof it === "object" ? it?.meta ?? it?.subtitle : undefined;
        return (
          <div
            key={i}
            style={{
              display: "flex", alignItems: "baseline", gap: 10, padding: "7px 2px",
              borderBottom: i < items.length - 1 ? "1px solid var(--line)" : "none",
              // The divider is structural, not decorative — keep it faint so the
              // card doesn't read as a table.
              borderBottomColor: "color-mix(in srgb, var(--line) 18%, transparent)",
            }}
          >
            <span style={{ fontSize: 14, flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {text}
            </span>
            {meta && <span style={{ fontSize: 12, color: "var(--text-faint)", flexShrink: 0 }}>{meta}</span>}
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// markdown
// ---------------------------------------------------------------------------

function Md({ row }: { row: ModuleRow }) {
  const d = moduleData(row);
  const text = typeof d === "string" ? d : (d as any)?.text ?? (row.spec.props as any)?.text ?? "";
  if (!text) return <Empty />;
  return (
    <div style={{ flex: 1, minHeight: 0, overflowY: "auto", fontSize: 14, lineHeight: 1.55 }}>
      <Markdown text={String(text)} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

function Empty({ label = "No data yet" }: { label?: string }) {
  return (
    <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--text-faint)", fontSize: 13 }}>
      {label}
    </div>
  );
}

/// Kinds that validate but don't render until M4. Honest placeholder beats a
/// blank card that looks broken.
function Stub({ kind }: { kind: ModuleKind }) {
  return (
    <div style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: 8, color: "var(--text-faint)" }}>
      <span style={{ fontSize: 13 }}>`{kind}` renders in M4</span>
      <span style={{ fontSize: 12 }}>the spec is saved and valid</span>
    </div>
  );
}

/// "2h ago" — dashboards are pull-only, so data CAN be old and the UI has to be
/// honest about it. Never show a bare number with no age.
export function relTime(ms: number | null): string {
  if (!ms) return "never";
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

const RENDERERS: Partial<Record<ModuleKind, (p: { row: ModuleRow }) => ReactNode>> = {
  stat: Stat,
  list: List,
  markdown: Md,
};

export function ModuleBody({ row }: { row: ModuleRow }) {
  const R = RENDERERS[row.spec.kind];
  if (!R) return <Stub kind={row.spec.kind} />;
  return <>{R({ row })}</>;
}

/// The status line under a module's title. Encodes the cost rule visually: a
/// paid module is always labeled as such BEFORE you click anything.
export function ModuleMeta({ row }: { row: ModuleRow }) {
  const bits: ReactNode[] = [];
  if (row.pending_approval) bits.push(<Pill key="pa" tone="danger">needs approval</Pill>);
  if (row.is_paid) bits.push(<Pill key="paid" tone="muted">model call</Pill>);
  const src = row.spec.source?.kind;
  const showAge = src && src !== "static";
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
      {bits}
      {showAge && (
        <span style={{ fontSize: 11, color: "var(--text-faint)" }}>{relTime(row.fetched_at)}</span>
      )}
      {row.error && (
        <span style={{ fontSize: 11, color: "var(--danger)" }} title={row.error}>
          refresh failed
        </span>
      )}
    </div>
  );
}

export const cardStyle: CSSProperties = {
  background: "var(--surface)",
  border: "var(--border-width) solid var(--line)",
  borderRadius: "var(--radius-card)",
  boxShadow: "var(--elevation)",
  padding: 18,
  display: "flex",
  flexDirection: "column",
  gap: 10,
  minHeight: 0,
  overflow: "hidden",
};
