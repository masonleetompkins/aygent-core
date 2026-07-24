import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Pill } from "./components/ui";
import { Sidebar, type ScreenId } from "./components/Sidebar";
import { Settings } from "./screens/Settings";
import { Playground } from "./screens/Playground";
import { initTheme, saveTheme, type Mode } from "./lib/theme";

// Phase 1: app shell (sidebar nav + content pane) on the design system.
// Screens: Settings (the wedge) + Playground (temp Phase-0 proofs) live now;
// Chat/Agents/Scheduler/Connections/Checkpoints land next.

type Status =
  | { kind: "booting" }
  | { kind: "no-daemon" }
  | { kind: "connecting"; port: number }
  | { kind: "connected"; port: number; latency?: number }
  | { kind: "error"; msg: string };

export function App() {
  const [status, setStatus] = useState<Status>({ kind: "booting" });
  const [ws, setWs] = useState<WebSocket | null>(null);
  const [folder, setFolder] = useState<string | null>(null);
  const [screen, setScreen] = useState<ScreenId>("settings");
  const [mode, setMode] = useState<Mode>("light");
  const [accent, setAccent] = useState("");

  useEffect(() => { const t = initTheme(); setMode(t.mode); setAccent(t.accent); }, []);
  function onTheme(m: Mode, a: string) { setMode(m); setAccent(a); saveTheme(m, a); }

  useEffect(() => {
    let socket: WebSocket | null = null;
    let cancelled = false;
    async function connect() {
      let info: { port: number | null; token: string };
      try { info = await invoke("daemon_info"); }
      catch (e) { setStatus({ kind: "error", msg: `daemon_info failed: ${String(e)}` }); return; }
      if (cancelled) return;
      if (!info.port) { setStatus({ kind: "no-daemon" }); setTimeout(connect, 600); return; }
      setStatus({ kind: "connecting", port: info.port });
      socket = new WebSocket(`ws://127.0.0.1:${info.port}`);
      let sentAt = 0;
      socket.onopen = () => socket!.send(JSON.stringify({ type: "auth", token: info.token }));
      socket.onmessage = (ev) => {
        let msg: any; try { msg = JSON.parse(ev.data); } catch { return; }
        if (msg.type === "auth:ok") { sentAt = Date.now(); setWs(socket); socket!.send(JSON.stringify({ type: "ping" })); }
        else if (msg.type === "pong") setStatus({ kind: "connected", port: info.port!, latency: Date.now() - sentAt });
      };
      socket.onerror = () => setStatus({ kind: "error", msg: "ws error" });
      socket.onclose = () => setStatus((s) => (s.kind === "connected" ? s : { kind: "error", msg: "ws closed (auth rejected?)" }));
    }
    connect();
    return () => { cancelled = true; socket?.close(); };
  }, []);

  async function pickFolder() {
    const chosen = await invoke<string | null>("pick_agent_folder");
    if (chosen) setFolder(chosen);
  }

  const good = status.kind === "connected";

  return (
    <div style={{ display: "flex", minHeight: "100vh" }}>
      <Sidebar active={screen} onSelect={setScreen} />
      <div style={{ flex: 1, height: "100vh", overflowY: "auto" }}>
        {/* top status strip */}
        <div style={{
          display: "flex", justifyContent: "flex-end", alignItems: "center",
          padding: "14px 28px", borderBottom: "var(--border-width) solid var(--line)",
        }}>
          <Pill tone={good ? "ok" : "muted"}>{good ? "● " : "○ "}{statusLine(status)}</Pill>
        </div>

        <div style={{ padding: "28px 32px" }}>
          {screen === "settings" && (
            <Settings mode={mode} accent={accent} onTheme={onTheme} folder={folder} onPickFolder={pickFolder} />
          )}
          {screen === "playground" && <Playground folder={folder} ws={ws} />}
        </div>
      </div>
    </div>
  );
}

function statusLine(s: Status): string {
  switch (s.kind) {
    case "booting": return "booting…";
    case "no-daemon": return "waiting for daemon…";
    case "connecting": return `connecting :${s.port}…`;
    case "connected": return `daemon connected ✓  (:${s.port}, ${s.latency ?? "?"}ms)`;
    case "error": return `not connected — ${s.msg}`;
  }
}
