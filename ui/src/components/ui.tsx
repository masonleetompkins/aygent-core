// AYGENT UI primitives — themed entirely via tokens (DESIGN.md). No hardcoded
// hex or shadows here; everything references the CSS variables through Tailwind
// token classes, so light/dark/accent flow automatically.
import type { ReactNode, CSSProperties } from "react";

export function Card({ title, children, style }: { title?: string; children: ReactNode; style?: CSSProperties }) {
  return (
    <section
      className="bg-surface border border-line rounded-card shadow-elevation"
      style={{ padding: "var(--card-pad)", display: "flex", flexDirection: "column", gap: "var(--card-gap)", ...style }}
    >
      {title && (
        <div style={{ fontSize: "var(--text-caption)", letterSpacing: "0.12em", textTransform: "uppercase", fontWeight: 700, color: "var(--text-muted)" }}>
          {title}
        </div>
      )}
      {children}
    </section>
  );
}

export function Button({
  children, onClick, disabled, variant = "primary",
}: { children: ReactNode; onClick?: () => void; disabled?: boolean; variant?: "primary" | "secondary" }) {
  const base: CSSProperties = {
    borderRadius: "var(--radius-control)",
    padding: "9px 16px",
    fontWeight: 600,
    fontSize: 14,
    cursor: disabled ? "default" : "pointer",
    opacity: disabled ? 0.5 : 1,
    transition: "box-shadow 150ms ease, transform 150ms ease",
    border: "var(--border-width) solid var(--line)",
  };
  const variantStyle: CSSProperties =
    variant === "primary"
      ? { background: "var(--accent)", color: "var(--bg)", boxShadow: "var(--elevation)" }
      : { background: "var(--surface)", color: "var(--text)", boxShadow: "var(--elevation)" };
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      style={{ ...base, ...variantStyle }}
      onMouseEnter={(e) => { if (!disabled) e.currentTarget.style.boxShadow = "var(--elevation-hover)"; }}
      onMouseLeave={(e) => { e.currentTarget.style.boxShadow = "var(--elevation)"; }}
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
        padding: "9px 12px",
        fontSize: 14,
        fontFamily: mono ? "var(--font-mono, ui-monospace, monospace)" : "inherit",
        boxShadow: "var(--elevation)",
        ...style,
      }}
    />
  );
}

export function Pill({ children, tone = "muted" }: { children: ReactNode; tone?: "muted" | "ok" | "danger" }) {
  const color = tone === "ok" ? "var(--ok)" : tone === "danger" ? "var(--danger)" : "var(--text-muted)";
  return (
    <span style={{
      border: `var(--border-width) solid ${color}`, color,
      borderRadius: "var(--radius-pill)", padding: "5px 12px", fontSize: 13, fontWeight: 600,
      boxShadow: "var(--elevation)", background: "var(--surface)",
    }}>
      {children}
    </span>
  );
}

export function Transcript({ text, error }: { text: string; error?: boolean }) {
  return (
    <pre style={{
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 13, lineHeight: 1.5, margin: 0,
      background: "var(--bg)", color: error ? "var(--danger)" : "var(--text)",
      border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
      padding: "14px 16px", maxHeight: 340, overflowY: "auto",
      whiteSpace: "pre-wrap", wordBreak: "break-word", boxShadow: "var(--elevation)",
    }}>{text}</pre>
  );
}
