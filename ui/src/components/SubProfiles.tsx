// SubProfiles — Claude Code + Codex subscription seats (Mason 10-08).
//
// One-click connect: the backend auto-detects CLI logins on this machine, so
// there is no label-first step. Claude (Keychain-held, no importable file)
// resolves to the `claude setup-token` paste flow; Codex imports directly.
// Manual multi-profile (Personal/Work) lives under "Add another profile".

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";

type Profile = { id: string; kind: string; label: string; has_token: boolean };
type Usage = { pct_5h: number | null; reset_at_ms: number | null; weekly_pct: number | null; error?: string };
type Detected = { file_found: boolean; has_token: boolean; cli_found: boolean };

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

function KindSection({ kind, title, blurb }: { kind: string; title: string; blurb: string }) {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [label, setLabel] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  const [usage, setUsage] = useState<Record<string, Usage>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [detected, setDetected] = useState<Detected | null>(null);
  const [pasteFor, setPasteFor] = useState<string | null>(null);
  const [pasteHint, setPasteHint] = useState<string | null>(null);

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
  async function detect() {
    try {
      const d = await invoke<any>("sub_detect");
      setDetected(d?.[kind] ?? null);
    } catch { /* detect never blocks the list */ }
  }
  useEffect(() => { void refresh(); void detect(); /* eslint-disable-next-line */ }, [kind]);

  const connected = profiles.filter((p) => p.has_token);

  async function connect() {
    setBusy("connect"); setMsg(null);
    try {
      const r = await invoke<any>("sub_connect", { kind });
      if (r.connected) {
        setPasteFor(null); setPasteHint(null);
        setMsg(r.expired
          ? `✓ ${r.label} connected — but the token looks expired. Refresh the login, then Re-import.`
          : `✓ ${r.label} connected`);
      } else {
        // Claude without an importable file: open the setup-token paste box.
        setPasteFor(r.id); setPasteHint(r.hint ?? null);
        setMsg(null);
      }
      await refresh();
    } catch (e) { setMsg("✗ " + String(e)); }
    finally { setBusy(null); }
  }
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
      setMsg(r.expired ? "Imported — token looks expired, refresh the login then import again." : `✓ ${p.label} connected`);
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
        {connected.length > 0
          ? <Pill tone="ok">{connected.length} connected ✓</Pill>
          : <Pill tone="muted">not connected</Pill>}
      </div>
      <p style={{ color: "var(--text-muted)", fontSize: 12, margin: 0 }}>{blurb}</p>
      {connected.length === 0 && (
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap", padding: "10px 12px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--accent)", background: "color-mix(in srgb, var(--accent) 8%, transparent)" }}>
          <span style={{ fontSize: 13 }}>
            {detected?.has_token
              ? `Found your ${title} login on this Mac.`
              : kind === "claude-code"
                ? `${title} keeps its login private — one click, then paste a setup token.`
                : `No ${title} login found yet.`}
          </span>
          <Button onClick={connect} disabled={busy === "connect"}>{busy === "connect" ? "…" : detected?.has_token ? "Connect" : kind === "claude-code" ? "Connect with setup-token" : "Connect"}</Button>
        </div>
      )}
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
      {(pasteFor || pasteHint) && <PasteToken profiles={profiles} forcePid={pasteFor} hint={pasteHint} open onDone={() => { setPasteFor(null); setPasteHint(null); void refresh(); }} />}
      <details style={{ fontSize: 12, color: "var(--text-muted)" }}>
        <summary style={{ cursor: "pointer" }}>Add another profile (Personal, Work…)</summary>
        <div style={{ display: "flex", gap: 8, marginTop: 6 }}>
          <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Profile label…" />
          <Button variant="secondary" onClick={create} disabled={busy === "create" || !label.trim()}>Add</Button>
        </div>
        <PasteToken profiles={profiles} onDone={refresh} />
      </details>
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "ok"}>{msg}</Pill>}
    </div>
  );
}

function PasteToken({ profiles, onDone, forcePid, hint, open }: { profiles: Profile[]; onDone: () => void; forcePid?: string | null; hint?: string | null; open?: boolean }) {
  const [pid, setPid] = useState(forcePid ?? profiles.find((p) => !p.has_token)?.id ?? "");
  const [tok, setTok] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    if (forcePid) { setPid(forcePid); return; }
    if (!pid && profiles.length > 0) setPid(profiles.find((p) => !p.has_token)?.id ?? profiles[0].id);
  }, [profiles]);
  async function save() {
    if (!pid || !tok.trim()) return;
    setMsg(null);
    try {
      await invoke("sub_profile_save_token", { id: pid, accessToken: tok.trim() });
      setTok("");
      setMsg("✓ token saved to Keychain");
      onDone();
    } catch (e) { setMsg("✗ " + String(e)); }
  }
  if (profiles.length === 0 && !forcePid) return null;
  const body = (
    <>
      <div style={{ display: "flex", gap: 8, marginTop: 6, flexWrap: "wrap" }}>
        <select value={pid} onChange={(e) => setPid(e.target.value)} style={{ padding: "8px 10px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)" }}>
          {profiles.map((p) => <option key={p.id} value={p.id}>{p.label}{p.has_token ? " (replace)" : ""}</option>)}
        </select>
        <Input type="password" value={tok} onChange={(e) => setTok(e.target.value)} placeholder="paste token…" />
        <Button variant="secondary" onClick={save} disabled={!pid || !tok.trim()}>Save</Button>
      </div>
      <p style={{ margin: "4px 0 0" }}>{hint ?? <>Claude: run <code>claude setup-token</code> in terminal, paste the result. Codex: paste the access token from <code>~/.codex/auth.json → tokens.access_token</code>.</>}</p>
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "ok"}>{msg}</Pill>}
    </>
  );
  if (open) return <div style={{ fontSize: 12, color: "var(--text-muted)" }}>{body}</div>;
  return (
    <details style={{ fontSize: 12, color: "var(--text-muted)" }}>
      <summary style={{ cursor: "pointer" }}>Paste a token manually (no CLI needed)</summary>
      {body}
    </details>
  );
}

function KeychainBanner() {
  const [state, setState] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function check() {
    setBusy(true);
    try {
      const s = await invoke<any>("keychain_status");
      setState(s?.state ?? null);
    } catch { setState(null); }
    finally { setBusy(false); }
  }
  useEffect(() => { void check(); }, []);
  if (state !== "locked") return null;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap", padding: "10px 12px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--danger)", background: "color-mix(in srgb, var(--danger) 8%, transparent)", fontSize: 13 }}>
      <span>AYGENT can't read its Keychain vault in this build — macOS treats every unsigned rebuild as a new app. Allow the prompt once and all keys return.</span>
      <Button variant="secondary" onClick={() => void check()} disabled={busy}>{busy ? "…" : "Re-authorize"}</Button>
    </div>
  );
}

export function SubProfiles() {
  return (
    <Card title="Subscriptions">
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <p style={{ color: "var(--text-muted)", fontSize: 14, margin: 0 }}>
          Use Claude / ChatGPT subscriptions instead of API keys. $0 billed — tokens
          + API-equivalent value still tracked per turn.
        </p>
        <KeychainBanner />
        <KindSection kind="claude-code" title="Claude Code" blurb="Tokens stay in Keychain — the UI only sees labels + usage %." />
        <KindSection kind="codex" title="Codex" blurb="Tokens stay in Keychain — the UI only sees labels + usage %." />
      </div>
    </Card>
  );
}
