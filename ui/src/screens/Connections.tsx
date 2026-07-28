// Connections — one-click integrations that plug an agent into real accounts.
// Slice 1: GitHub via a fine-grained/classic Personal Access Token (paste).
// A Connection = a keychain-backed bearer credential + non-secret metadata,
// exposed to an agent as Rust-side token-attached tools. The token never
// touches the browser after paste; the Rust side validates + stores it.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";

type Connection = {
  id: number;
  provider: string;
  kind: string;
  auth_kind: string;
  label: string;
  account: string | null;
  scopes: string | null;
  status: string;
};

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

// Deep link that opens GitHub's fine-grained PAT creation page, pre-named.
const GH_TOKEN_URL = "https://github.com/settings/personal-access-tokens/new";

export function Connections({ agentId }: { agentId: string | null }) {
  const [list, setList] = useState<Connection[]>([]);
  const [enabledIds, setEnabledIds] = useState<number[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [token, setToken] = useState("");
  const [showConnect, setShowConnect] = useState(false);

  async function refresh() {
    try {
      setList(await invoke<Connection[]>("connections_list"));
      if (agentId) setEnabledIds(await invoke<number[]>("connection_enabled_for_agent", { agentId }));
    } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { refresh(); }, [agentId]);

  const github = list.find((c) => c.provider === "github");

  async function connectGithub() {
    setConnecting(true); setErr(null);
    try {
      const r = await invoke<{ id: number; login: string }>("github_connect", { token: token.trim() });
      setToken(""); setShowConnect(false);
      // Auto-enable the new connection for the active agent (convenience).
      if (agentId) await invoke("connection_set_agent_enabled", { agentId, connectionId: r.id, enabled: true });
      setErr(`Connected as @${r.login}.`);
      refresh();
    } catch (e) { setErr(String(e)); } finally { setConnecting(false); }
  }
  async function disconnect(id: number) {
    setErr(null);
    try { await invoke("connection_disconnect", { id }); refresh(); }
    catch (e) { setErr(String(e)); }
  }
  async function toggleForAgent(c: Connection, on: boolean) {
    if (!agentId) { setErr("Pick an agent in the rail to enable a connection for it."); return; }
    try { await invoke("connection_set_agent_enabled", { agentId, connectionId: c.id, enabled: on }); refresh(); }
    catch (e) { setErr(String(e)); }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 680 }}>
      <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Connections</h2>
      <p style={{ ...hint, marginTop: -8 }}>
        Plug an agent into your real accounts. Tokens live in your OS keychain — never in files, never
        sent to the internet by us. The agent only gets what a connected tool returns.
      </p>
      {err && <Pill tone={err.startsWith("Connected") ? "ok" : "danger"}>{err}</Pill>}
      {!agentId && <Pill tone="muted">Pick an agent in the rail to enable connections for it.</Pill>}

      {/* GitHub catalog card */}
      <Card title="GitHub">
        {github ? (
          <>
            <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
              <Pill tone={github.status === "connected" ? "ok" : "danger"}>
                {github.status === "connected" ? `● Connected ${github.account ?? ""}` : `○ ${github.status}`}
              </Pill>
              {github.scopes ? <span style={{ ...hint, fontSize: 12 }}>scopes: {github.scopes}</span> : null}
              <Button variant="secondary" onClick={() => disconnect(github.id)}>Disconnect</Button>
            </div>
            <label style={{ display: "flex", gap: 8, alignItems: "center", ...hint, marginTop: 8 }}>
              <input
                type="checkbox"
                checked={enabledIds.includes(github.id)}
                disabled={!agentId}
                onChange={(e) => toggleForAgent(github, e.target.checked)}
              />
              Enable GitHub for the active agent (adds <code>github_list_prs</code> to its tools)
            </label>
          </>
        ) : showConnect ? (
          <>
            <p style={hint}>
              Create a token, then paste it below. It grants only what you scope it to — read-only is enough
              for now. Recommended: a fine-grained token with <b>read access to your repositories &amp; pull requests</b>.
            </p>
            <div>
              <Button variant="secondary" onClick={() => window.open(GH_TOKEN_URL, "_blank")}>
                Open GitHub token page ↗
              </Button>
            </div>
            <Input
              mono
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="paste your GitHub token (github_pat_… or ghp_…)"
            />
            <div style={{ display: "flex", gap: 8 }}>
              <Button onClick={connectGithub} disabled={connecting || !token.trim()}>
                {connecting ? "Validating…" : "Connect"}
              </Button>
              <Button variant="secondary" onClick={() => { setShowConnect(false); setToken(""); }}>Cancel</Button>
            </div>
          </>
        ) : (
          <>
            <p style={hint}>Let an agent read your open pull requests. Connect with a Personal Access Token (~30s).</p>
            <div><Button onClick={() => setShowConnect(true)}>Connect GitHub</Button></div>
          </>
        )}
      </Card>

      {/* Google Calendar — coming with OAuth verification */}
      <Card title="Google Calendar">
        <p style={hint}>
          Read your calendar for scheduled briefings. Coming soon — pending Google’s OAuth app verification
          (a launch requirement for the Calendar scope).
        </p>
        <div><Button variant="secondary" onClick={() => {}} disabled>Connect (soon)</Button></div>
      </Card>
    </div>
  );
}
