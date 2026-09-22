// AYGENT UI primitives — Command Pro (branch ui-command-pro).
// Dense, hairline, muted accent. Everything reads CSS vars; no hardcoded hex.
import type { ReactNode, CSSProperties } from "react";

export function Card({ title, children, style }: { title?: string; children: ReactNode; style?: CSSProperties }) {
  return (
    <section
      className="bg-surface border border-line rounded-card shadow-elevation"
      style={{ padding: "var(--card-pad)", display: "flex", flexDirection: "column", gap: "var(--card-gap)", ...style }}
    >
      {title && (
        <div style={{ fontSize: "var(--text-caption)", letterSpacing: "0.06em", textTransform: "uppercase", fontWeight: 700, color: "var(--text-faint)" }}>
          {title}
        </div>
      )}
      {children}
    </section>
  );
}

export function Button({
  children, onClick, disabled, variant = "primary", style,
}: { children: ReactNode; onClick?: () => void; disabled?: boolean; variant?: "primary" | "secondary"; style?: CSSProperties }) {
  const base: CSSProperties = {
    borderRadius: "var(--radius-control)",
    padding: "6px 12px",
    fontWeight: 600,
    fontSize: 13,
    cursor: disabled ? "default" : "pointer",
    opacity: disabled ? 0.5 : 1,
    transition: "border-color 120ms ease, box-shadow 120ms ease",
    border: "var(--border-width) solid var(--line)",
  };
  const variantStyle: CSSProperties =
    variant === "primary"
      ? { background: "var(--accent)", borderColor: "var(--accent)", color: "#fff", boxShadow: "none" }
      : { background: "var(--surface)", color: "var(--text)", boxShadow: "var(--elevation)" };
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      style={{ ...base, ...variantStyle, ...style }}
      onFocus={(e) => { e.currentTarget.style.boxShadow = "0 0 0 3px var(--focus-ring)"; }}
      onBlur={(e) => { e.currentTarget.style.boxShadow = variant === "primary" ? "none" : "var(--elevation)"; }}
    >
      {children}
    </button>
  );
}

export function Input(props: React.InputHTMLAttributes<HTMLInputElement> & { mono?: boolean }) {
  const { mono, style, ...rest } = props;
  return (
    <input
      {...rest}
      style={{
        flex: 1,
        background: "var(--bg)",
        border: "var(--border-width) solid var(--line)",
        borderRadius: "var(--radius-control)",
        color: "var(--text)",
        padding: "7px 10px",
        fontSize: 13,
        fontFamily: mono ? "ui-monospace, SFMono-Regular, Menlo, monospace" : "inherit",
        boxShadow: "none",
        outline: "none",
        ...style,
      }}
      onFocus={(e) => { e.currentTarget.style.borderColor = "var(--accent)"; e.currentTarget.style.boxShadow = "0 0 0 3px var(--focus-ring)"; props.onFocus?.(e as any); }}
      onBlur={(e) => { e.currentTarget.style.borderColor = "var(--line)"; e.currentTarget.style.boxShadow = "none"; props.onBlur?.(e as any); }}
    />
  );
}

export function Pill({ children, tone = "muted" }: { children: ReactNode; tone?: "muted" | "ok" | "danger" }) {
  const color = tone === "ok" ? "var(--ok)" : tone === "danger" ? "var(--danger)" : "var(--text-muted)";
  return (
    <span style={{
      display: "inline-flex", alignItems: "center", gap: 4,
      whiteSpace: "nowrap", width: "fit-content", alignSelf: "flex-start",
      border: "var(--border-width) solid var(--line)", color,
      borderRadius: "var(--radius-pill)", padding: "2px 9px", fontSize: 11.5, fontWeight: 600,
      lineHeight: 1.4, background: "var(--bg)",
    }}>
      {children}
    </span>
  );
}

export function Transcript({ text, error }: { text: string; error?: boolean }) {
  return (
    <pre style={{
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 12, lineHeight: 1.5, margin: 0,
      background: "var(--bg)", color: error ? "var(--danger)" : "var(--text)",
      border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
      padding: "10px 12px", maxHeight: 340, overflowY: "auto",
      whiteSpace: "pre-wrap", wordBreak: "break-word",
    }}>{text}</pre>
  );
}
