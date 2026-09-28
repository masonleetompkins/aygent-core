// DASHBOARD RENDERERS, PART 2 (M4) — the remaining 8 module kinds.
//
// Same discipline as Modules.tsx: every one of these reads ONLY CSS variables.
// No hex, no shadows, no font stacks. That is what makes "one design system"
// structurally true rather than a guideline a model can drift away from.
//
// Charts are hand-rolled SVG on purpose — a charting library would bring its
// own palette and typography and immediately break the token contract.
import type { ReactNode } from "react";
import { Button, Pill } from "../ui";
import { Icon } from "../Icon";
import { runAction, toneToVariant, type ActionCtx, type DashAction } from "./actions";

type Row = { spec: { props?: Record<string, unknown>; actions?: { label: string; action: DashAction; tone?: string }[] } };

// ---------------------------------------------------------------------------
// table
// ---------------------------------------------------------------------------

export function Table({ data, props }: { data: unknown; props: Record<string, unknown> }) {
  const rows: unknown[] = Array.isArray(data) ? data : Array.isArray((data as any)?.rows) ? (data as any).rows : [];
  if (rows.length === 0) return <Empty label="No rows" />;
  // Columns come from the spec, else inferred from the first row's keys.
  const cols: string[] = Array.isArray(props.columns)
    ? (props.columns as string[])
    : Object.keys((rows[0] ?? {}) as Record<string, unknown>).slice(0, 5);

  return (
    <div style={{ flex: 1, minHeight: 0, overflow: "auto" }}>
      <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
        <thead>
          <tr>
            {cols.map((c) => (
              <th key={c} style={{
                textAlign: "left", padding: "6px 8px", fontWeight: 700, fontSize: 11,
                letterSpacing: "0.08em", textTransform: "uppercase", color: "var(--text-faint)",
                borderBottom: "var(--border-width) solid var(--line)", position: "sticky", top: 0,
                background: "var(--surface)",
              }}>{c}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.slice(0, 200).map((r, i) => (
            <tr key={i}>
              {cols.map((c) => (
                <td key={c} style={{
                  padding: "6px 8px", borderBottom: "1px solid color-mix(in srgb, var(--line) 15%, transparent)",
                  whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", maxWidth: 240,
                }}>{fmtCell((r as any)?.[c])}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function fmtCell(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}

// ---------------------------------------------------------------------------
// chart — line / bar / sparkline, drawn as token-colored SVG
// ---------------------------------------------------------------------------

export function Chart({ data, props }: { data: unknown; props: Record<string, unknown> }) {
  const raw: unknown[] = Array.isArray(data) ? data : Array.isArray((data as any)?.points) ? (data as any).points : [];
  const pts: number[] = raw
    .map((p) => (typeof p === "number" ? p : Number((p as any)?.value ?? (p as any)?.y ?? NaN)))
    .filter((n) => Number.isFinite(n));
  if (pts.length === 0) return <Empty label="No data points" />;

  const variant = String(props.variant ?? "line");
  const W = 100, H = 40;
  const min = Math.min(...pts), max = Math.max(...pts);
  const span = max - min || 1;
  const x = (i: number) => (pts.length === 1 ? W / 2 : (i / (pts.length - 1)) * W);
  const y = (v: number) => H - ((v - min) / span) * (H - 4) - 2;

  return (
    <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", gap: 8, justifyContent: "center" }}>
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" style={{ width: "100%", height: "100%", minHeight: 40, display: "block" }}>
        {variant === "bar" ? (
          pts.map((v, i) => {
            const bw = W / pts.length;
            const top = y(v);
            return <rect key={i} x={i * bw + bw * 0.15} y={top} width={bw * 0.7} height={H - top}
              fill="var(--accent)" opacity={0.85} />;
          })
        ) : (
          <>
            <polyline
              points={pts.map((v, i) => `${x(i)},${y(v)}`).join(" ")}
              fill="none" stroke="var(--accent)" strokeWidth={1.5}
              strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke"
            />
            {/* a soft area fill reads as "lighting", consistent with the theme */}
            <polygon
              points={`0,${H} ${pts.map((v, i) => `${x(i)},${y(v)}`).join(" ")} ${W},${H}`}
              fill="var(--accent)" opacity={0.08}
            />
          </>
        )}
      </svg>
      <div style={{ display: "flex", justifyContent: "space-between", fontSize: 11, color: "var(--text-faint)" }}>
        <span>{fmtNum(min)}</span>
        <span>{pts.length} pts</span>
        <span>{fmtNum(max)}</span>
      </div>
    </div>
  );
}

function fmtNum(n: number): string {
  return Math.abs(n) >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(Math.round(n * 100) / 100);
}

// ---------------------------------------------------------------------------
// progress
// ---------------------------------------------------------------------------

export function Progress({ data, props }: { data: unknown; props: Record<string, unknown> }) {
  const d = (data ?? {}) as any;
  const value = Number(d.value ?? d ?? 0);
  const goal = Number(d.goal ?? props.goal ?? 100);
  const pct = goal > 0 ? Math.max(0, Math.min(100, (value / goal) * 100)) : 0;
  return (
    <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", gap: 10, justifyContent: "center" }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
        <span style={{ fontSize: 26, fontWeight: 700 }}>{fmtNum(value)}</span>
        <span style={{ fontSize: 13, color: "var(--text-muted)" }}>/ {fmtNum(goal)}{d.label ? ` · ${d.label}` : ""}</span>
      </div>
      <div style={{
        height: 10, borderRadius: "var(--radius-pill)", background: "var(--bg)",
        border: "var(--border-width) solid var(--line)", overflow: "hidden",
      }}>
        <div style={{ width: `${pct}%`, height: "100%", background: "var(--accent)", transition: "width 200ms ease" }} />
      </div>
      <span style={{ fontSize: 12, color: "var(--text-faint)" }}>{Math.round(pct)}%</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// timeline
// ---------------------------------------------------------------------------

export function Timeline({ data }: { data: unknown }) {
  const items: any[] = Array.isArray(data) ? data : Array.isArray((data as any)?.items) ? (data as any).items : [];
  if (items.length === 0) return <Empty label="Nothing yet" />;
  return (
    <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column" }}>
      {items.slice(0, 100).map((it, i) => (
        <div key={i} style={{ display: "flex", gap: 10, padding: "6px 0" }}>
          {/* the rail: a dot + a connecting line, drawn with borders not images */}
          <div style={{ display: "flex", flexDirection: "column", alignItems: "center", width: 12, flexShrink: 0 }}>
            <span style={{ width: 7, height: 7, borderRadius: "50%", background: "var(--accent)", marginTop: 5 }} />
            {i < items.length - 1 && <span style={{ flex: 1, width: 1, background: "color-mix(in srgb, var(--line) 25%, transparent)" }} />}
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 13, overflow: "hidden", textOverflow: "ellipsis" }}>
              {typeof it === "string" ? it : it?.text ?? it?.title ?? JSON.stringify(it)}
            </div>
            {(it?.meta || it?.date) && (
              <div style={{ fontSize: 11, color: "var(--text-faint)", marginTop: 1 }}>{it.meta ?? it.date}</div>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// status — health pills
// ---------------------------------------------------------------------------

export function Status({ data }: { data: unknown }) {
  const items: any[] = Array.isArray(data) ? data : Array.isArray((data as any)?.items) ? (data as any).items : [];
  if (items.length === 0) return <Empty label="Nothing to report" />;
  return (
    <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexWrap: "wrap", gap: 8, alignContent: "flex-start" }}>
      {items.map((it, i) => {
        const label = typeof it === "string" ? it : it?.label ?? it?.text ?? "—";
        const state = String(typeof it === "object" ? it?.meta ?? it?.status ?? "" : "").toLowerCase();
        const tone = /ok|connected|good|pass|healthy|active/.test(state) ? "ok"
          : /err|fail|down|bad|refus/.test(state) ? "danger" : "muted";
        return <Pill key={i} tone={tone as "ok" | "danger" | "muted"}>{label}{state ? ` · ${state}` : ""}</Pill>;
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// feed — a last-N SNAPSHOT, not a live tail (nothing on a dashboard ticks)
// ---------------------------------------------------------------------------

export function Feed({ data }: { data: unknown }) {
  const items: any[] = Array.isArray(data) ? data : Array.isArray((data as any)?.items) ? (data as any).items : [];
  if (items.length === 0) return <Empty label="No activity" />;
  return (
    <pre style={{
      flex: 1, minHeight: 0, overflow: "auto", margin: 0,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 12, lineHeight: 1.5, whiteSpace: "pre-wrap", wordBreak: "break-word",
      color: "var(--text-muted)",
    }}>
      {items.slice(0, 200).map((it) => (typeof it === "string" ? it : it?.text ?? JSON.stringify(it))).join("\n")}
    </pre>
  );
}

// ---------------------------------------------------------------------------
// actions — the "add buttons" module
// ---------------------------------------------------------------------------

export function Actions({ row, ctx }: { row: Row; ctx: ActionCtx }) {
  const acts = row.spec.actions ?? [];
  if (acts.length === 0) return <Empty label="No buttons configured" />;
  return (
    <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexWrap: "wrap", gap: 8, alignContent: "flex-start" }}>
      {acts.map((a, i) => (
        <Button key={i} variant={toneToVariant(a.tone)} onClick={() => void runAction(a.action, ctx)}>
          {a.label}
        </Button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// form — inputs that feed an action
// ---------------------------------------------------------------------------

export function Form({
  row, ctx, values, setValue,
}: {
  row: Row; ctx: ActionCtx;
  values: Record<string, string>;
  setValue: (k: string, v: string) => void;
}) {
  const fields: any[] = Array.isArray(row.spec.props?.fields) ? (row.spec.props!.fields as any[]) : [];
  const submit = row.spec.actions?.[0];
  if (fields.length === 0) return <Empty label="No fields configured" />;
  return (
    <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column", gap: 8 }}>
      {fields.map((f, i) => {
        const name = String(f?.name ?? `field${i}`);
        return (
          <label key={i} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 11, color: "var(--text-faint)", textTransform: "uppercase", letterSpacing: "0.08em" }}>
              {f?.label ?? name}
            </span>
            <input
              value={values[name] ?? ""}
              placeholder={f?.placeholder ?? ""}
              onChange={(e) => setValue(name, e.target.value)}
              style={{
                background: "var(--bg)", border: "var(--border-width) solid var(--line)",
                borderRadius: "var(--radius-control)", color: "var(--text)",
                padding: "7px 10px", fontSize: 13, width: "100%", boxSizing: "border-box",
              }}
            />
          </label>
        );
      })}
      {submit && (
        <Button
          onClick={() => {
            // Substitute {field} placeholders in a run_prompt before firing, so
            // a form can compose a real request from what the user typed.
            const a = submit.action;
            if (a.kind === "run_prompt") {
              const filled = a.prompt.replace(/\{(\w+)\}/g, (_m, k) => values[k] ?? "");
              void runAction({ ...a, prompt: filled }, ctx);
            } else {
              void runAction(a, ctx);
            }
          }}
        >
          {submit.label}
        </Button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------

export function Empty({ label = "No data yet" }: { label?: string }): ReactNode {
  return (
    <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--text-faint)", fontSize: 13 }}>
      {label}
    </div>
  );
}

/// Small inline control used on module headers (refresh / approve).
export function IconBtn({ title, icon, onClick, tone }: {
  title: string; icon: "refresh" | "close" | "bolt"; onClick: () => void; tone?: "danger";
}) {
  return (
    <button
      title={title}
      onClick={onClick}
      style={{
        background: "none", border: "none", cursor: "pointer", padding: 2, display: "flex",
        color: tone === "danger" ? "var(--danger)" : "var(--text-faint)",
      }}
    >
      <Icon name={icon} size={14} />
    </button>
  );
}
