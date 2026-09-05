import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

// The agent switcher rail (§10.2) — a Slack-workspace-style column of agent
// chips. Clicking sets the active agent (which re-points the broker jail at
// that agent's folder, backend-side) and reloads the active agent's context.
// Sits to the LEFT of the nav Sidebar.

export type AgentProfile = {
  id: string;
  name: string;
  icon: string;
  color: string;
  folder_path: string;
  model: string;
  provider: string;
  model_variant?: string;
  context_mode: string;
  system_prompt: string;
  created_at: number;
  updated_at: number;
  archived: boolean;
  sort_order?: number;
  telegram_enabled?: boolean;
  telegram_bot_username?: string;
  telegram_allowed_chats?: string;
};

export function AgentSwitcher({
  activeId,
  onActiveChange,
  onManage,
}: {
  activeId: string | null;
  onActiveChange: (a: AgentProfile) => void;
  onManage: () => void;
}) {
  const [agents, setAgents] = useState<AgentProfile[]>([]);

  async function refresh() {
    try {
      const r = await invoke<{ agents: AgentProfile[]; activeId: string }>("agents_list");
      setAgents((r.agents || []).filter((a) => !a.archived));
    } catch { /* daemon may not be up yet */ }
  }
  useEffect(() => { refresh(); }, [activeId]);

  async function pick(a: AgentProfile) {
    if (a.id === activeId) return;
    try {
      const updated = await invoke<AgentProfile | null>("agents_set_active", { id: a.id });
      // Slice 6: mark the active agent so the browser uses its per-agent profile.
      invoke("set_active_agent_marker", { id: a.id }).catch(() => {});
      if (updated) onActiveChange(updated);
    } catch { /* ignore */ }
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 10,
        width: 64,
        padding: "16px 0",
        borderRight: "var(--border-width) solid var(--line)",
        background: "var(--bg-elevated, var(--bg))",
      }}
    >
      {agents.map((a) => {
        const active = a.id === activeId;
        return (
          <button
            key={a.id}
            onClick={() => pick(a)}
            title={`${a.name}${a.model ? ` · ${a.model}` : ""}`}
            style={{
              width: 42,
              height: 42,
              borderRadius: active ? 14 : 21,
              border: active ? `2px solid ${a.color}` : "2px solid transparent",
              background: active ? a.color : "var(--bg)",
              color: active ? "#fff" : "var(--text)",
              boxShadow: active ? `0 0 0 3px ${a.color}22` : "none",
              fontSize: 20,
              cursor: "pointer",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              transition: "border-radius .15s ease, background .15s ease",
            }}
          >
            {a.icon || "🤖"}
          </button>
        );
      })}

      {/* + New / manage agents */}
      <button
        onClick={onManage}
        title="New agent"
        style={{
          width: 42,
          height: 42,
          borderRadius: 21,
          border: "2px dashed var(--line)",
          background: "transparent",
          color: "var(--text-muted)",
          fontSize: 24,
          lineHeight: 1,
          cursor: "pointer",
        }}
      >
        +
      </button>
    </div>
  );
}
