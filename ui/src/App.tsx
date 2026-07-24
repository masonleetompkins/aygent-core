import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill, Transcript } from "./components/ui";
import { initTheme, saveTheme, type Mode } from "./lib/theme";

// Phase 1: rebuilt into the AYGENT design system (DESIGN.md). Every color,
// shadow, and outline comes from tokens — light/dark/accent flow automatically.

type Status =
  | { kind: "booting" }
  | { kind: "no-daemon" }
  | { kind: "connecting"; port: number }
  | { kind: "connected"; port: number; latency?: number }
  | { kind: "error"; msg: string };

type ProbeResult = { ok: boolean; resolved?: string; error?: string } | null;

const ACCENT_SWATCHES = ["", "#2dd4bf", "#6366f1", "#e0533d", "#22c55e", "#eab308", "#ec4899"];

export function App() {
  const [status, setStatus] = useState<Status>({ kind: "booting" });
  const [folder, setFolder] = useState<string | null>(null);
  const [probePath, setProbePath] = useState("notes/hello.md");
  const [probe, setProbe] = useState<ProbeResult>(null);
  const [wsRef, setWsRef] = useState<WebSocket | null>(null);
  const [selftest, setSelftest] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [keySet, setKeySet] = useState(false);
  const [prompt, setPrompt] = useState("Say hello in one short sentence.");
  const [reply, setReply] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // theme state
  const [mode, setMode] = useState<Mode>("light");
  const [accent, setAccent] = useState("");

  useEffect(() => {
    const t = initTheme();
    setMode(t.mode); setAccent(t.accent);
  }, []);
  function setTheme(nextMode: Mode, nextAccent: string) {
    setMode(nextMode); setAccent(nextAccent); saveTheme(nextMode, nextAccent);
  }

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

  useEffect(() => { invoke<boolean>("has_provider_key", { provider: "anthropic" }).then(setKeySet).catch(() => {}); }, []);

  async function pickFolder() {
    const chosen = await invoke<string | null>("pick_agent_folder");
    if (chosen) { setFolder(chosen); setProbe(null); }
  }
  async function runProbe() { setProbe(await invoke<ProbeResult>("broker_probe", { requested: probePath })); }
  function runDaemonSelftest() { setSelftest("running…"); wsRef?.send(JSON.stringify({ type: "selftest" })); }
  async function saveKey() {
    if (!apiKey.trim()) return;
    await invoke("set_provider_key", { provider: "anthropic", key: apiKey.trim() });
    setApiKey(""); setKeySet(true);
  }
  async function call(cmd: string) {
    setBusy(true); setReply(null);
    try { setReply(await invoke<string>(cmd, { prompt })); }
    catch (e) { setReply("✗ " + String(e)); }
    finally { setBusy(false); }
  }

  const good = status.kind === "connected";
  const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;

  return (
    <main style={{ minHeight: "100vh", display: "flex", flexDirection: "column", alignItems: "center", padding: "40px 20px", gap: 16 }}>
      {/* Header */}
      <div style={{ textAlign: "center", marginBottom: 4 }}>
        <h1 style={{ fontSize: 30, fontWeight: 800, letterSpacing: "0.14em", margin: 0 }}>AYGENT</h1>
        <p style={{ ...hint, marginTop: 6 }}>The AI agent you actually own.</p>
      </div>

      {/* Theme bar */}
      <div style={{ display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap", justifyContent: "center" }}>
        <Button variant="secondary" onClick={() => setTheme(mode === "light" ? "dark" : "light", accent)}>
          {mode === "light" ? "◐ Light" : "◑ Dark"}
        </Button>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          {ACCENT_SWATCHES.map((c) => (
            <button key={c || "default"} onClick={() => setTheme(mode, c)} title={c || "default (black/white)"}
              style={{
                width: 24, height: 24, borderRadius: 999, cursor: "pointer",
                border: `2px solid ${accent === c ? "var(--text)" : "var(--line)"}`,
                background: c || (mode === "light" ? "#0a0a0a" : "#ffffff"),
                boxShadow: accent === c ? "var(--elevation)" : "none",
              }} />
          ))}
        </div>
      </div>

      <div style={{ width: "min(560px, 92vw)", display: "flex", flexDirection: "column", gap: 16 }}>
        <div style={{ display: "flex", justifyContent: "center" }}>
          <Pill tone={good ? "ok" : "muted"}>{good ? "● " : "○ "}{statusLine(status)}</Pill>
        </div>

        <Card title="Agent Folder">
          {folder ? <code style={{ fontSize: 13, wordBreak: "break-all", fontFamily: "ui-monospace, monospace" }}>🔒 {folder}</code>
                  : <p style={hint}>No folder chosen — the agent can touch nothing yet.</p>}
          <div><Button onClick={pickFolder}>{folder ? "Change folder…" : "Choose folder…"}</Button></div>
        </Card>

        {folder && (
          <Card title="Probe the jail">
            <p style={hint}>Type a path and see if the broker admits or refuses it.</p>
            <div style={{ display: "flex", gap: 8 }}>
              <Input mono value={probePath} onChange={(e) => setProbePath(e.target.value)} placeholder="notes/hello.md  or  ../../etc/passwd" />
              <Button onClick={runProbe}>Probe</Button>
            </div>
            {probe && <Pill tone={probe.ok ? "ok" : "danger"}>{probe.ok ? `✓ admitted → ${probe.resolved}` : `✗ refused → ${probe.error}`}</Pill>}
            <div style={{ borderTop: "var(--border-width) solid var(--line)", paddingTop: 12, display: "flex", flexDirection: "column", gap: 10 }}>
              <p style={hint}>Or test the jail from the <b>daemon's</b> side (the jailed brain asking the broker):</p>
              <div><Button variant="secondary" onClick={runDaemonSelftest} disabled={!wsRef}>Test jail from daemon</Button></div>
              {selftest && <Transcript text={selftest} />}
            </div>
          </Card>
        )}

        <Card title="Provider — Anthropic">
          <p style={hint}>{keySet
            ? "API key set ✓ (stored in macOS Keychain — never seen by the UI)"
            : "Paste your Anthropic API key. It goes straight to the macOS Keychain; the UI never keeps it."}</p>
          <div style={{ display: "flex", gap: 8 }}>
            <Input type="password" mono value={apiKey} onChange={(e) => setApiKey(e.target.value)} placeholder={keySet ? "replace key…" : "sk-ant-…"} />
            <Button onClick={saveKey}>{keySet ? "Replace" : "Save key"}</Button>
          </div>
          {keySet && (
            <div style={{ borderTop: "var(--border-width) solid var(--line)", paddingTop: 12, display: "flex", flexDirection: "column", gap: 10 }}>
              <p style={hint}>Send a prompt to the model (key fetched Rust-side, never enters JS):</p>
              <div style={{ display: "flex", gap: 8 }}>
                <Input value={prompt} onChange={(e) => setPrompt(e.target.value)} />
                <Button onClick={() => call("anthropic_test")} disabled={busy}>{busy ? "…" : "Send"}</Button>
              </div>
              <p style={hint}>Or run the <b>agent</b> — it can use jailed file tools (read/write/list) inside your folder:</p>
              <div><Button onClick={() => call("agent_run")} disabled={busy || !folder}>{busy ? "…" : "Run agent (tool use)"}</Button></div>
              {!folder && <p style={{ ...hint, color: "var(--text-faint)" }}>Pick an Agent Folder above first.</p>}
              {reply && <Transcript text={reply} error={reply.startsWith("✗")} />}
            </div>
          )}
        </Card>
      </div>
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
