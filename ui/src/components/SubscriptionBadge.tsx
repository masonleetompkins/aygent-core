// SubscriptionBadge — Chat footer badge for subscription turns (slice 2, UNWIRED).
// Standalone: import + render explicitly in Chat once the provider ships.
// Shows 5h window usage + token value instead of $ cost when billing is subscription.

export function SubscriptionBadge({
  kind, label, pct5h, resetAtMs, tokens, apiEquivCents,
}: {
  kind: string; label: string;
  pct5h?: number | null; resetAtMs?: number | null;
  tokens?: number | null; apiEquivCents?: number | null;
}) {
  const pct = typeof pct5h === "number" ? Math.max(0, Math.min(100, pct5h)) : null;
  const hot = pct != null && pct >= 90;
  const reset = resetAtMs
    ? new Date(resetAtMs).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : null;
  const api = typeof apiEquivCents === "number" ? `$${(apiEquivCents / 100).toFixed(2)} API value` : null;
  return (
    <span
      title={`${kind} · ${label} — subscription ($0 billed)`}
      style={{
        display: "inline-flex", alignItems: "center", gap: 8, fontSize: 12,
        padding: "3px 10px", borderRadius: 999,
        border: "1px solid var(--line)", background: "var(--surface)",
        color: hot ? "var(--danger)" : "var(--text-muted)",
      }}
    >
      <span style={{ fontWeight: 700 }}>Subscription · {label}</span>
      {pct != null && <span>{pct.toFixed(0)}% 5h</span>}
      {pct != null && (
        <span style={{ width: 56, height: 4, borderRadius: 2, background: "var(--bg)", overflow: "hidden" }}>
          <span style={{
            display: "block", height: "100%", width: `${pct}%`,
            background: hot ? "var(--danger)" : "var(--accent)",
          }} />
        </span>
      )}
      {reset && <span>resets {reset}</span>}
      {typeof tokens === "number" && <span>{(tokens / 1000).toFixed(1)}k tok</span>}
      {api && <span style={{ color: "var(--text-faint)" }}>~{api}</span>}
    </span>
  );
}
