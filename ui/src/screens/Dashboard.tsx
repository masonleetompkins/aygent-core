// DASHBOARD SCREEN — per-agent, prompt-built, pull-only.
//
// THE SAFETY RULE (Mason, 2026-08-04): the dashboard NEVER runs an LLM call
// automatically. Nothing here polls, ticks, or refreshes on mount beyond a
// single read of already-stored values. Two consequences visible in this file:
//
//  1. The global Refresh button refreshes FREE sources only. Paid (agent_turn)
//     modules are deliberately excluded — one click fanning out to N model calls
//     is exactly the ambiguity the rule exists to prevent. They get a separate,
//     explicitly-labeled control that names the cost.
//  2. Because nothing auto-updates, every non-static module shows its age.
//     Stale-but-honest beats fresh-but-surprising.
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Input } from "../components/ui";
import { Icon } from "../components/Icon";
import {
  ModuleBody, ModuleMeta, cardStyle, type ModuleRow,
} from "../components/dashboard/Modules";

const GRID_COLS = 12;
const ROW_H = 44;
const GAP = 16;

interface DashboardView {
  id: string;
  agent_id: string;
  title: string;
  modules: ModuleRow[];
}

export function Dashboard({ agentId, agentName }: { agentId: string | null; agentName?: string }) {
  const [view, setView] = useState<DashboardView | null>(null);
  const [busy, setBusy] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(() => {
    if (!agentId) return;
    invoke<DashboardView>("dashboard_load", { agentId })
      .then(setView)
      .catch((e) => setNote(String(e)));
  }, [agentId]);

  // Load ONLY. This reads stored rows; it does not fetch, run, or spend.
  useEffect(() => { load(); }, [load]);

  const modules = view?.modules ?? [];
  const paidCount = modules.filter((m) => m.is_paid).length;
  const freeRefreshable = modules.filter((m) => !m.is_paid && m.spec.source?.kind !== "static").length;

  // M2 wires this to the agent tool loop. Keeping the bar live now (disabled,
  // labeled) rather than hiding it: the prompt bar IS the feature, and shipping
  // the shell first means the plumbing lands against a real surface.
  function submitPrompt() {
    const p = prompt.trim();
    if (!p) return;
    setNote("The prompt bar lands in M2 — the dashboard tools aren't wired to the agent loop yet.");
  }

  if (!agentId) {
    return <Centered>Pick an agent to see its dashboard.</Centered>;
  }

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
          disabled={busy || freeRefreshable === 0}
          onClick={() => setNote("Live data sources (and this button's fetch) land in M3.")}
        >
          <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
            <Icon name="refresh" size={15} />
            Refresh
          </span>
        </Button>

        {/* The paid path is ALWAYS a separate, self-describing control. */}
        {paidCount > 0 && (
          <Button
            variant="secondary"
            disabled={busy}
            onClick={() => setNote("Model-backed modules land in M5.")}
          >
            Refresh {paidCount} model {paidCount === 1 ? "module" : "modules"}
          </Button>
        )}
      </div>

      {/* ---- prompt bar: the builder surface ---- */}
      <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
        <Input
          value={prompt}
          onChange={(e) => setPrompt((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === "Enter") submitPrompt(); }}
          placeholder="Describe a module to add — “show my open PRs as a list”"
        />
        <Button onClick={submitPrompt} disabled={!prompt.trim()}>Build</Button>
        <Button
          variant="secondary"
          onClick={() => invoke("dashboard_undo", { agentId }).then(load).catch((e) => setNote(String(e)))}
        >
          Undo
        </Button>
      </div>

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
      {modules.length === 0 ? (
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
      <div style={{ fontSize: 14, maxWidth: 420, lineHeight: 1.5 }}>
        Describe what you want to see and your agent will build it — or start from a preset.
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
