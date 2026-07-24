// Playground — temporary home for the working Phase-0 proofs (jail probe, daemon
// self-test, agent tool-use run) so we don't lose them while the real screens
// (Chat, Agents, etc.) get built. Themed via tokens.
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill, Transcript } from "../components/ui";

type ProbeResult = { ok: boolean; resolved?: string; error?: string } | null;
const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

export function Playground({ folder, ws }: { folder: string | null; ws: WebSocket | null }) {
  const [probePath, setProbePath] = useState("notes/hello.md");
  const [probe, setProbe] = useState<ProbeResult>(null);
  const [selftest, setSelftest] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("Create a file called hello.md with a short greeting, then read it back.");
  const [reply, setReply] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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
    </div>
  );
}
