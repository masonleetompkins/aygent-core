// Playground — temporary home for the working Phase-0 proofs (jail probe, daemon
// self-test, agent tool-use run) so we don't lose them while the real screens
// (Chat, Agents, etc.) get built. Themed via tokens.
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill, Transcript } from "../components/ui";

type ProbeResult = { ok: boolean; resolved?: string; error?: string } | null;
const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

// M1.7 Slice 1 — vault memory read-path proof types (mirror memory.rs).
type IngestReport = {
  scanned: number; parsed: number; embedded: number;
  skipped_unchanged: number; links: number; errors: string[];
};
type RetrievedNote = {
  path: string; title: string; ntype: string;
  score: number; via: string; snippet: string;
};

export function Playground({ folder, ws, agentId }: { folder: string | null; ws: WebSocket | null; agentId: string | null }) {
  const [probePath, setProbePath] = useState("notes/hello.md");
  const [probe, setProbe] = useState<ProbeResult>(null);
  const [selftest, setSelftest] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("Create a file called hello.md with a short greeting, then read it back.");
  const [reply, setReply] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // --- Memory test panel state (Slice 1) ---
  const [vaultPath, setVaultPath] = useState("");
  const [ingest, setIngest] = useState<IngestReport | null>(null);
  const [ingestErr, setIngestErr] = useState<string | null>(null);
  const [ingesting, setIngesting] = useState(false);
  const [query, setQuery] = useState("why am I so tired lately?");
  const [hits, setHits] = useState<RetrievedNote[] | null>(null);
  const [retrieveErr, setRetrieveErr] = useState<string | null>(null);
  const [retrieving, setRetrieving] = useState(false);

  async function runIngest() {
    if (!agentId) { setIngestErr("No active agent — pick one in the rail first."); return; }
    setIngesting(true); setIngest(null); setIngestErr(null);
    try { setIngest(await invoke<IngestReport>("memory_ingest", { agentId, vaultPath })); }
    catch (e) { setIngestErr(String(e)); }
    finally { setIngesting(false); }
  }
  async function runRetrieve() {
    if (!agentId) { setRetrieveErr("No active agent — pick one in the rail first."); return; }
    setRetrieving(true); setHits(null); setRetrieveErr(null);
    try { setHits(await invoke<RetrievedNote[]>("memory_retrieve", { agentId, query, topK: 3, expandHops: 1 })); }
    catch (e) { setRetrieveErr(String(e)); }
    finally { setRetrieving(false); }
  }

  async function runProbe() { setProbe(await invoke<ProbeResult>("broker_probe", { requested: probePath })); }
  function runDaemonSelftest() {
    setSelftest("running…");
    if (!ws) { setSelftest("daemon not connected"); return; }
    const handler = (ev: MessageEvent) => {
      let m: any; try { m = JSON.parse(ev.data); } catch { return; }
      if (m.type === "selftest:result") {
        const i = m.inside?.ok ? "ADMIT" : "refuse:" + m.inside?.error;
        const o = m.outside?.ok ? "ADMIT(!!)" : "refuse:" + m.outside?.error;
        setSelftest(`inside=${i}  ·  outside=${o}`);
        ws.removeEventListener("message", handler);
      }
    };
    ws.addEventListener("message", handler);
    ws.send(JSON.stringify({ type: "selftest" }));
  }
  async function call(cmd: string) {
    setBusy(true); setReply(null);
    try { setReply(await invoke<string>(cmd, { prompt })); }
    catch (e) { setReply("✗ " + String(e)); }
    finally { setBusy(false); }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 620 }}>
      <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Playground</h2>
      <p style={{ ...hint, marginTop: -8 }}>Temporary — the Phase-0 security + agent proofs. These move into Chat/Agents as those screens land.</p>

      {!folder && <Pill tone="muted">Pick an Agent Folder in Settings first.</Pill>}

      {folder && (
        <Card title="Probe the jail">
          <p style={hint}>Type a path; the broker admits or refuses it.</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input mono value={probePath} onChange={(e) => setProbePath(e.target.value)} placeholder="notes/hello.md  or  ../../etc/passwd" />
            <Button onClick={runProbe}>Probe</Button>
          </div>
          {probe && <Pill tone={probe.ok ? "ok" : "danger"}>{probe.ok ? `✓ admitted → ${probe.resolved}` : `✗ refused → ${probe.error}`}</Pill>}
          <div style={{ borderTop: "var(--border-width) solid var(--line)", paddingTop: 12, display: "flex", flexDirection: "column", gap: 10 }}>
            <p style={hint}>Test the jail from the <b>daemon's</b> side:</p>
            <div><Button variant="secondary" onClick={runDaemonSelftest} disabled={!ws}>Test jail from daemon</Button></div>
            {selftest && <Transcript text={selftest} />}
          </div>
        </Card>
      )}

      {folder && (
        <Card title="Run the agent">
          <p style={hint}>The agent can use jailed file tools (read/write/list) inside your folder.</p>
          <Input value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          <div style={{ display: "flex", gap: 8 }}>
            <Button onClick={() => call("agent_run")} disabled={busy}>{busy ? "…" : "Run agent (tool use)"}</Button>
            <Button variant="secondary" onClick={() => call("anthropic_test")} disabled={busy}>{busy ? "…" : "Plain completion"}</Button>
          </div>
          {reply && <Transcript text={reply} error={reply.startsWith("✗")} />}
        </Card>
      )}

      {/* M1.7 Slice 1 — Vault Memory read path (ingest + graph-expansion retrieval). */}
      <Card title="Vault memory (Slice 1 — read path)">
        <p style={hint}>
          Point at a vault folder, ingest it (parse links + embed <b>in-process</b> via the
          built-in engine — AYGENT auto-downloads the tiny embedding model on first use,
          no install, no Ollama), then ask a question. Success = a query whose literal
          words match almost nothing still surfaces the right notes — some <code>semantic</code>,
          some pulled in via <code>graph:</code> expansion. Zero vault writes.
        </p>
        <p style={{ ...hint, fontSize: 12 }}>
          First ingest downloads the ~85MB embedding model into AYGENT’s own folder — that one-time
          fetch can take a moment; after that it’s instant and fully offline.
        </p>
        {!agentId && <Pill tone="muted">Pick an agent in the rail first (memory is scoped to it).</Pill>}

        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <p style={hint}>Vault folder (absolute path — e.g. the bundled <code>aygent/test-vault</code>):</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input mono value={vaultPath} onChange={(e) => setVaultPath(e.target.value)}
              placeholder="/Users/you/Documents/aygent/test-vault" />
            <Button variant="secondary" onClick={async () => {
              const p = await invoke<string | null>("pick_vault_folder");
              if (p) setVaultPath(p);
            }}>Browse…</Button>
            <Button onClick={runIngest} disabled={ingesting || !agentId || !vaultPath}>
              {ingesting ? "Ingesting…" : "Ingest"}
            </Button>
          </div>
          {ingest && (
            <Pill tone={ingest.errors.length ? "danger" : "ok"}>
              {`✓ scanned ${ingest.scanned} · parsed ${ingest.parsed} · embedded ${ingest.embedded} · skipped ${ingest.skipped_unchanged} · links ${ingest.links}`}
              {ingest.errors.length ? ` · ${ingest.errors.length} error(s)` : ""}
            </Pill>
          )}
          {ingest && ingest.errors.length > 0 && <Transcript text={ingest.errors.join("\n")} error />}
          {ingestErr && <Transcript text={"✗ " + ingestErr} error />}
        </div>

        <div style={{ borderTop: "var(--border-width) solid var(--line)", paddingTop: 12, display: "flex", flexDirection: "column", gap: 8 }}>
          <p style={hint}>Ask the memory:</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="why am I so tired lately?" />
            <Button onClick={runRetrieve} disabled={retrieving || !agentId}>
              {retrieving ? "…" : "Retrieve"}
            </Button>
          </div>
          {retrieveErr && <Transcript text={"✗ " + retrieveErr} error />}
          {hits && hits.length === 0 && <Pill tone="muted">No hits — ingest a vault first.</Pill>}
          {hits && hits.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {hits.map((h, i) => (
                <div key={i} style={{
                  border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)",
                  padding: "10px 12px", background: "var(--bg)", boxShadow: "var(--elevation)",
                }}>
                  <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                    <b style={{ fontSize: 14 }}>{h.title || h.path}</b>
                    <Pill tone={h.via === "semantic" ? "ok" : "muted"}>
                      {h.via === "semantic" ? `semantic ${h.score.toFixed(2)}` : `↳ ${h.via}`}
                    </Pill>
                    <span style={{ ...hint, fontSize: 12 }}>{h.ntype} · {h.path}</span>
                  </div>
                  <p style={{ ...hint, marginTop: 6 }}>{h.snippet}</p>
                </div>
              ))}
            </div>
          )}
        </div>
      </Card>
    </div>
  );
}
