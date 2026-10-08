// SubProfiles — Claude Code + Codex subscription profiles (slice 3, UNWIRED).
//
// Drop-in section for Settings → Providers. Calls the sub_* commands from
// sub_cmds.rs (register them in lib.rs first — see docs/subscription-providers.md).
// v1 auth = run `claude login` / `codex login` in terminal, then Import.
// Profiles are multi (Personal/Work), tokens in Keychain, usage polled cached 90s.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";

type Profile = { id: string; kind: string; label: string; has_token: boolean };
type Usage = { pct_5h: number | null; reset_at_ms: number | null; weekly_pct: number | null; error?: string };

function fmtReset(ms: number | null): string {
  if (!ms) return "";
  const d = new Date(ms > 1e12 ? ms : ms * 1000);
  return `resets ${d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`;
}

function UsageBar({ u }: { u: Usage | undefined }) {
  if (!u) return null;
  if (u.error) return <span style={{ fontSize: 12, color: "var(--text-faint)" }}>{u.error}</span>;
  if (u.pct_5h == null) return <span style={{ fontSize: 12, color: "var(--text-faint)" }}>no usage yet</span>;
  const pct = Math.min(100, Math.max(0, u.pct_5h));
  const hot = pct >= 90;
  return (
    <span style={{ display: "flex", alignItems: "center", gap: 8, flex: 1, minWidth: 140 }}>
      <span style={{ flex: 1, height: 6, borderRadius: 3, background: "var(--line)", overflow: "hidden" }}>
        <span style={{ display: "block", width: `${pct}%`, height: "100%", background: hot ? "var(--danger)" : "var(--accent)" }} />
      </span>
      <span style={{ fontSize: 12, fontFamily: "ui-monospace, monospace", color: hot ? "var(--danger)" : "var(--text-muted)" }}>
        {Math.round(pct)}% {fmtReset(u.reset_at_ms)}
      </span>
    </span>
  );
}

function KindSection({ kind, title, loginHint }: { kind: string; title: string; loginHint: string }) {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [label, setLabel] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  const [usage, setUsage] = useState<Record<string, Usage>>({});
  const [busy, setBusy] = useState<string | null>(null);

  async function refresh() {
    try {
      const list = await invoke<Profile[]>("sub_profiles_list", { kind });
      setProfiles(list || []);
      for (const p of list || []) {
        if (!p.has_token) continue;
        try {
          const u = await invoke<any>("sub_usage", { id: p.id });
          setUsage((m) => ({
            ...m,
            [p.id]: u.ok
              ? { pct_5h: u.pct_5h ?? null, reset_at_ms: u.reset_at_ms ?? null, weekly_pct: u.weekly_pct ?? null }
              : { pct_5h: null, reset_at_ms: null, weekly_pct: null, error: String(u.error || "usage unavailable") },
          }));
        } catch { /* per-profile usage never blocks the list */ }
      }
    } catch (e) { setMsg("✗ " + String(e)); }
  }
  useEffect(() => { void refresh(); /* eslint-disable-next-line */ }, [kind]);

  async function create() {
    if (!label.trim()) return;
    setBusy("create"); setMsg(null);
    try { await invoke("sub_profile_create", { kind, label: label.trim() }); setLabel(""); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
    finally { setBusy(null); }
  }
  async function importCli(p: Profile) {
    setBusy(p.id); setMsg(null);
    try {
      const r = await invoke<any>("sub_profile_import_cli", { id: p.id });
      setMsg(r.expired ? "Imported — token looks expired, re-run login then import again." : `✓ ${p.label} connected`);
      await refresh();
    } catch (e) { setMsg("✗ " + String(e)); }
    finally { setBusy(null); }
  }
  async function remove(p: Profile) {
    if (!window.confirm(`Remove subscription profile "${p.label}"? (Tokens forgotten.)`)) return;
    try { await invoke("sub_profile_delete", { id: p.id }); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, paddingTop: 8 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <span style={{ fontSize: 14, fontWeight: 700, width: 100 }}>{title}</span>
        {profiles.length === 0
          ? <Pill tone="muted">no profiles</Pill>
          : <Pill tone="ok">{profiles.length} profile{profiles.length === 1 ? "" : "s"}</Pill>}
      </div>
      <p style={{ color: "var(--text-muted)", fontSize: 12, margin: 0 }}>
        {loginHint} Tokens stay in Keychain — the UI only sees labels + usage %.
      </p>
      {profiles.map((p) => (
        <div key={p.id} style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          <b style={{ fontSize: 13, minWidth: 90 }}>{p.label}</b>
          {p.has_token ? <Pill tone="ok">connected ✓</Pill> : <Pill tone="muted">no token</Pill>}
          <UsageBar u={usage[p.id]} />
          <span style={{ display: "flex", gap: 6, marginLeft: "auto" }}>
            <Button variant="secondary" onClick={() => void importCli(p)} disabled={busy === p.id}>
              {busy === p.id ? "…" : p.has_token ? "Re-import" : "Import from CLI"}
            </Button>
            <Button variant="secondary" onClick={() => void remove(p)}>Remove</Button>
          </span>
        </div>
      ))}
      <div style={{ display: "flex", gap: 8 }}>
        <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="New profile label — Personal, Work…" />
        <Button onClick={create} disabled={busy === "create" || !label.trim()}>Add</Button>
      </div>
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "ok"}>{msg}</Pill>}
    </div>
  );
}

export function SubProfiles() {
  return (
    <Card title="Subscriptions">
      <p style={{ color: "var(--text-muted)", fontSize: 14, margin: 0 }}>
        Use Claude / ChatGPT subscriptions instead of API keys. $0 billed — tokens
        + API-equivalent value still tracked per turn.
      </p>
      <KindSection kind="claude-code" title="Claude Code" loginHint="Run `claude login` in terminal, then Import." />
      <KindSection kind="codex" title="Codex" loginHint="Run `codex login` in terminal, then Import." />
    </Card>
  );
}
