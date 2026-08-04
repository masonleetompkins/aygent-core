// Connections — the LIBRARY of services an agent can plug into.
//
// This screen is now CATALOG-DRIVEN: it renders whatever connectors_catalog
// returns, so adding a provider in descriptors.rs makes a card appear here with
// no UI work. Nothing about a specific provider is hardcoded below.
//
// THE THREE IDEAS A USER MUST GET, in this order:
//   1. A connection is an ACCOUNT ("who am I to this service?"). You can have
//      several per service — a personal GitHub and a work GitHub.
//   2. Each AGENT uses at most ONE account per service. That's why the account
//      list has radio buttons, not checkboxes: Cleo pushes with your personal
//      token, the work agent pushes with the company one, and neither can
//      silently use the other's.
//   3. READ is the default. WRITE is a separate, deliberate switch with the
//      blast radius written out.
//
// Credentials never come back to the browser after paste — the Rust side
// validates, stores in the OS keychain, and returns only non-secret metadata.
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";

type AuthField = {
  key: string; label: string; help: string; secret: boolean; placeholder: string;
};
type ConnectorTool = { name: string; description: string; access: "Read" | "Write" };
type Connector = {
  id: string; label: string; category: string; blurb: string;
  auth_kind: string; auth_fields: AuthField[];
  credential_url: string; docs_url: string; setup_steps: string[];
  write_warning: string; tools: ConnectorTool[];
};
type Connection = {
  id: number; provider: string; label: string; nickname?: string | null;
  account: string | null; scopes: string | null; status: string;
};
type AgentState = {
  enabled_ids: number[];
  providers: { provider: string; access_mode: string }[];
};

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const faint = { ...hint, fontSize: 12, color: "var(--text-faint)" } as const;

export function Connections({ agentId }: { agentId: string | null }) {
  const [catalog, setCatalog] = useState<Connector[]>([]);
  const [conns, setConns] = useState<Connection[]>([]);
  const [state, setState] = useState<AgentState>({ enabled_ids: [], providers: [] });
  const [err, setErr] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [connectingTo, setConnectingTo] = useState<Connector | null>(null);
  const [query, setQuery] = useState("");

  async function refresh() {
    try {
      const [cat, list] = await Promise.all([
        invoke<Connector[]>("connectors_catalog"),
        invoke<Connection[]>("connections_list"),
      ]);
      setCatalog(cat);
      setConns(list);
      if (agentId) setState(await invoke<AgentState>("connection_agent_state", { agentId }));
      else setState({ enabled_ids: [], providers: [] });
    } catch (e) { setErr(String(e)); }
  }
  useEffect(() => { refresh(); /* eslint-disable-next-line */ }, [agentId]);

  function accountsFor(providerId: string) {
    return conns.filter((c) => c.provider === providerId);
  }
  function modeFor(providerId: string) {
    return state.providers.find((p) => p.provider === providerId)?.access_mode ?? "read";
  }

  // Enabling an account for this agent REPLACES any other account for the same
  // service. The swap happens in ONE transaction in Rust (set_agent_enabled
  // disables siblings first), so the UI doesn't loop over disables — doing it
  // here would leave a window where two accounts are enabled, or none.
  async function useAccount(c: Connection) {
    if (!agentId) { setErr("Pick an agent in the rail first."); return; }
    setErr(null); setOk(null);
    const switching = accountsFor(c.provider).some(
      (o) => o.id !== c.id && state.enabled_ids.includes(o.id));
    try {
      await invoke("connection_set_agent_enabled", { agentId, connectionId: c.id, enabled: true });
      const name = c.nickname || c.account || c.label;
      setOk(switching ? `Switched to ${name} for this agent.` : `Using ${name}.`);
      await refresh();
    } catch (e) { setErr(String(e)); }
  }
  async function stopUsing(c: Connection) {
    if (!agentId) return;
    try {
      await invoke("connection_set_agent_enabled", { agentId, connectionId: c.id, enabled: false });
      await refresh();
    } catch (e) { setErr(String(e)); }
  }
  async function setWrite(c: Connection, write: boolean) {
    if (!agentId) return;
    setErr(null);
    try {
      await invoke("connection_set_write", { agentId, connectionId: c.id, write });
      await refresh();
    } catch (e) { setErr(String(e)); }
  }
  async function disconnect(c: Connection) {
    setErr(null);
    try { await invoke("connection_disconnect", { id: c.id }); await refresh(); }
    catch (e) { setErr(String(e)); }
  }

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const shown = q
      ? catalog.filter((c) =>
          c.label.toLowerCase().includes(q) ||
          c.blurb.toLowerCase().includes(q) ||
          c.category.toLowerCase().includes(q))
      : catalog;
    const by: Record<string, Connector[]> = {};
    for (const c of shown) (by[c.category] ||= []).push(c);
    return Object.entries(by);
  }, [catalog, query]);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "var(--space-4)", maxWidth: 780 }}>
      <h2 style={{ fontSize: "var(--text-h1)", fontWeight: "var(--weight-heading)", margin: 0 }}>Connections</h2>
      <p style={{ ...hint, marginTop: -8 }}>
        Accounts your agents can use. Credentials live in your macOS keychain — never in files, never
        sent anywhere by us. Each agent uses <b>one account per service</b>, so a work agent and a
        personal agent never share credentials.
      </p>

      <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search services…" />

      {err && <Pill tone="danger">{err}</Pill>}
      {ok && <Pill tone="ok">{ok}</Pill>}
      {!agentId && <Pill tone="muted">Pick an agent in the rail to choose which accounts it uses.</Pill>}

      {groups.map(([category, items]) => (
        <div key={category} style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <span style={{ fontSize: 12, fontWeight: 800, letterSpacing: "0.06em",
                         textTransform: "uppercase", color: "var(--text-faint)" }}>{category}</span>
          {items.map((def) => {
            const accounts = accountsFor(def.id);
            const writeMode = modeFor(def.id) === "write";
            const activeAcct = accounts.find((a) => state.enabled_ids.includes(a.id));
            const hasWriteTools = def.tools.some((t) => t.access === "Write");
            const setupOnly = def.tools.length === 0;
            return (
              <Card key={def.id} title={def.label}>
                <p style={hint}>{def.blurb}</p>

                {setupOnly && (
                  <Pill tone="muted">
                    Setup only for now — the tools for this service land in the next release.
                  </Pill>
                )}

                {/* Existing accounts: radio, because one per agent is the rule. */}
                {accounts.length > 0 && (
                  <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 6 }}>
                    {accounts.map((a) => {
                      const active = state.enabled_ids.includes(a.id);
                      return (
                        <div key={a.id} style={{
                          display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap",
                          padding: "8px 10px", borderRadius: "var(--radius-control)",
                          border: `var(--border-width) solid ${active ? "var(--accent, var(--text))" : "var(--line)"}`,
                        }}>
                          <input
                            type="radio"
                            name={`acct-${def.id}`}
                            checked={active}
                            disabled={!agentId}
                            onChange={() => useAccount(a)}
                          />
                          <span style={{ fontWeight: 700, fontSize: 14 }}>{a.nickname || a.account || a.label}</span>
                          {a.account && a.nickname !== a.account && <span style={faint}>{a.account}</span>}
                          {a.status !== "connected" && <Pill tone="danger">{a.status}</Pill>}
                          <span style={{ flex: 1 }} />
                          {active && <Pill tone="ok">this agent</Pill>}
                          <Button variant="secondary" onClick={() => disconnect(a)}>Remove</Button>
                        </div>
                      );
                    })}
                  </div>
                )}

                {/* WRITE GATE. Deliberately separate, off by default, and it
                    states the blast radius instead of a vague "allow writes". */}
                {activeAcct && hasWriteTools && (
                  <div style={{
                    marginTop: 10, padding: 10, borderRadius: "var(--radius-control)",
                    border: `var(--border-width) solid ${writeMode ? "var(--danger, #b4442f)" : "var(--line)"}`,
                  }}>
                    <label style={{ display: "flex", gap: 8, alignItems: "flex-start", cursor: "pointer" }}>
                      <input type="checkbox" checked={writeMode}
                             onChange={(e) => setWrite(activeAcct, e.target.checked)} />
                      <span style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                        <span style={{ fontWeight: 700, fontSize: 14 }}>Allow write actions</span>
                        <span style={faint}>{def.write_warning}</span>
                      </span>
                    </label>
                  </div>
                )}

                {/* What the agent actually gains. Read tools always, write tools
                    only listed when write is on — so the list is the truth. */}
                {activeAcct && !setupOnly && (
                  <details style={{ marginTop: 8 }}>
                    <summary style={{ ...faint, cursor: "pointer" }}>
                      {def.tools.filter((t) => writeMode || t.access === "Read").length} tools available to this agent
                    </summary>
                    <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 6 }}>
                      {def.tools.filter((t) => writeMode || t.access === "Read").map((t) => (
                        <div key={t.name} style={{ display: "flex", gap: 8, alignItems: "baseline" }}>
                          <code style={{ fontSize: 12 }}>{t.name}</code>
                          {t.access === "Write" && <Pill tone="danger">write</Pill>}
                        </div>
                      ))}
                    </div>
                  </details>
                )}

                <div style={{ display: "flex", gap: 8, marginTop: 10, flexWrap: "wrap" }}>
                  <Button variant={accounts.length ? "secondary" : "primary"}
                          onClick={() => setConnectingTo(def)}>
                    {accounts.length ? "+ Add another account" : `Connect ${def.label}`}
                  </Button>
                  {activeAcct && (
                    <Button variant="secondary" onClick={() => stopUsing(activeAcct)}>
                      Stop using for this agent
                    </Button>
                  )}
                </div>
              </Card>
            );
          })}
        </div>
      ))}

      {connectingTo && (
        <ConnectSheet
          def={connectingTo}
          agentId={agentId}
          onClose={() => setConnectingTo(null)}
          onDone={async (msg) => { setConnectingTo(null); setOk(msg); await refresh(); }}
        />
      )}
    </div>
  );
}

// The connect flow: numbered steps, a deep link straight to the page that mints
// the credential, then the fields. Generated from the descriptor — every future
// connector inherits this without new UI code.
function ConnectSheet({ def, agentId, onClose, onDone }: {
  def: Connector; agentId: string | null;
  onClose: () => void; onDone: (msg: string) => void;
}) {
  const [values, setValues] = useState<Record<string, string>>({});
  const [nickname, setNickname] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const ready = def.auth_fields.every((f) => (values[f.key] ?? "").trim().length > 0);

  async function connect() {
    setBusy(true); setErr(null);
    try {
      const r = await invoke<{ id: number; account: string }>("connector_connect", {
        provider: def.id, nickname, values,
      });
      // Convenience: the account you just added becomes the one this agent uses.
      if (agentId) {
        await invoke("connection_set_agent_enabled", { agentId, connectionId: r.id, enabled: true });
      }
      onDone(`Connected ${def.label} as ${r.account}.`);
    } catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  }

  return (
    <Card title={`Connect ${def.label}`}>
      <ol style={{ ...hint, paddingLeft: 20, display: "flex", flexDirection: "column", gap: 6 }}>
        {def.setup_steps.map((s, i) => <li key={i}>{s}</li>)}
      </ol>

      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", margin: "10px 0" }}>
        <Button variant="secondary" onClick={() => window.open(def.credential_url, "_blank")}>
          Open the {def.label} credential page ↗
        </Button>
        <Button variant="secondary" onClick={() => window.open(def.docs_url, "_blank")}>Docs ↗</Button>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        {def.auth_fields.map((f) => (
          <label key={f.key} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>{f.label}</span>
            <Input
              mono
              type={f.secret ? "password" : "text"}
              value={values[f.key] ?? ""}
              placeholder={f.placeholder}
              onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
            />
            <span style={faint}>{f.help}</span>
          </label>
        ))}

        {/* Nickname is what makes two accounts for one service tellable apart.
            Optional — we fall back to the account name the API reports. */}
        <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <span style={{ fontSize: 13, fontWeight: 600 }}>Nickname (optional)</span>
          <Input value={nickname} onChange={(e) => setNickname(e.target.value)}
                 placeholder="Personal · Work · Client project" />
          <span style={faint}>
            Helps you tell accounts apart when you connect more than one {def.label} account.
          </span>
        </label>

        {err && <Pill tone="danger">{err}</Pill>}

        <div style={{ display: "flex", gap: 8 }}>
          <Button onClick={connect} disabled={busy || !ready}>
            {busy ? "Validating…" : "Connect"}
          </Button>
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
        </div>
        <span style={faint}>
          We check the credential against {def.label} before saving, so a typo fails here — not later
          in the middle of a task.
        </span>
      </div>
    </Card>
  );
}
