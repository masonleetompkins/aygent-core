import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Pill } from "./components/ui";
import { Sidebar, type ScreenId } from "./components/Sidebar";
import { AgentRail } from "./components/AgentRail";
import { type AgentProfile } from "./components/AgentSwitcher";
import { Agents } from "./screens/Agents";
import { Settings } from "./screens/Settings";
import { Playground } from "./screens/Playground";
import { Chat } from "./screens/Chat";
import { Checkpoints } from "./screens/Checkpoints";
import { Tools } from "./screens/Tools";
import { Scheduler } from "./screens/Scheduler";
import { Connections } from "./screens/Connections";
import { initTheme, saveTheme, type Mode } from "./lib/theme";
import { startHeadlessWatcher } from "./lib/turns";

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
  const [activeAgent, setActiveAgent] = useState<AgentProfile | null>(null);
  const [screen, setScreen] = useState<ScreenId>("chat");
  const [mode, setMode] = useState<Mode>("light");
  const [accent, setAccent] = useState("");
  const [keySet, setKeySet] = useState(false);

  useEffect(() => { const t = initTheme(); setMode(t.mode); setAccent(t.accent); }, []);
  // Start the standing watcher for inter-agent (headless) turns so their live
  // stream is captured into the per-agent store even though the UI didn't start
  // them — this is what makes you WATCH agents talk to each other.
  useEffect(() => { void startHeadlessWatcher(); }, []);
  // Restore the saved Agent Folder on boot (folder persistence) so the user
  // never has to re-pick after a restart. The backend re-registers the broker
  // scope; if the folder vanished it returns null and we prompt a fresh pick.
  useEffect(() => {
    invoke<string | null>("restore_agent_folder").then((f) => {
      if (f) setFolder(f);
      // After the (possible) back-compat migration runs inside restore, load the
      // active agent so the switcher + context are correct on boot.
      invoke<AgentProfile | null>("agents_get_active").then((a) => {
        if (a) { setActiveAgent(a); if (a.folder_path) setFolder(a.folder_path); }
      }).catch(() => {});
    }).catch(() => {});
  }, []);

  // When the active agent changes (switcher), its folder becomes the scope.
  function onActiveChange(a: AgentProfile) {
    setActiveAgent(a);
    if (a.folder_path) setFolder(a.folder_path);
  }
  useEffect(() => { invoke<boolean>("has_provider_key", { provider: "anthropic" }).then(setKeySet).catch(() => {}); }, [screen]);
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

  // M1.4 parallel UI: viewing an agent no longer changes which agents RUN. We
  // still call agents_set_active (so the primary Chat pane's folder/model track
  // the viewed agent), but every agent runs in the background regardless.
  function onViewAgent(a: AgentProfile) {
    invoke<AgentProfile | null>("agents_set_active", { id: a.id })
      .then((updated) => { if (updated) onActiveChange(updated); else onActiveChange(a); })
      .catch(() => onActiveChange(a));
    if (screen !== "chat") setScreen("chat");
  }

  return (
    <div style={{ display: "flex", minHeight: "100vh" }}>
      <Sidebar active={screen} onSelect={setScreen} />
      <AgentRail
        viewingId={activeAgent?.id ?? null}
        onView={onViewAgent}
        onManage={() => setScreen("agents")}
      />
      <div style={{ flex: 1, height: "100vh", overflowY: "auto" }}>
        {/* top status strip */}
        <div style={{
          display: "flex", justifyContent: "flex-end", alignItems: "center",
          padding: "14px 28px", borderBottom: "var(--border-width) solid var(--line)",
        }}>
          <Pill tone={good ? "ok" : "muted"}>{good ? "● " : "○ "}{statusLine(status)}</Pill>
        </div>

        <div style={{ padding: "28px 32px" }}>
          {screen === "chat" && <Chat folder={folder} keySet={keySet} agentId={activeAgent?.id ?? null} />}
          {screen === "agents" && (
            <Agents
              activeId={activeAgent?.id ?? null}
              onActiveChange={onActiveChange}
              onPickFolder={pickFolder}
              pendingFolder={folder}
            />
          )}
          {screen === "settings" && (
            <Settings mode={mode} accent={accent} onTheme={onTheme} folder={folder} onPickFolder={pickFolder} agentId={activeAgent?.id ?? null} />
          )}
          {screen === "checkpoints" && <Checkpoints folder={folder} />}
          {screen === "scheduler" && <Scheduler agentId={activeAgent?.id ?? null} />}
          {screen === "connections" && <Connections agentId={activeAgent?.id ?? null} />}
          {screen === "tools" && <Tools folder={folder} />}
          {screen === "playground" && <Playground folder={folder} ws={ws} agentId={activeAgent?.id ?? null} />}
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
