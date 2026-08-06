// AYGENT — MCP servers section of the Connections screen (2026-08-06).
// Built-in catalog (Premiere) + add-your-own from the web. Enabling a server
// runs its managed install (narrated live), starts it, and — for servers with a
// verify tool (Premiere) — shows the manual in-app step + a Verify button.
// Disable stops it; "remove" (custom only) or "uninstall" clears what it added.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Button, Input, Pill } from "../components/ui";

type McpServer = {
  key: string; label: string; command: string; args: string[];
  enabled: boolean; builtin: boolean; needs_node: boolean;
  setup_note: string; verify_tool: string | null;
  running: boolean; tool_count: number;
};

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

export function McpConnections() {
  const [servers, setServers] = useState<McpServer[]>([]);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [progress, setProgress] = useState<string>("");
  const [verifyMsg, setVerifyMsg] = useState<Record<string, { ok: boolean; text: string }>>({});
  const [err, setErr] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const unref = useRef<null | (() => void)>(null);

  async function refresh() {
    try { setServers(await invoke<McpServer[]>("mcp_list")); } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { refresh(); return () => { unref.current?.(); }; }, []);

  async function enable(s: McpServer) {
    setErr(null); setBusyKey(s.key); setProgress("Starting…");
    const channel = `mcp-enable-${s.key}-${Date.now()}`;
    const un = await listen<{ phase: string; note: string; ok?: boolean }>(channel, (e) => {
      const p = e.payload || ({} as any);
      if (p.note) setProgress(p.note);
    });
    unref.current = un;
    try {
      const r = await invoke<{ verify_tool: string | null; tool_count: number; setup_note: string }>(
        "mcp_enable", { channel, key: s.key });
      await refresh();
      if (r.verify_tool) {
        // Manual in-app step needed (e.g. Premiere Start Bridge) — leave a note.
        setVerifyMsg((m) => ({ ...m, [s.key]: { ok: false, text: "Installed. Do the in-app step below, then Verify." } }));
      }
      window.dispatchEvent(new Event("aygent-tools-changed"));
    } catch (e) { setErr(String(e)); }
    finally { un(); unref.current = null; setBusyKey(null); setProgress(""); }
  }

  async function disable(s: McpServer, uninstall: boolean) {
    setErr(null); setBusyKey(s.key); setProgress(uninstall ? "Removing…" : "Disabling…");
    const channel = `mcp-disable-${s.key}-${Date.now()}`;
    try {
      await invoke("mcp_disable", { channel, key: s.key, uninstall });
      await refresh();
      window.dispatchEvent(new Event("aygent-tools-changed"));
    } catch (e) { setErr(String(e)); }
    finally { setBusyKey(null); setProgress(""); }
  }

  async function removeCustom(s: McpServer) {
    setErr(null);
    try { await invoke("mcp_remove_custom", { key: s.key }); await refresh(); }
    catch (e) { setErr(String(e)); }
  }

  async function verify(s: McpServer) {
    setErr(null); setBusyKey(s.key); setProgress("Verifying…");
    try {
      const text = await invoke<string>("mcp_verify", { key: s.key });
      setVerifyMsg((m) => ({ ...m, [s.key]: { ok: true, text } }));
    } catch (e) {
      setVerifyMsg((m) => ({ ...m, [s.key]: { ok: false, text: String(e) } }));
    } finally { setBusyKey(null); setProgress(""); }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "var(--space-4)" }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>MCP</h2>
        <Button variant="secondary" onClick={() => setAdding(true)}>+ Add from the web</Button>
      </div>
      <p style={{ ...hint, marginTop: -8 }}>
        Connect external tool servers (Model Context Protocol). Enable a built-in with one click —
        AYGENT installs everything it needs into its own space and tells you each step. Your agents
        then get that server’s tools.
      </p>
      {err && <Pill tone="danger">✗ {err}</Pill>}

      {servers.map((s) => {
        const busy = busyKey === s.key;
        const v = verifyMsg[s.key];
        return (
          <Card key={s.key} title={s.label}>
            <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
              {s.enabled ? <Pill tone="ok">enabled ✓</Pill> : <Pill tone="muted">off</Pill>}
              {s.running && <span style={{ ...hint, fontSize: 12 }}>{s.tool_count} tools live</span>}
              <span style={{ ...hint, fontSize: 12, fontFamily: "ui-monospace, monospace", color: "var(--text-faint)" }}>{s.command}</span>
              <span style={{ flex: 1 }} />
              {busy ? (
                <span style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
                  <span style={{ display: "inline-block", animation: "aygentSpin 1s linear infinite" }}>◐</span>{progress}
                </span>
              ) : !s.enabled ? (
                <Button onClick={() => void enable(s)}>Enable</Button>
              ) : (
                <span style={{ display: "flex", gap: 6 }}>
                  {s.verify_tool && <Button variant="secondary" onClick={() => void verify(s)}>Verify</Button>}
                  <Button variant="secondary" onClick={() => void disable(s, false)}>Disable</Button>
                  {s.builtin
                    ? <Button variant="secondary" onClick={() => void disable(s, true)}>Remove + uninstall</Button>
                    : <Button variant="secondary" onClick={() => void removeCustom(s)}>Remove</Button>}
                </span>
              )}
            </div>
            {s.setup_note && <p style={{ ...hint, fontSize: 12.5, marginTop: 10 }}>{s.setup_note}</p>}
            {/* Premiere-style manual step + verify result. */}
            {s.enabled && s.verify_tool && (
              <div style={{ marginTop: 10, border: "var(--border-width) solid var(--line)", borderRadius: 8, padding: 10, fontSize: 13 }}>
                <b>Final step (in Premiere):</b> restart Premiere Pro, open <code>Window &gt; Extensions &gt; MCP Bridge (CEP)</code>,
                and click <b>Start Bridge</b>. Then click <b>Verify</b> above.
                {v && (
                  <div style={{ marginTop: 8, color: v.ok ? "var(--ok, #16a34a)" : "var(--danger, #dc2626)" }}>
                    {v.ok ? "✓ " : "✕ "}{v.text}
                  </div>
                )}
              </div>
            )}
          </Card>
        );
      })}

      {adding && <AddMcp onClose={() => setAdding(false)} onAdded={async () => { setAdding(false); await refresh(); }} />}
      <style>{`@keyframes aygentSpin { to { transform: rotate(360deg); } }`}</style>
    </div>
  );
}

function AddMcp({ onClose, onAdded }: { onClose: () => void; onAdded: () => void }) {
  const [label, setLabel] = useState("");
  const [command, setCommand] = useState("");
  const [argsText, setArgsText] = useState("");
  const [needsNode, setNeedsNode] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  async function save() {
    if (!label.trim() || !command.trim()) { setErr("Name and command are required."); return; }
    setSaving(true); setErr(null);
    const args = argsText.split(/\s+/).filter(Boolean);
    try {
      await invoke("mcp_add_custom", { label, command, args, env: [], needsNode });
      onAdded();
    } catch (e) { setErr(String(e)); }
    finally { setSaving(false); }
  }

  return (
    <Card title="Add an MCP server">
      <p style={hint}>Point AYGENT at any MCP server. If it’s an npm package, AYGENT runs it with its own Node.</p>
      <div style={{ display: "flex", flexDirection: "column", gap: 10, marginTop: 8 }}>
        <label style={{ fontSize: 13 }}>Name<Input value={label} onChange={(e: any) => setLabel(e.target.value)} placeholder="My MCP" /></label>
        <label style={{ fontSize: 13 }}>Command<Input value={command} onChange={(e: any) => setCommand(e.target.value)} placeholder="e.g. some-mcp-server" /></label>
        <label style={{ fontSize: 13 }}>Arguments (space-separated, optional)<Input value={argsText} onChange={(e: any) => setArgsText(e.target.value)} placeholder="--flag value" /></label>
        <label style={{ fontSize: 13, display: "flex", alignItems: "center", gap: 8 }}>
          <input type="checkbox" checked={needsNode} onChange={(e) => setNeedsNode(e.target.checked)} /> It’s a Node/npm server (use AYGENT’s bundled Node)
        </label>
        {err && <Pill tone="danger">{err}</Pill>}
        <div style={{ display: "flex", gap: 8 }}>
          <Button onClick={() => void save()} disabled={saving}>{saving ? "Adding…" : "Add server"}</Button>
          <Button variant="secondary" onClick={onClose} disabled={saving}>Cancel</Button>
        </div>
      </div>
    </Card>
  );
}
