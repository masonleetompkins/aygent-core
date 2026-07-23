import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// M0.1 + M0.2(d): connect to the daemon over the authed WS, AND let the user
// pick their Agent Folder + probe the jail live (type a path -> admit/refuse).

type Status =
  | { kind: "booting" }
  | { kind: "no-daemon" }
  | { kind: "connecting"; port: number }
  | { kind: "connected"; port: number; latency?: number }
  | { kind: "error"; msg: string };

type ProbeResult = { ok: boolean; resolved?: string; error?: string } | null;

export function App() {
  const [status, setStatus] = useState<Status>({ kind: "booting" });
  const [folder, setFolder] = useState<string | null>(null);
  const [probePath, setProbePath] = useState("notes/hello.md");
  const [probe, setProbe] = useState<ProbeResult>(null);
  const [wsRef, setWsRef] = useState<WebSocket | null>(null);
  const [selftest, setSelftest] = useState<string | null>(null);
  // M0.3 Providers
  const [apiKey, setApiKey] = useState("");
  const [keySet, setKeySet] = useState(false);
  const [prompt, setPrompt] = useState("Say hello in one short sentence.");
  const [reply, setReply] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let ws: WebSocket | null = null;
    let cancelled = false;
    async function connect() {
      let info: { port: number | null; token: string };
      try { info = await invoke("daemon_info"); }
      catch (e) { setStatus({ kind: "error", msg: `daemon_info failed: ${String(e)}` }); return; }
      if (cancelled) return;
      if (!info.port) { setStatus({ kind: "no-daemon" }); setTimeout(connect, 600); return; }
      setStatus({ kind: "connecting", port: info.port });
      ws = new WebSocket(`ws://127.0.0.1:${info.port}`);
      let sentAt = 0;
      ws.onopen = () => ws!.send(JSON.stringify({ type: "auth", token: info.token }));
      ws.onmessage = (ev) => {
        let msg: any; try { msg = JSON.parse(ev.data); } catch { return; }
        if (msg.type === "auth:ok") { sentAt = Date.now(); setWsRef(ws); ws!.send(JSON.stringify({ type: "ping" })); }
        else if (msg.type === "pong") setStatus({ kind: "connected", port: info.port!, latency: Date.now() - sentAt });
        else if (msg.type === "selftest:result") {
          const i = msg.inside?.ok ? "ADMIT" : "refuse:" + msg.inside?.error;
          const o = msg.outside?.ok ? "ADMIT(!!)" : "refuse:" + msg.outside?.error;
          setSelftest(`inside=${i}  ·  outside=${o}`);
        }
      };
      ws.onerror = () => setStatus({ kind: "error", msg: "ws error" });
      ws.onclose = () => setStatus((s) => (s.kind === "connected" ? s : { kind: "error", msg: "ws closed (auth rejected?)" }));
    }
    connect();
    return () => { cancelled = true; ws?.close(); };
  }, []);

  async function pickFolder() {
    const chosen = await invoke<string | null>("pick_agent_folder");
    if (chosen) { setFolder(chosen); setProbe(null); }
  }
  async function runProbe() {
    const r = await invoke<ProbeResult>("broker_probe", { requested: probePath });
    setProbe(r);
  }
  function runDaemonSelftest() {
    setSelftest("running…");
    wsRef?.send(JSON.stringify({ type: "selftest" }));
  }
  // Check on mount whether an anthropic key is already stored.
  useEffect(() => { invoke<boolean>("has_provider_key", { provider: "anthropic" }).then(setKeySet).catch(() => {}); }, []);
  async function saveKey() {
    if (!apiKey.trim()) return;
    await invoke("set_provider_key", { provider: "anthropic", key: apiKey.trim() });
    setApiKey(""); // never keep the secret in JS state
    setKeySet(true);
  }
  async function runAgent() {
    setBusy(true); setReply(null);
    try {
      const r = await invoke<string>("agent_run", { prompt });
      setReply(r);
    } catch (e) {
      setReply("✗ " + String(e));
    } finally { setBusy(false); }
  }
  async function testAnthropic() {
    setBusy(true); setReply(null);
    try {
      const r = await invoke<string>("anthropic_test", { prompt });
      setReply(r);
    } catch (e) {
      setReply("✗ " + String(e));
    } finally { setBusy(false); }
  }

  const good = status.kind === "connected";

  return (
    <main style={S.main}>
      <h1 style={S.h1}>AYGENT</h1>
      <p style={S.tag}>The AI agent you actually own.</p>

      <code style={{ ...S.pill, color: good ? "#2dd4bf" : "#8fa9b6", borderColor: good ? "#14b8a6" : "#1b2a35" }}>
        {good ? "● " : "○ "}{statusLine(status)}
      </code>

      <section style={S.card}>
        <div style={S.cardTitle}>Agent Folder</div>
        {folder ? (
          <code style={S.folder}>🔒 {folder}</code>
        ) : (
          <p style={S.hint}>No folder chosen — the agent can touch nothing yet.</p>
        )}
        <button style={S.btn} onClick={pickFolder}>{folder ? "Change folder…" : "Choose folder…"}</button>
      </section>

      {folder && (
        <section style={S.card}>
          <div style={S.cardTitle}>Probe the jail</div>
          <p style={S.hint}>Type a path and see if the broker admits or refuses it.</p>
          <div style={{ display: "flex", gap: "0.5rem" }}>
            <input style={S.input} value={probePath} onChange={(e) => setProbePath(e.target.value)}
              placeholder="notes/hello.md  or  ../../etc/passwd" />
            <button style={S.btn} onClick={runProbe}>Probe</button>
          </div>
          {probe && (
            <code style={{ ...S.result, color: probe.ok ? "#2dd4bf" : "#ef6f6f", borderColor: probe.ok ? "#14b8a6" : "#5a2b2b" }}>
              {probe.ok ? `✓ admitted → ${probe.resolved}` : `✗ refused → ${probe.error}`}
            </code>
          )}
          <div style={{ marginTop: "0.6rem", borderTop: "1px solid #1b2a35", paddingTop: "0.8rem" }}>
            <p style={S.hint}>Or test the jail from the <b>daemon's</b> side (the jailed brain asking the broker):</p>
            <button style={S.btn} onClick={runDaemonSelftest} disabled={!wsRef}>Test jail from daemon</button>
            {selftest && <code style={{ ...S.result, marginTop: "0.5rem", color: "#8fa9b6", borderColor: "#1b2a35" }}>{selftest}</code>}
          </div>
        </section>
      )}

      <section style={S.card}>
        <div style={S.cardTitle}>Provider — Anthropic</div>
        {keySet ? (
          <p style={S.hint}>API key set ✓ <span style={{ color: "#5a6b76" }}>(stored in macOS Keychain — never seen by the UI)</span></p>
        ) : (
          <p style={S.hint}>Paste your Anthropic API key. It goes straight to the macOS Keychain; the UI never keeps it.</p>
        )}
        <div style={{ display: "flex", gap: "0.5rem" }}>
          <input style={S.input} type="password" value={apiKey} onChange={(e) => setApiKey(e.target.value)}
            placeholder={keySet ? "replace key…" : "sk-ant-…"} />
          <button style={S.btn} onClick={saveKey}>{keySet ? "Replace" : "Save key"}</button>
        </div>
        {keySet && (
          <div style={{ marginTop: "0.6rem", borderTop: "1px solid #1b2a35", paddingTop: "0.8rem" }}>
            <p style={S.hint}>Send a prompt to the model (key fetched Rust-side, never enters JS):</p>
            <div style={{ display: "flex", gap: "0.5rem" }}>
              <input style={S.input} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
              <button style={S.btn} onClick={testAnthropic} disabled={busy}>{busy ? "…" : "Send"}</button>
            </div>
            <p style={{ ...S.hint, marginTop: "0.6rem" }}>Or run the <b>agent</b> — it can use jailed file tools (read/write/list) inside your folder:</p>
            <button style={S.btn} onClick={runAgent} disabled={busy || !folder}>{busy ? "…" : "Run agent (tool use)"}</button>
            {!folder && <p style={{ ...S.hint, color: "#5a6b76" }}>Pick an Agent Folder above first.</p>}
            {reply && (
              <code style={{ ...S.result, marginTop: "0.5rem", color: reply.startsWith("✗") ? "#ef6f6f" : "#2dd4bf", borderColor: reply.startsWith("✗") ? "#5a2b2b" : "#14b8a6", whiteSpace: "pre-wrap" }}>{reply}</code>
            )}
          </div>
        )}
      </section>
    </main>
  );
}

function statusLine(s: Status): string {
  switch (s.kind) {
    case "booting": return "booting…";
    case "no-daemon": return "waiting for daemon…";
    case "connecting": return `connecting to daemon :${s.port}…`;
    case "connected": return `daemon connected ✓  (:${s.port}, ${s.latency ?? "?"}ms)`;
    case "error": return `not connected — ${s.msg}`;
  }
}

const S: Record<string, React.CSSProperties> = {
  main: { fontFamily: "-apple-system, system-ui, sans-serif", background: "#0a0f14", color: "#e8f4f8", minHeight: "100vh", display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", gap: "0.6rem", padding: "2rem" },
  h1: { letterSpacing: "0.15em", color: "#2dd4bf", margin: 0 },
  tag: { color: "#8fa9b6", margin: 0 },
  pill: { marginTop: "0.5rem", fontSize: "0.85rem", border: "1px solid #1b2a35", borderRadius: "999px", padding: "0.35rem 0.9rem" },
  card: { marginTop: "1.2rem", width: "min(560px, 90vw)", background: "#111b24", border: "1px solid #1b2a35", borderRadius: "14px", padding: "1.2rem 1.3rem", display: "flex", flexDirection: "column", gap: "0.7rem" },
  cardTitle: { color: "#2dd4bf", fontSize: "0.8rem", letterSpacing: "0.12em", textTransform: "uppercase", fontWeight: 700 },
  hint: { color: "#8fa9b6", fontSize: "0.85rem", margin: 0 },
  folder: { color: "#e8f4f8", fontSize: "0.8rem", wordBreak: "break-all" },
  btn: { alignSelf: "flex-start", background: "#14b8a6", color: "#04121a", border: "none", borderRadius: "8px", padding: "0.5rem 1rem", fontWeight: 700, cursor: "pointer" },
  input: { flex: 1, background: "#0a0f14", border: "1px solid #1b2a35", borderRadius: "8px", color: "#e8f4f8", padding: "0.5rem 0.7rem", fontFamily: "monospace", fontSize: "0.85rem" },
  result: { fontSize: "0.8rem", border: "1px solid", borderRadius: "8px", padding: "0.5rem 0.7rem", wordBreak: "break-all" },
};
