import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentProfile } from "./AgentSwitcher";

// AGENT RAIL (M1.4 parallel UI) — a persistent secondary sidebar of ALL agents.
// Replaces the "switch the one active agent" model: agents all run in parallel
// in the background; this rail just picks WHICH ONE YOU'RE LOOKING AT
// (viewingAgentId). Each chip shows live status:
//   • idle    — nothing running
//   • working — a turn is in flight (human OR inter-agent), pulsing ring
//   • unread  — an inter-agent message landed while you weren't looking (badge)
// Status is driven by the global `agent-activity` broadcast + a periodic poll of
// mailbox_pending_counts (unread) as a safety net.

type Activity = { agentId: string; kind: "turn_start" | "turn_done"; from?: string; fromName?: string };

export function AgentRail({
  viewingId,
  onView,
  onManage,
  refreshKey,
}: {
  viewingId: string | null;
  onView: (a: AgentProfile) => void;
  onManage: () => void;
  refreshKey?: number; // bump to force a re-list (e.g. after create/delete)
}) {
  const [agents, setAgents] = useState<AgentProfile[]>([]);
  const [working, setWorking] = useState<Record<string, boolean>>({});
  const [unread, setUnread] = useState<Record<string, number>>({});

  async function refresh() {
    try {
      const r = await invoke<{ agents: AgentProfile[]; activeId: string }>("agents_list");
      setAgents((r.agents || []).filter((a) => !a.archived));
    } catch { /* daemon may not be up yet */ }
  }
  useEffect(() => { void refresh(); }, [refreshKey]);

  // Unread counts (inter-agent messages pending/just-delivered per agent).
  async function refreshUnread() {
    try {
      const counts = await invoke<Array<[string, number]>>("mailbox_pending_counts");
      const map: Record<string, number> = {};
      for (const [id, n] of counts) map[id] = n;
      setUnread(map);
    } catch { /* ignore */ }
  }
  useEffect(() => {
    void refreshUnread();
    const t = setInterval(refreshUnread, 3000);
    return () => clearInterval(t);
  }, []);

  // Live activity: agents starting/finishing turns. Drives the working ring +
  // clears/raises the unread badge.
  useEffect(() => {
    let un: (() => void) | undefined;
    listen<Activity>("agent-activity", (ev) => {
      const a = ev.payload;
      if (!a?.agentId) return;
      if (a.kind === "turn_start") {
        setWorking((w) => ({ ...w, [a.agentId]: true }));
      } else if (a.kind === "turn_done") {
        setWorking((w) => ({ ...w, [a.agentId]: false }));
        // A finished inter-agent turn means new content in that agent's inbox —
        // bump unread unless you're currently looking at it.
        setUnread((u) => (a.agentId === viewingId ? u : { ...u, [a.agentId]: (u[a.agentId] || 0) + 1 }));
      }
    }).then((f) => { un = f; });
    return () => { if (un) un(); };
  }, [viewingId]);

  // Clear unread for the agent you're now viewing.
  useEffect(() => {
    if (viewingId) setUnread((u) => (u[viewingId] ? { ...u, [viewingId]: 0 } : u));
  }, [viewingId]);

  function pick(a: AgentProfile) {
    onView(a); // NOTE: does NOT change which agents run — only what you view.
  }

  return (
    <div
      style={{
        display: "flex", flexDirection: "column", alignItems: "center", gap: 10,
        width: 64, padding: "16px 0", flexShrink: 0,
        borderRight: "var(--border-width) solid var(--line)",
        background: "var(--bg-elevated, var(--bg))",
      }}
      title="Your agents — all run in parallel"
    >
      {agents.map((a) => {
        const viewing = a.id === viewingId;
        const busy = !!working[a.id];
        const badge = unread[a.id] || 0;
        return (
          <button
            key={a.id}
            onClick={() => pick(a)}
            title={`${a.name}${a.model ? ` · ${a.model}` : ""}${busy ? " · working…" : ""}`}
            style={{
              position: "relative",
              width: 42, height: 42, borderRadius: viewing ? 14 : 21,
              border: viewing ? `2px solid ${a.color}` : "2px solid transparent",
              background: viewing ? a.color : "var(--bg)",
              color: viewing ? "#fff" : "var(--text)",
              boxShadow: busy
                ? `0 0 0 3px ${a.color}66`
                : viewing ? `0 0 0 3px ${a.color}22` : "none",
              animation: busy ? "aygentPulse 1.1s ease-in-out infinite" : "none",
              fontSize: 20, cursor: "pointer",
              display: "flex", alignItems: "center", justifyContent: "center",
              transition: "border-radius .15s ease, background .15s ease",
            }}
          >
            {a.icon || "🤖"}
            {badge > 0 && (
              <span style={{
                position: "absolute", top: -3, right: -3, minWidth: 16, height: 16,
                padding: "0 4px", borderRadius: 8, background: "var(--danger, #ef4444)",
                color: "#fff", fontSize: 10, fontWeight: 800, lineHeight: "16px",
                display: "flex", alignItems: "center", justifyContent: "center",
              }}>{badge}</span>
            )}
          </button>
        );
      })}

      <button
        onClick={onManage}
        title="Manage agents"
        style={{
          width: 42, height: 42, borderRadius: 21, border: "2px dashed var(--line)",
          background: "transparent", color: "var(--text-muted)", fontSize: 24,
          lineHeight: 1, cursor: "pointer",
        }}
      >+</button>

      {/* keyframes for the working pulse (scoped-ish via a style tag) */}
      <style>{`@keyframes aygentPulse { 0%,100% { box-shadow: 0 0 0 3px var(--pulse-a, rgba(91,140,255,.3)); } 50% { box-shadow: 0 0 0 6px rgba(91,140,255,.12); } }`}</style>
    </div>
  );
}
