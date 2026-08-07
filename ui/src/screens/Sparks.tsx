// AYGENT — SPARKS. Interactive mini-apps the agent builds for you: a single
// self-contained HTML file per Spark, rendered LIVE here in a SANDBOXED iframe
// (sandbox="allow-scripts", NO same-origin) — so a Spark runs its own JS + any
// data the agent embedded at build time, but can never read your files, reach
// this machine, or call the agent back. You ask the agent (in Chat) to build or
// change a Spark; this tab is the library + the live stage.
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Pill } from "../components/ui";

type SparkMeta = {
  slug: string; title: string; description: string; created: number; modified: number;
};

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const faint = { ...hint, fontSize: 12, color: "var(--text-faint)" } as const;

export function Sparks({ agentId, onNavigate }: { agentId: string | null; onNavigate?: (id: string) => void }) {
  const [sparks, setSparks] = useState<SparkMeta[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [html, setHtml] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    if (!agentId) { setSparks([]); return; }
    try { setSparks(await invoke<SparkMeta[]>("sparks_list", { agentId })); }
    catch (e) { setMsg("✗ " + String(e)); }
  }, [agentId]);

  useEffect(() => { void refresh(); }, [refresh]);

  // Auto-select the newest Spark on first load / after refresh if none chosen.
  useEffect(() => {
    if (!selected && sparks.length > 0) setSelected(sparks[0].slug);
  }, [sparks, selected]);

  const open = useCallback(async (slug: string) => {
    if (!agentId) return;
    setSelected(slug); setHtml(null); setLoading(true); setMsg(null);
    try {
      const r = await invoke<{ slug: string; html: string }>("sparks_read", { agentId, slug });
      setHtml(r.html);
    } catch (e) { setMsg("✗ " + String(e)); }
    finally { setLoading(false); }
  }, [agentId]);

  useEffect(() => { if (selected) void open(selected); /* eslint-disable-next-line */ }, [selected]);

  async function del(slug: string) {
    if (!agentId) return;
    try {
      await invoke("sparks_delete", { agentId, slug });
      if (selected === slug) { setSelected(null); setHtml(null); }
      await refresh();
    } catch (e) { setMsg("✗ " + String(e)); }
  }

  const current = useMemo(() => sparks.find((s) => s.slug === selected) || null, [sparks, selected]);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16, height: "100%", minHeight: 0 }}>
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 12 }}>
        <div>
          <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Sparks</h2>
          <p style={{ ...hint, marginTop: 6 }}>
            Little interactive apps your agent builds for you — a calculator, a chart of your data,
            a tool, a game. Ask in <b>Chat</b> ("make me a Spark that…") and it appears here, live.
          </p>
        </div>
        <Button variant="secondary" onClick={() => void refresh()}>Refresh</Button>
      </div>

      {!agentId && <Pill tone="muted">Pick an agent to see its Sparks.</Pill>}
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "muted"}>{msg}</Pill>}

      {agentId && sparks.length === 0 && (
        <Card title="No Sparks yet">
          <p style={hint}>
            Sparks are interactive mini-apps — the agent writes a self-contained little web app and
            embeds your data right in it, then it runs safely here in a sandbox. Try asking your
            agent, in Chat:
          </p>
          <ul style={{ ...faint, lineHeight: 1.7, margin: 0, paddingLeft: 18 }}>
            <li>"Make me a Spark that charts my last 6 months of Stripe revenue."</li>
            <li>"Build a Spark: a tip calculator with a slider."</li>
            <li>"Turn this CSV into a sortable table Spark."</li>
          </ul>
          {onNavigate && (
            <div style={{ marginTop: 4 }}>
              <Button onClick={() => onNavigate("chat")}>Go to Chat</Button>
            </div>
          )}
        </Card>
      )}

      {agentId && sparks.length > 0 && (
        <div style={{ display: "flex", gap: 14, flex: 1, minHeight: 0 }}>
          {/* Library rail */}
          <div style={{ width: 240, flexShrink: 0, display: "flex", flexDirection: "column", gap: 8, overflowY: "auto" }} className="aygent-scroll">
            {sparks.map((s) => (
              <button
                key={s.slug}
                onClick={() => setSelected(s.slug)}
                style={{
                  textAlign: "left", padding: "10px 12px", cursor: "pointer",
                  borderRadius: "var(--radius-control)",
                  border: `var(--border-width) solid ${selected === s.slug ? "var(--accent, var(--text))" : "var(--line)"}`,
                  background: selected === s.slug ? "var(--bg)" : "transparent",
                  boxShadow: selected === s.slug ? "var(--elevation)" : "none",
                  color: "var(--text)",
                }}
              >
                <div style={{ fontWeight: 700, fontSize: 14 }}>{s.title || s.slug}</div>
                {s.description && <div style={{ ...faint, marginTop: 2 }}>{s.description}</div>}
              </button>
            ))}
          </div>

          {/* Live stage */}
          <div style={{
            flex: 1, minWidth: 0, display: "flex", flexDirection: "column",
            border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
            boxShadow: "var(--elevation)", overflow: "hidden", background: "var(--surface)",
          }}>
            <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "8px 12px", borderBottom: "var(--border-width) solid var(--line)" }}>
              <span style={{ fontWeight: 700, fontSize: 14 }}>{current?.title || selected}</span>
              <code style={{ ...faint, fontSize: 11 }}>Sparks/{selected}/index.html</code>
              <span style={{ flex: 1 }} />
              <span style={faint}>ask Chat to edit</span>
              {selected && <Button variant="secondary" onClick={() => void del(selected)}>Delete</Button>}
            </div>
            <div style={{ flex: 1, minHeight: 0, background: "#ffffff" }}>
              {loading && <div style={{ ...hint, padding: 16 }}>Loading…</div>}
              {!loading && html != null && (
                // SANDBOXED: allow-scripts only (NO allow-same-origin) — the Spark
                // runs its own JS + embedded data, but is fully isolated from the
                // app, the file system, and the agent. It can't call anything back.
                <iframe
                  title={selected || "spark"}
                  srcDoc={html}
                  sandbox="allow-scripts allow-popups allow-forms"
                  style={{ width: "100%", height: "100%", border: "none", background: "#fff" }}
                />
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
