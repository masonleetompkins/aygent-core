// DASHBOARD SCREEN — per-agent, prompt-built, pull-only.
//
// THE SAFETY RULE (Mason, 2026-08-04): the dashboard NEVER runs an LLM call
// automatically. Nothing here polls, ticks, or refreshes on mount beyond a
// single read of already-stored values. Three consequences visible in this file:
//
//  1. The global Refresh button refreshes FREE sources only. Paid (agent_turn)
//     modules are deliberately excluded — one click fanning out to N model calls
//     is exactly the ambiguity the rule exists to prevent. They get a separate,
//     explicitly-labeled control that names the cost.
//  2. Because nothing auto-updates, every non-static module shows its age.
//     Stale-but-honest beats fresh-but-surprising.
//  3. The prompt bar spends money — so it only ever fires on Enter/Build, and
//     it says so while it's running.
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Input } from "../components/ui";
import { Icon } from "../components/Icon";
import { runTurn, useAgentTurn, isRunning, stopTurn, type ToolCard } from "../lib/turns";
import { ModuleBody, ModuleMeta, cardStyle, type ModuleRow } from "../components/dashboard/Modules";

const GRID_COLS = 12;
const ROW_H = 44;
const GAP = 16;

interface DashboardView {
  id: string;
  agent_id: string;
  title: string;
  modules: ModuleRow[];
}

export function Dashboard({
  agentId, agentName, folder,
}: { agentId: string | null; agentName?: string; folder?: string | null }) {
  const [view, setView] = useState<DashboardView | null>(null);
  const [prompt, setPrompt] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [channel, setChannel] = useState<string | null>(null);

  // Subscribe to the shared turn store so the builder shows the SAME live tool
  // cards the Chat screen would — the user watches modules being authored.
  const turn = useAgentTurn(agentId);
  const building = !!channel && isRunning(agentId);

  const load = useCallback(() => {
    if (!agentId) return;
    invoke<DashboardView>("dashboard_load", { agentId })
      .then(setView)
      .catch((e) => setNote(String(e)));
  }, [agentId]);

  // Load ONLY. This reads stored rows; it does not fetch, run, or spend.
  useEffect(() => { load(); }, [load]);

  // LIVE BUILD: re-read the dashboard as each dashboard_* tool call completes,
  // so modules appear while the model is still talking. We key off the count of
  // finished tool cards rather than polling — no timer, and it costs one cheap
  // SQLite read per tool call.
  const doneToolCount = turn.liveTools.filter(
    (t: ToolCard) => !t.running && t.name?.startsWith("dashboard_"),
  ).length;
  useEffect(() => { if (doneToolCount > 0) load(); }, [doneToolCount, load]);

  const modules = view?.modules ?? [];
  const paidCount = modules.filter((m) => m.is_paid).length;
  const freeRefreshable = modules.filter((m) => !m.is_paid && m.spec.source?.kind !== "static").length;

  async function submitPrompt() {
    const p = prompt.trim();
    if (!p || !agentId || building) return;
    setNote(null);
    setPrompt("");
    const ch = `dash-${agentId}-${Date.now()}`;
    setChannel(ch);
    try {
      // Scoped instruction: this bar is a BUILDER, so keep the model on the
      // dashboard tools instead of answering conversationally.
      await runTurn({
        agentId,
        channel: ch,
        prompt:
          `[Dashboard builder] The user is on their dashboard and wants to change it. ` +
          `Use the dashboard_* tools to make it happen, then reply in ONE short sentence ` +
          `describing what you built. Request: ${p}`,
        model: null,
        provider: null,
        folder: folder ?? null,
        sessionId: "",
      });
    } catch (e) {
      setNote(String(e));
    } finally {
      setChannel(null);
      load(); // final reconcile
    }
  }

  if (!agentId) return <Centered>Pick an agent to see its dashboard.</Centered>;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 20, flex: 1, minHeight: 0 }}>
      {/* ---- header ---- */}
      <div style={{ display: "flex", alignItems: "flex-end", gap: 16, flexWrap: "wrap" }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 26, fontWeight: 700, letterSpacing: "-0.01em" }}>
            {view?.title ?? "Dashboard"}
          </div>
          <div style={{ fontSize: 13, color: "var(--text-muted)", marginTop: 2 }}>
            {agentName ? `${agentName} · ` : ""}
            {modules.length} {modules.length === 1 ? "module" : "modules"}
          </div>
        </div>

        {/* Free refresh. Never touches paid modules — see the rule at the top. */}
        <Button
          variant="secondary"
          disabled={freeRefreshable === 0}
          onClick={() => setNote("Live data sources (and this button's fetch) land in M3.")}
        >
          <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
            <Icon name="refresh" size={15} />
            Refresh
          </span>
        </Button>

        {/* The paid path is ALWAYS a separate, self-describing control. */}
        {paidCount > 0 && (
          <Button variant="secondary" onClick={() => setNote("Model-backed modules land in M5.")}>
            Refresh {paidCount} model {paidCount === 1 ? "module" : "modules"}
          </Button>
        )}
      </div>

      {/* ---- prompt bar: the builder surface ---- */}
      <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
        <Input
          value={prompt}
          disabled={building}
          onChange={(e) => setPrompt((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === "Enter") void submitPrompt(); }}
          placeholder={building ? "Building…" : "Describe a module to add — “show my open PRs as a list”"}
        />
        {building ? (
          <Button variant="secondary" onClick={() => { if (channel) void stopTurn(channel); }}>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
              <Icon name="stop" size={14} /> Stop
            </span>
          </Button>
        ) : (
          <Button onClick={() => void submitPrompt()} disabled={!prompt.trim()}>Build</Button>
        )}
        <Button
          variant="secondary"
          onClick={() => invoke("dashboard_undo", { agentId }).then(load).catch((e) => setNote(String(e)))}
        >
          Undo
        </Button>
      </div>

      {/* ---- live build feedback: the model's tool calls, as they happen ---- */}
      {building && (
        <div style={{
          display: "flex", flexDirection: "column", gap: 6, padding: "12px 14px",
          border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
          background: "var(--surface)", boxShadow: "var(--elevation)",
        }}>
          {turn.liveTools.length === 0 && (
            <span style={{ fontSize: 13, color: "var(--text-muted)" }}>Thinking…</span>
          )}
          {turn.liveTools.map((t: ToolCard, i: number) => (
            <div key={i} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
              <span style={{ color: t.running ? "var(--text-faint)" : t.ok === false ? "var(--danger)" : "var(--ok)" }}>
                {t.running ? "○" : t.ok === false ? "✕" : "✓"}
              </span>
              <span style={{ color: "var(--text-muted)" }}>{t.summary || t.name}</span>
            </div>
          ))}
          {turn.liveText && (
            <div style={{ fontSize: 13, color: "var(--text)", marginTop: 4 }}>{turn.liveText}</div>
          )}
        </div>
      )}

      {note && (
        <div style={{
          fontSize: 13, color: "var(--text-muted)", padding: "10px 14px",
          border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
          display: "flex", alignItems: "center", gap: 10,
        }}>
          <span style={{ flex: 1 }}>{note}</span>
          <button
            onClick={() => setNote(null)}
            style={{ background: "none", border: "none", cursor: "pointer", color: "var(--text-faint)", display: "flex" }}
          >
            <Icon name="close" size={14} />
          </button>
        </div>
      )}

      {/* ---- the grid ---- */}
      {modules.length === 0 && !building ? (
        <EmptyState />
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: `repeat(${GRID_COLS}, 1fr)`,
            gridAutoRows: `${ROW_H}px`,
            gap: GAP,
            alignContent: "start",
            paddingBottom: 24,
          }}
        >
          {modules.map((row) => (
            <section
              key={row.id}
              style={{
                ...cardStyle,
                gridColumn: `${row.spec.layout.x + 1} / span ${row.spec.layout.w}`,
                gridRow: `span ${row.spec.layout.h}`,
              }}
            >
              <header style={{ display: "flex", alignItems: "flex-start", gap: 10 }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{
                    fontSize: "var(--text-caption)", letterSpacing: "0.12em", textTransform: "uppercase",
                    fontWeight: 700, color: "var(--text-muted)",
                    overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
                  }}>
                    {row.spec.title}
                  </div>
                  <div style={{ marginTop: 5 }}><ModuleMeta row={row} /></div>
                </div>
                <button
                  title="Remove module"
                  onClick={() => {
                    void invoke("dashboard_remove_module", { agentId, moduleId: row.id })
                      .then(load)
                      .catch((e) => setNote(String(e)));
                  }}
                  style={{ background: "none", border: "none", cursor: "pointer", color: "var(--text-faint)", display: "flex", padding: 2 }}
                >
                  <Icon name="close" size={14} />
                </button>
              </header>
              <ModuleBody row={row} />
            </section>
          ))}
        </div>
      )}
    </div>
  );
}

function EmptyState() {
  return (
    <div style={{
      flex: 1, display: "flex", flexDirection: "column", alignItems: "center",
      justifyContent: "center", gap: 12, textAlign: "center", padding: 40,
      border: "var(--border-width) dashed var(--line)", borderRadius: "var(--radius-card)",
      color: "var(--text-muted)", minHeight: 260,
    }}>
      <div style={{ fontSize: 17, fontWeight: 600, color: "var(--text)" }}>No modules yet</div>
      <div style={{ fontSize: 14, maxWidth: 440, lineHeight: 1.5 }}>
        Describe what you want to see and your agent will build it. Try
        {" "}<em>“add a stat card for my open PRs”</em>.
        Nothing here runs on its own; data only moves when you ask.
      </div>
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", color: "var(--text-muted)", fontSize: 14 }}>
      {children}
    </div>
  );
}
