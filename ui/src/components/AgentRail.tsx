import { useEffect, useState, useRef, useLayoutEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentProfile } from "./AgentSwitcher";
import { Icon, type IconName } from "./Icon";

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
  openIds,
  onView,
  onManage,
  refreshKey,
}: {
  viewingId: string | null;
  openIds?: string[]; // agents with an OPEN chat pane (side-by-side dot)
  onView: (a: AgentProfile) => void;
  onManage: () => void;
  refreshKey?: number; // bump to force a re-list (e.g. after create/delete)
}) {
  const [agents, setAgents] = useState<AgentProfile[]>([]);
  const [working, setWorking] = useState<Record<string, boolean>>({});
  const [unread, setUnread] = useState<Record<string, number>>({});
  // AYGENT Remote: paired/enabled/running for the rail-bottom toggle.
  const [remote, setRemote] = useState<{ paired: boolean; enabled: boolean; running: boolean } | null>(null);

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

  // Remote status: local-only command (keychain + a bool) — cheap to poll.
  async function refreshRemote() {
    try { setRemote(await invoke("remote_local_status")); } catch { /* ignore */ }
  }
  useEffect(() => {
    void refreshRemote();
    const t = setInterval(refreshRemote, 5000);
    return () => clearInterval(t);
  }, []);

  async function toggleRemote() {
    if (!remote) return;
    const next = !remote.enabled;
    // Optimistic: the socket teardown/bring-up follows within a beat.
    setRemote({ ...remote, enabled: next, running: next ? remote.running : false });
    try { await invoke("remote_set_enabled", { on: next }); } catch { /* ignore */ }
    setTimeout(refreshRemote, 1200);
  }

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

  // ---- FLIP reorder animation ----------------------------------------------
  // Each agent button registers its DOM node; before every paint we compare its
  // new top to its last top and, if it moved, jump it back to the old spot with
  // a transform (no transition) then release to 0 with a transition on the next
  // frame — so it SLIDES from where it was to where it now is. Fast + smooth.
  const btnRefs = useRef<Map<string, HTMLButtonElement>>(new Map());
  const lastTops = useRef<Map<string, number>>(new Map());
  useLayoutEffect(() => {
    const tops = new Map<string, number>();
    btnRefs.current.forEach((el, id) => { tops.set(id, el.getBoundingClientRect().top); });
    tops.forEach((newTop, id) => {
      const prev = lastTops.current.get(id);
      const el = btnRefs.current.get(id);
      if (prev == null || !el) return;
      const dy = prev - newTop;
      if (Math.abs(dy) < 1) return;
      // Invert: place it back where it was, no transition.
      el.style.transition = "none";
      el.style.transform = `translateY(${dy}px)`;
      // Play: next frame, transition to home.
      requestAnimationFrame(() => {
        el.style.transition = "transform 220ms cubic-bezier(.2,.8,.2,1)";
        el.style.transform = "";
      });
    });
    lastTops.current = tops;
  }, [agents]);

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
        const open = openIds?.includes(a.id) ?? false;
        return (
          <button
            key={a.id}
            ref={(el) => { if (el) btnRefs.current.set(a.id, el); else btnRefs.current.delete(a.id); }}
            onClick={() => pick(a)}
            title={`${a.name}${a.model ? ` · ${a.model}` : ""}${busy ? " · working…" : ""}`}
            style={{
              position: "relative",
              width: 42, height: 42, borderRadius: "var(--radius-control)",
              // NO background color fill — the SF-symbol itself carries the
              // accent color (flat, no glow — Mason's call). Viewing = accent
              // outline + accent-tinted icon; idle = quiet muted icon.
              border: viewing ? "var(--border-width) solid var(--accent)" : "var(--border-width) solid transparent",
              background: "transparent",
              color: viewing ? "var(--accent)" : "var(--text-muted)",
              boxShadow: viewing ? "var(--elevation)" : "none",
              animation: busy ? "aygentPulse 1.1s ease-in-out infinite" : "none",
              cursor: "pointer",
              display: "flex", alignItems: "center", justifyContent: "center",
              transition: "border .15s ease, color .15s ease, box-shadow .15s ease",
            }}
          >
            <Icon name={(a.icon as IconName) || "sparkles"} size={22} />
            {/* open-pane indicator: a small accent dot on the left edge so you
                can see at a glance which agents have a live side-by-side pane. */}
            {open && !viewing && (
              <span style={{
                position: "absolute", left: -1, top: "50%", transform: "translateY(-50%)",
                width: 3, height: 18, borderRadius: 2, background: "var(--accent)",
              }} />
            )}
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
          width: 42, height: 42, borderRadius: "var(--radius-pill)", border: "var(--border-width) dashed var(--line)",
          background: "transparent", color: "var(--text-muted)", cursor: "pointer",
          display: "flex", alignItems: "center", justifyContent: "center",
        }}
      ><Icon name="plus" size={18} /></button>

      {/* AYGENT Remote toggle — only when a device is paired. Online = the
          Mac holds its relay session (reachable from the web); offline =
          paired but silent (no socket, no heartbeat). */}
      {remote?.paired && (
        <button
          onClick={toggleRemote}
          title={remote.enabled
            ? (remote.running ? "Remote: online — reachable from the web. Click to go offline." : "Remote: connecting…")
            : "Remote: offline (still paired). Click to go online."}
          style={{
            position: "relative",
            width: 42, height: 42, borderRadius: "var(--radius-pill)",
            border: "var(--border-width) solid transparent",
            background: "transparent", cursor: "pointer",
            color: remote.enabled && remote.running ? "var(--accent)" : "var(--text-muted)",
            display: "flex", alignItems: "center", justifyContent: "center",
            marginTop: "auto",
            opacity: remote.enabled ? 1 : 0.55,
            transition: "color .15s ease, opacity .15s ease",
          }}
        >
          <Icon name="globe" size={19} />
          {/* offline slash */}
          {!remote.enabled && (
            <span style={{
              position: "absolute", width: 24, height: 2, borderRadius: 1,
              background: "var(--text-muted)", transform: "rotate(-45deg)",
            }} />
          )}
          {/* online dot */}
          {remote.enabled && remote.running && (
            <span style={{
              position: "absolute", top: 4, right: 4, width: 7, height: 7,
              borderRadius: "50%", background: "var(--ok, #22c55e)",
            }} />
          )}
        </button>
      )}

      {/* keyframes for the working pulse (scoped-ish via a style tag) */}
      <style>{`@keyframes aygentPulse { 0%,100% { box-shadow: 0 0 0 3px var(--pulse-a, rgba(91,140,255,.3)); } 50% { box-shadow: 0 0 0 6px rgba(91,140,255,.12); } }`}</style>
    </div>
  );
}
