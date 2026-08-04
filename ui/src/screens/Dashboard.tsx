// DASHBOARD SCREEN — per-agent, prompt-built, pull-only.
//
// THE SAFETY RULE (Mason, 2026-08-04): the dashboard NEVER runs an LLM call
// automatically. Nothing here polls, ticks, or refreshes on mount beyond a
// single read of already-stored values. How that shows up in this file:
//
//  1. `Refresh` calls dashboard_refresh, which is hard-filtered on is_paid() in
//     RUST. Paid modules are excluded there, not here — the UI can't forget.
//  2. Paid modules get a separate, explicitly-labeled control that names the
//     cost, plus a per-module refresh on the card itself.
//  3. Exec modules render a pending state showing the literal command until a
//     human approves that exact string.
//  4. Nothing auto-updates, so every non-static module shows its age.
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Input } from "../components/ui";
import { Icon } from "../components/Icon";
import { runTurn, useAgentTurn, isRunning, stopTurn, type ToolCard } from "../lib/turns";
import { ModuleBody, ModuleMeta, cardStyle, type ModuleRow } from "../components/dashboard/Modules";
import { IconBtn } from "../components/dashboard/Modules2";
import { PRESETS } from "../components/dashboard/presets";
import type { ActionCtx } from "../components/dashboard/actions";
import { useGridDrag, ResizeGrip } from "../components/dashboard/useGridDrag";
import { GRID_COLS, ROW_H, GAP, type Placed } from "../components/dashboard/gridDrag";

interface DashboardView {
  id: string; agent_id: string; title: string; modules: ModuleRow[];
}

export function Dashboard({
  agentId, agentName, folder, onNavigate,
}: {
  agentId: string | null; agentName?: string; folder?: string | null;
  onNavigate?: (screen: string) => void;
}) {
  const [view, setView] = useState<DashboardView | null>(null);
  const [prompt, setPrompt] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [channel, setChannel] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [showPresets, setShowPresets] = useState(false);
  const [formValues, setFormValues] = useState<Record<string, string>>({});

  const turn = useAgentTurn(agentId);
  const building = !!channel && isRunning(agentId);

  const load = useCallback(() => {
    if (!agentId) return;
    invoke<DashboardView>("dashboard_load", { agentId })
      .then(setView)
      .catch((e) => setNote(String(e)));
  }, [agentId]);

  // Load ONLY — reads stored rows; does not fetch, run, or spend.
  useEffect(() => { load(); }, [load]);

  // LIVE BUILD: re-read as each dashboard_* tool call completes so modules
  // appear while the model is still talking. Keyed off finished tool cards —
  // no timer.
  const doneToolCount = turn.liveTools.filter(
    (t: ToolCard) => !t.running && t.name?.startsWith("dashboard_"),
  ).length;
  useEffect(() => { if (doneToolCount > 0) load(); }, [doneToolCount, load]);

  const modules = view?.modules ?? [];
  const paidCount = modules.filter((m) => m.is_paid).length;
  const freeFetchable = modules.filter(
    (m) => !m.is_paid && m.spec.source?.kind !== "static",
  ).length;

  function runPromptTurn(text: string) {
    if (!agentId || building) return;
    const ch = `dash-${agentId}-${Date.now()}`;
    setChannel(ch);
    void runTurn({
      agentId, channel: ch, prompt: text,
      model: null, provider: null, folder: folder ?? null, sessionId: "",
    })
      .catch((e) => setNote(String(e)))
      .finally(() => { setChannel(null); load(); });
  }

  // The context every action button gets. Built once so a card can't reach
  // anything the dashboard itself can't do.
  const actionCtx: ActionCtx = useMemo(() => ({
    agentId: agentId ?? "",
    runPrompt: (p) => runPromptTurn(p),
    refreshModule: (moduleId) => {
      void invoke("dashboard_refresh_module", { agentId, moduleId })
        .then(load).catch((e) => setNote(String(e)));
    },
    navigate: (screen) => onNavigate?.(screen),
    setVar: (name, value) => setFormValues((v) => ({ ...v, [name]: String(value) })),
    notify: (msg) => setNote(msg),
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [agentId, folder, building, onNavigate, load]);

  async function submitPrompt() {
    const p = prompt.trim();
    if (!p || !agentId || building) return;
    setNote(null); setPrompt("");
    runPromptTurn(
      `[Dashboard builder] The user is on their dashboard and wants to change it. ` +
      `Use the dashboard_* tools to make it happen, then reply in ONE short sentence ` +
      `describing what you built. Request: ${p}`,
    );
  }

  async function refreshAll() {
    if (!agentId) return;
    setRefreshing(true);
    try {
      const r = await invoke<{ refreshed: number; failed: number; skipped_paid: number }>(
        "dashboard_refresh", { agentId },
      );
      load();
      const bits = [`${r.refreshed} refreshed`];
      if (r.failed) bits.push(`${r.failed} failed`);
      if (r.skipped_paid) bits.push(`${r.skipped_paid} model module${r.skipped_paid === 1 ? "" : "s"} skipped (costs money)`);
      setNote(bits.join(" · "));
    } catch (e) { setNote(String(e)); }
    finally { setRefreshing(false); }
  }

  async function applyPreset(id: string) {
    const p = PRESETS.find((x) => x.id === id);
    if (!p || !agentId) return;
    setShowPresets(false);
    try {
      for (const m of p.modules) {
        await invoke("dashboard_upsert_module", { agentId, module: m });
      }
      load();
      setNote(`Applied “${p.name}”. Nothing has run yet — hit Refresh when you're ready.`);
    } catch (e) { setNote(String(e)); }
  }

  // Geometry the grid actually renders. During a drag this is the live preview
  // (nothing persisted yet); otherwise it's what's in the database.
  const placed: Placed[] = useMemo(
    () => modules.map((m) => ({ id: m.id, ...m.spec.layout })),
    [modules],
  );

  const commitMoves = useCallback((moves: Placed[]) => {
    if (!agentId) return;
    // ONE arrange call for the whole gesture => one undo step. Per-frame writes
    // would bury the revision log and make Undo useless.
    void invoke("dashboard_arrange", {
      agentId,
      moves: moves.map((m) => ({ id: m.id, layout: { x: m.x, y: m.y, w: m.w, h: m.h } })),
    }).then(load).catch((e) => setNote(String(e)));
  }, [agentId, load]);

  const drag = useGridDrag(placed, commitMoves);
  const geometry = new Map((drag.preview ?? placed).map((p) => [p.id, p]));

  if (!agentId) return <Centered>Pick an agent to see its dashboard.</Centered>;

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, flex: 1, minHeight: 0 }}>
      {/* ---- header ---- */}
      <div style={{ display: "flex", alignItems: "flex-end", gap: 12, flexWrap: "wrap" }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 26, fontWeight: 700, letterSpacing: "-0.01em" }}>
            {view?.title ?? "Dashboard"}
          </div>
          <div style={{ fontSize: 13, color: "var(--text-muted)", marginTop: 2 }}>
            {agentName ? `${agentName} · ` : ""}
            {modules.length} {modules.length === 1 ? "module" : "modules"}
          </div>
        </div>

        <Button variant="secondary" onClick={() => setShowPresets((s) => !s)}>Presets</Button>

        <Button variant="secondary" disabled={refreshing || freeFetchable === 0} onClick={() => void refreshAll()}>
          <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
            <Icon name="refresh" size={15} />
            {refreshing ? "Refreshing…" : "Refresh"}
          </span>
        </Button>

        {/* The paid path is ALWAYS separate and self-describing. */}
        {paidCount > 0 && (
          <Button
            variant="secondary"
            onClick={() => setNote(`Refresh each model module from its own ⟳ — that way you only pay for the one you want.`)}
          >
            {paidCount} model {paidCount === 1 ? "module" : "modules"}
          </Button>
        )}
      </div>

      {showPresets && (
        <div style={{
          display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))", gap: 12,
          padding: 16, border: "var(--border-width) solid var(--line)",
          borderRadius: "var(--radius-card)", background: "var(--surface)", boxShadow: "var(--elevation)",
        }}>
          {PRESETS.map((p) => (
            <button
              key={p.id}
              onClick={() => void applyPreset(p.id)}
              style={{
                textAlign: "left", padding: 14, cursor: "pointer",
                border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
                background: "var(--bg)", color: "var(--text)",
              }}
            >
              <div style={{ fontWeight: 700, fontSize: 14, marginBottom: 4 }}>{p.name}</div>
              <div style={{ fontSize: 12, color: "var(--text-muted)", lineHeight: 1.45 }}>{p.blurb}</div>
              <div style={{ fontSize: 11, color: "var(--text-faint)", marginTop: 8 }}>
                {p.modules.length} modules · $0 to apply
              </div>
            </button>
          ))}
        </div>
      )}

      {/* ---- prompt bar: the builder surface ---- */}
      <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
        <Input
          value={prompt}
          disabled={building}
          onChange={(e) => setPrompt((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === "Enter") void submitPrompt(); }}
          placeholder={building ? "Building…" : "Describe a module — “show my schedules as a list”"}
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

      {/* ---- live build feedback ---- */}
      {building && (
        <div style={{
          display: "flex", flexDirection: "column", gap: 6, padding: "12px 14px",
          border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
          background: "var(--surface)", boxShadow: "var(--elevation)",
        }}>
          {turn.liveTools.length === 0 && <span style={{ fontSize: 13, color: "var(--text-muted)" }}>Thinking…</span>}
          {turn.liveTools.map((t: ToolCard, i: number) => (
            <div key={i} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
              <span style={{ color: t.running ? "var(--text-faint)" : t.ok === false ? "var(--danger)" : "var(--ok)" }}>
                {t.running ? "○" : t.ok === false ? "✕" : "✓"}
              </span>
              <span style={{ color: "var(--text-muted)" }}>{t.summary || t.name}</span>
            </div>
          ))}
          {turn.liveText && <div style={{ fontSize: 13, marginTop: 4 }}>{turn.liveText}</div>}
        </div>
      )}

      {note && (
        <div style={{
          fontSize: 13, color: "var(--text-muted)", padding: "10px 14px",
          border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
          display: "flex", alignItems: "center", gap: 10,
        }}>
          <span style={{ flex: 1 }}>{note}</span>
          <button onClick={() => setNote(null)} style={{ background: "none", border: "none", cursor: "pointer", color: "var(--text-faint)", display: "flex" }}>
            <Icon name="close" size={14} />
          </button>
        </div>
      )}

      {/* ---- the grid ---- */}
      {modules.length === 0 && !building ? (
        <EmptyState onPresets={() => setShowPresets(true)} />
      ) : (
        <div ref={drag.gridRef} style={{
          display: "grid", gridTemplateColumns: `repeat(${GRID_COLS}, 1fr)`,
          gridAutoRows: `${ROW_H}px`, gap: GAP, alignContent: "start", paddingBottom: 24,
        }}>
          {modules.map((row) => (
            <section key={row.id} style={{
              ...cardStyle,
              // EXPLICIT row placement (not `span`): auto-placement would
              // reflow cards on its own and fight the drag preview.
              gridColumn: `${(geometry.get(row.id)?.x ?? 0) + 1} / span ${geometry.get(row.id)?.w ?? 4}`,
              gridRow: `${(geometry.get(row.id)?.y ?? 0) + 1} / span ${geometry.get(row.id)?.h ?? 4}`,
              position: "relative",
              // Lift the dragged card and kill transitions on it, so it tracks
              // the pointer exactly while its neighbours glide out of the way.
              zIndex: drag.activeId === row.id ? 10 : 1,
              boxShadow: drag.activeId === row.id ? "var(--elevation-hover)" : "var(--elevation)",
              transition: drag.activeId === row.id ? "none" : "box-shadow 150ms ease",
              userSelect: drag.activeId ? "none" : "auto",
            }}>
              <header
                onPointerDown={(e) => {
                  // Drag from the header only — so buttons, links and text
                  // selection inside a module still work normally.
                  const el = e.target as HTMLElement;
                  if (el.closest("button")) return;
                  drag.onPointerDown(e, row.id, "move");
                }}
                style={{ display: "flex", alignItems: "flex-start", gap: 8, cursor: "grab" }}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{
                    fontSize: "var(--text-caption)", letterSpacing: "0.12em", textTransform: "uppercase",
                    fontWeight: 700, color: "var(--text-muted)",
                    overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
                  }}>{row.spec.title}</div>
                  <div style={{ marginTop: 5 }}><ModuleMeta row={row} /></div>
                </div>
                {row.spec.source?.kind !== "static" && (
                  <IconBtn
                    title={row.is_paid ? "Refresh (uses a model call)" : "Refresh this module"}
                    icon="refresh"
                    onClick={() => {
                      void invoke("dashboard_refresh_module", { agentId, moduleId: row.id })
                        .then(load).catch((e) => setNote(String(e)));
                    }}
                  />
                )}
                <IconBtn
                  title="Remove module" icon="close"
                  onClick={() => {
                    void invoke("dashboard_remove_module", { agentId, moduleId: row.id })
                      .then(load).catch((e) => setNote(String(e)));
                  }}
                />
              </header>

              {/* APPROVAL GATE: an exec module shows its literal command and
                  refuses to run until a human okays that exact string. */}
              {row.pending_approval ? (
                <PendingApproval
                  cmd={String((row.spec.source as { cmd?: string })?.cmd ?? "")}
                  onApprove={() => {
                    void invoke("dashboard_approve_exec", { agentId, moduleId: row.id })
                      .then(load).catch((e) => setNote(String(e)));
                  }}
                />
              ) : (
                <ModuleBody
                  row={row}
                  ctx={actionCtx}
                  formValues={formValues}
                  setFormValue={(k, v) => setFormValues((s) => ({ ...s, [k]: v }))}
                />
              )}
              <ResizeGrip onPointerDown={(e) => drag.onPointerDown(e, row.id, "resize")} />
            </section>
          ))}
        </div>
      )}
    </div>
  );
}

function PendingApproval({ cmd, onApprove }: { cmd: string; onApprove: () => void }) {
  return (
    <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", gap: 10, justifyContent: "center" }}>
      <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
        This module runs a shell command. Review it before allowing it to run:
      </div>
      <code style={{
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", fontSize: 12,
        background: "var(--bg)", border: "var(--border-width) solid var(--line)",
        borderRadius: "var(--radius-control)", padding: "8px 10px",
        whiteSpace: "pre-wrap", wordBreak: "break-all", color: "var(--text)",
      }}>{cmd || "(empty)"}</code>
      <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
        <Button onClick={onApprove}>Approve</Button>
        <span style={{ fontSize: 11, color: "var(--text-faint)" }}>
          Editing the command later will ask again.
        </span>
      </div>
    </div>
  );
}

function EmptyState({ onPresets }: { onPresets: () => void }) {
  return (
    <div style={{
      flex: 1, display: "flex", flexDirection: "column", alignItems: "center",
      justifyContent: "center", gap: 14, textAlign: "center", padding: 40,
      border: "var(--border-width) dashed var(--line)", borderRadius: "var(--radius-card)",
      color: "var(--text-muted)", minHeight: 260,
    }}>
      <div style={{ fontSize: 17, fontWeight: 600, color: "var(--text)" }}>No modules yet</div>
      <div style={{ fontSize: 14, maxWidth: 460, lineHeight: 1.5 }}>
        Describe what you want and your agent will build it — try
        {" "}<em>“show my schedules and recent runs”</em>. Or start from a preset.
        Nothing here runs on its own; data only moves when you ask.
      </div>
      <Button variant="secondary" onClick={onPresets}>Browse presets</Button>
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
