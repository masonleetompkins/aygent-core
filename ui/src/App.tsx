import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Sidebar, type ScreenId } from "./components/Sidebar";
import { AgentRail } from "./components/AgentRail";
import { type AgentProfile } from "./components/AgentSwitcher";
import { Agents } from "./screens/Agents";
import { Settings } from "./screens/Settings";
import { Chat } from "./screens/Chat";
import { Browser } from "./screens/Browser";
import { SavePoints } from "./screens/SavePoints";
import { Tools } from "./screens/Tools";
import { Scheduler } from "./screens/Scheduler";
import { Connections } from "./screens/Connections";
import { initTheme, saveTheme, type Mode } from "./lib/theme";
import { startHeadlessWatcher } from "./lib/turns";

// Phase 1: app shell (sidebar nav + content pane) on the design system.

type Status =
  | { kind: "booting" }
  | { kind: "no-daemon" }
  | { kind: "connecting"; port: number }
  | { kind: "connected"; port: number; latency?: number }
  | { kind: "error"; msg: string };

export function App() {
  const [status, setStatus] = useState<Status>({ kind: "booting" });
  const [folder, setFolder] = useState<string | null>(null);
  const [activeAgent, setActiveAgent] = useState<AgentProfile | null>(null);
  // Multi-agent: which agents have an OPEN chat pane (side by side). Stored as a
  // SET (membership only) — the render ORDER is always derived from the rail's
  // roster order (see paneIds below), so top-to-bottom in the rail == left-to-
  // right in the panes, no matter what sequence you open them in.
  const [openSet, setOpenSet] = useState<Set<string>>(new Set());
  // Roster order = the canonical ordering source (same list the rail renders).
  const [rosterOrder, setRosterOrder] = useState<string[]>([]);
  const [screen, setScreen] = useState<ScreenId>("chat");
  const [mode, setMode] = useState<Mode>("light");
  const [accent, setAccent] = useState("");
  const [keySet, setKeySet] = useState(false);

  // Load the canonical roster order (declared AFTER `screen` so the dep is in
  // scope — TDZ: referencing `screen` above its declaration crashed the module).
  useEffect(() => {
    invoke<{ agents: AgentProfile[] }>("agents_list")
      .then((r) => setRosterOrder((r.agents || []).filter((a) => !a.archived).map((a) => a.id)))
      .catch(() => {});
  }, [screen]);

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
        if (msg.type === "auth:ok") { sentAt = Date.now(); socket!.send(JSON.stringify({ type: "ping" })); }
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
  // Click a rail chip = TOGGLE its pane: closed → open (+ focus), open → closed.
  // (If we're not on the Chat screen, a click just brings you there + opens/
  // focuses — it never closes from another screen, since you can't see panes.)
  function onViewAgent(a: AgentProfile) {
    if (screen !== "chat") {
      setScreen("chat");
      setOpenSet((prev) => (prev.has(a.id) ? prev : new Set(prev).add(a.id)));
      focusAgent(a);
      return;
    }
    if (openSet.has(a.id)) {
      closePane(a.id); // toggle CLOSED
    } else {
      setOpenSet((prev) => new Set(prev).add(a.id)); // toggle OPEN
      focusAgent(a);
    }
  }
  function focusAgent(a: AgentProfile) {
    invoke<AgentProfile | null>("agents_set_active", { id: a.id })
      .then((updated) => { if (updated) onActiveChange(updated); else onActiveChange(a); })
      .catch(() => onActiveChange(a));
  }
  // Close a pane (only via its ×). If it was the focused agent, refocus a still-
  // open neighbor so the This-Agent screens don't track a closed pane.
  function closePane(id: string) {
    setOpenSet((prev) => {
      const next = new Set(prev); next.delete(id);
      if (activeAgent?.id === id) {
        const nextFocus = rosterOrder.find((rid) => next.has(rid));
        if (nextFocus) invoke<AgentProfile | null>("agents_set_active", { id: nextFocus })
          .then((u) => { if (u) onActiveChange(u); }).catch(() => {});
      }
      return next;
    });
  }
  // The panes to render, ALWAYS in roster order (rail order) so pane layout is
  // stable + synced with the rail. Fall back to the active agent when none open.
  const paneIds = rosterOrder.filter((id) => openSet.has(id));
  const effectivePaneIds = paneIds.length > 0
    ? paneIds
    : (activeAgent?.id ? [activeAgent.id] : []);

  return (
    <div style={{ display: "flex", minHeight: "100vh" }}>
      {/* Agent selector is the top-level axis → far left. */}
      <AgentRail
        viewingId={activeAgent?.id ?? null}
        openIds={effectivePaneIds}
        onView={onViewAgent}
        onManage={() => setScreen("agents")}
      />
      <Sidebar active={screen} onSelect={setScreen} />
      <div style={{ flex: 1, height: "100vh", overflowY: "auto", display: "flex", flexDirection: "column" }}>
        {/* The persistent daemon-status strip was dev telemetry — removed. The
           connection state now lives as a quiet sanity-check in Settings.
           full-height flex column so height:100% children (Chat) can fill the
           window and pin their footer to the bottom (no dead whitespace). */}
        <div style={{ padding: "28px 32px", flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
          {screen === "chat" && (
            <Chat
              keySet={keySet}
              paneIds={effectivePaneIds}
              activeId={activeAgent?.id ?? null}
              onClosePane={closePane}
              onFocusPane={(id) => { invoke<AgentProfile | null>("agents_set_active", { id }).then((u) => { if (u) onActiveChange(u); }).catch(() => {}); }}
            />
          )}
          {screen === "agents" && (
            <Agents
              activeId={activeAgent?.id ?? null}
              onActiveChange={onActiveChange}
              onPickFolder={pickFolder}
              pendingFolder={folder}
            />
          )}
          {screen === "settings" && (
            <Settings mode={mode} accent={accent} onTheme={onTheme} folder={folder} onPickFolder={pickFolder} agentId={activeAgent?.id ?? null} daemonStatus={statusLine(status)} daemonOk={good} />
          )}
          {screen === "savepoints" && <SavePoints folder={folder} />}
          {screen === "scheduler" && <Scheduler agentId={activeAgent?.id ?? null} />}
          {screen === "connections" && <Connections agentId={activeAgent?.id ?? null} />}
          {screen === "tools" && <Tools folder={folder} />}
          {screen === "browser" && <Browser />}
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
