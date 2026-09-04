import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Sidebar, type ScreenId } from "./components/Sidebar";
import { AgentRail } from "./components/AgentRail";
import { type AgentProfile } from "./components/AgentSwitcher";
import { Agents } from "./screens/Agents";
import { Settings } from "./screens/Settings";
import { Chat } from "./screens/Chat";
import { Dashboard } from "./screens/Dashboard";
import { Browser } from "./screens/Browser";
import { SavePoints } from "./screens/SavePoints";
import { Tools, Skills } from "./screens/Tools";
import { Sparks } from "./screens/Sparks";
import { Video } from "./screens/Video";
import { McpConnections } from "./screens/McpConnections";
import { applyAppIcon } from "./lib/appIcon";
import { Scheduler } from "./screens/Scheduler";
import { Connections } from "./screens/Connections";
import { Onboarding } from "./screens/Onboarding";
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
  // UI task #4: browser sidebar entry only when installed (Tools owns enabling).
  const [browserInstalled, setBrowserInstalled] = useState(false);
  useEffect(() => {
    const check = () => { invoke<{ installed: boolean }>("browser_status").then((s) => setBrowserInstalled(!!s?.installed)).catch(() => {}); };
    check();
    window.addEventListener("aygent-browser-changed", check);
    return () => window.removeEventListener("aygent-browser-changed", check);
  }, []);
  const [mode, setMode] = useState<Mode>("light");
  const [accent, setAccent] = useState("");
  const [keySet, setKeySet] = useState(false);
  // Bumped whenever the agent roster changes (create/delete/update in the Agents
  // screen). Both the AgentRail and the roster-order effect key off this, so a
  // delete is reflected everywhere immediately — no app restart. Previously the
  // rail kept showing a deleted agent because nothing told it to re-list.
  const [rosterRefresh, setRosterRefresh] = useState(0);
  // ONBOARDING gate: null = checking, true = show the wizard (no root configured),
  // false = normal app. Checked on boot via onboarding_status.
  const [needsOnboarding, setNeedsOnboarding] = useState<boolean | null>(null);
  useEffect(() => {
    invoke<{ needsOnboarding: boolean }>("onboarding_status")
      .then((r) => setNeedsOnboarding(!!r.needsOnboarding))
      .catch(() => setNeedsOnboarding(false)); // fail open to the app if the check errors
  }, []);

  // Load the canonical roster order (declared AFTER `screen` so the dep is in
  // scope — TDZ: referencing `screen` above its declaration crashed the module).
  // Re-runs on rosterRefresh so a delete drops the agent from panes/openSet too.
  useEffect(() => {
    invoke<{ agents: AgentProfile[] }>("agents_list")
      .then((r) => {
        const live = (r.agents || []).filter((a) => !a.archived);
        const liveIds = live.map((a) => a.id);
        setRosterOrder(liveIds);
        // Prune any open panes for agents that no longer exist.
        setOpenSet((prev) => {
          const next = new Set([...prev].filter((id) => liveIds.includes(id)));
          return next.size === prev.size ? prev : next;
        });
      })
      .catch(() => {});
  }, [screen, rosterRefresh]);

  useEffect(() => { const t = initTheme(); setMode(t.mode); setAccent(t.accent); }, []);
  // DYNAMIC APP ICON: redraw + install whenever the theme changes (and once on
  // boot, after initTheme populates these). Fire-and-forget — a themed Dock
  // icon is a nicety and must never block or break startup.
  useEffect(() => { void applyAppIcon(mode, accent); }, [mode, accent]);
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
  // Chat "ready" gate is per-agent: check the key for the provider THIS agent
  // uses. Local (in-process GGUF) needs no key at all. Previously this was
  // hardwired to "anthropic", so a local-only (or OpenAI/OpenRouter/Meta-only)
  // setup was blocked with "add an Anthropic key" even though the backend was fine.
  useEffect(() => {
    const p = activeAgent?.provider || "anthropic";
    if (p === "local") { setKeySet(true); return; }
    invoke<boolean>("has_provider_key", { provider: p }).then(setKeySet).catch(() => {});
  }, [screen, activeAgent?.id, activeAgent?.provider]);
  // Settings can change the active agent's provider/model (set_selection) without
  // going through the switcher — re-read the profile on every screen change so the
  // gate above sees the new provider instead of a stale one.
  useEffect(() => {
    invoke<AgentProfile | null>("agents_get_active").then((a) => { if (a) setActiveAgent(a); }).catch(() => {});
  }, [screen]);
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

  // While we don't know onboarding state yet, render nothing (avoids a flash of
  // the main app before the wizard). Once known, show the wizard OR the app.
  if (needsOnboarding === null) {
    return <div style={{ minHeight: "100vh", background: "var(--bg)" }} />;
  }
  if (needsOnboarding) {
    // On finish: NO process restart. onboarding_set_root already re-pointed the
    // live DB to <root>/.aygent (the writer thread swapped its connection), so
    // the agent created in step 3 is already in the root's DB. We just flip the
    // gate off + re-check status so the app renders. (app.restart() from inside
    // a command future was aborting the process — SIGABRT; removed.)
    return <Onboarding onDone={() => {
      void invoke("onboarding_finish").catch(() => {});
      setNeedsOnboarding(false);
    }} />;
  }

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
        refreshKey={rosterRefresh}
      />
      <Sidebar active={screen} onSelect={setScreen} showBrowser={browserInstalled} />
      <div style={{ flex: 1, height: "100vh", minHeight: 0, minWidth: screen === "video" ? 0 : undefined, overflowY: "auto", display: "flex", flexDirection: "column" }}>
        {/* The persistent daemon-status strip was dev telemetry — removed. The
           connection state now lives as a quiet sanity-check in Settings.
           full-height flex column so height:100% children (Chat) can fill the
           window and pin their footer to the bottom (no dead whitespace).
           minHeight:0 on BOTH this and the scroll parent is what lets flex
           children actually shrink below their content size — without it, a
           child measures its frozen intrinsic size (the Browser pane was stuck
           at its large-window rect because this chain couldn't shrink). */}
        <div style={{ padding: screen === "video" ? 0 : "28px 32px", flex: 1, minHeight: 0, minWidth: 0, display: "flex", flexDirection: "column", overflow: screen === "video" ? "hidden" : undefined }}>
          {screen === "dashboard" && (
            <Dashboard agentId={activeAgent?.id ?? null} agentName={activeAgent?.name} folder={folder} onNavigate={(sc) => setScreen(sc as ScreenId)} />
          )}
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
              onRosterChange={() => setRosterRefresh((n) => n + 1)}
              onOpenChat={(a) => {
                // Open a chat for this agent: make it active, then jump to Chat.
                invoke<AgentProfile | null>("agents_set_active", { id: a.id })
                  .then((u) => { onActiveChange(u ?? a); })
                  .catch(() => onActiveChange(a))
                  .finally(() => setScreen("chat"));
              }}
            />
          )}
          {screen === "settings" && (
            <Settings mode={mode} accent={accent} onTheme={onTheme} folder={folder} onPickFolder={pickFolder} agentId={activeAgent?.id ?? null} daemonStatus={statusLine(status)} daemonOk={good} />
          )}
          {screen === "savepoints" && <SavePoints folder={folder} />}
          {screen === "scheduler" && <Scheduler agentId={activeAgent?.id ?? null} />}
          {screen === "connections" && <Connections agentId={activeAgent?.id ?? null} />}
          {screen === "tools" && <Tools folder={folder} agentId={activeAgent?.id ?? null} />}
          {screen === "skills" && <Skills folder={folder} agentId={activeAgent?.id ?? null} />}
          {screen === "sparks" && <Sparks agentId={activeAgent?.id ?? null} onNavigate={(sc) => setScreen(sc as ScreenId)} />}
          {screen === "video" && <Video agentId={activeAgent?.id ?? null} agentName={activeAgent?.name} folder={folder} />}
          {screen === "mcp" && <McpConnections />}
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
