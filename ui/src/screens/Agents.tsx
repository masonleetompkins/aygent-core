import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";
import type { AgentProfile } from "../components/AgentSwitcher";
import { Icon, AGENT_ICONS, type IconName } from "../components/Icon";

// Agents management screen (§10.2): list all agent profiles, create/edit/delete,
// and pick each agent's jailed folder + model/provider. The switcher rail is the
// quick-switch; THIS is the full CRUD surface.

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
// Agent icons are SF-Symbol-style glyphs (see Icon.tsx); rendered in the
// accent color (flat, no glow) — no per-agent background color anymore (Mason's call).
const ICONS = AGENT_ICONS;

// Rank a model id most-powerful-first. Higher score = more capable = higher in
// the dropdown. Family tier dominates; version bumps break ties (opus-5 > opus-4-8).
// Provider-agnostic heuristic; unknown ids fall to the bottom but stay listed.
function modelRank(id: string): number {
  const s = id.toLowerCase();
  let base = 0;
  if (s.includes("fable") || s.includes("mythos")) base = 900;      // next-gen top tier
  else if (s.includes("opus")) base = 800;
  else if (s.includes("gpt-5") || s.includes("o3") || s.includes("o1")) base = 780; // OpenAI reasoning/top
  else if (s.includes("sonnet")) base = 700;
  else if (s.includes("gpt-4")) base = 680;
  else if (s.includes("haiku")) base = 500;
  else if (s.includes("mini") || s.includes("small")) base = 400;
  else base = 300;
  // version nudge: pull a trailing version like "-5", "-4-8", "4.6" out of the id.
  const m = s.match(/(\d+)(?:[.-](\d+))?/g);
  let ver = 0;
  if (m) { const last = m[m.length - 1].replace(/[.-]/g, "."); const parts = last.split("."); ver = (parseInt(parts[0] || "0") * 10) + parseInt(parts[1] || "0"); }
  return base + ver;
}

// A short, human label for a model id (family + version), so the dropdown reads
// nicely instead of showing raw ids.
function modelLabel(id: string): string {
  const s = id.toLowerCase();
  const fam =
    s.includes("fable") ? "Fable" : s.includes("mythos") ? "Mythos" :
    s.includes("opus") ? "Opus" : s.includes("sonnet") ? "Sonnet" : s.includes("haiku") ? "Haiku" :
    null;
  return fam ? `${fam} — ${id}` : id;
}

export function Agents({
  activeId,
  onActiveChange,
  onPickFolder,
  pendingFolder,
  onRosterChange,
  onOpenChat,
}: {
  activeId: string | null;
  onActiveChange: (a: AgentProfile) => void;
  onPickFolder: () => void;
  pendingFolder: string | null;
  // Fired whenever the roster changes (create/delete/update) so App can bump the
  // shared refreshKey — this is what makes the AgentRail re-list immediately
  // instead of showing a deleted agent until restart.
  onRosterChange?: () => void;
  // Open a chat window for this agent (activate it + jump to the Chat screen).
  onOpenChat?: (a: AgentProfile) => void;
}) {
  const [agents, setAgents] = useState<AgentProfile[]>([]);
  const [editing, setEditing] = useState<AgentProfile | null>(null);
  const [creating, setCreating] = useState(false);
  // Pointer-drag reorder: the row being dragged (id) — while set, the list
  // live-reorders as the pointer moves so the OTHER rows slide open to reveal
  // the drop slot; release commits the order. HTML5 DnD gave no live gap and
  // dropped onto inner children unreliably, so we drive it by pointer events.
  const [dragId, setDragId] = useState<string | null>(null);
  // DOM node per row, so pointermove can find which slot the pointer is over.
  const rowRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  // Latest order during a drag lives in a ref (pointermove mutates it without
  // waiting for React state), then we setAgents to render + persist.
  const orderRef = useRef<AgentProfile[]>([]);
  useEffect(() => { orderRef.current = agents; }, [agents]);

  // FLIP: when the list order changes, make each row SLIDE from its old spot to
  // its new one instead of jumping (the rows that move aside to open the drop
  // slot). We capture each row's top before paint, then invert+play. The row
  // currently being dragged is skipped (it tracks the pointer / lifts).
  const lastRowTops = useRef<Map<string, number>>(new Map());
  useLayoutEffect(() => {
    const tops = new Map<string, number>();
    rowRefs.current.forEach((el, id) => { tops.set(id, el.getBoundingClientRect().top); });
    tops.forEach((newTop, id) => {
      if (id === dragId) return; // dragged row lifts, not FLIP-slid
      const prev = lastRowTops.current.get(id);
      const el = rowRefs.current.get(id);
      if (prev == null || !el) return;
      const dy = prev - newTop;
      if (Math.abs(dy) < 1) return;
      el.style.transition = "none";
      el.style.transform = `translateY(${dy}px)`;
      requestAnimationFrame(() => {
        el.style.transition = "transform 180ms cubic-bezier(.2,.8,.2,1)";
        el.style.transform = "";
      });
    });
    lastRowTops.current = tops;
  }, [agents, dragId]);

  // Persist the current order to the backend + tell App so the rail re-lists
  // (and FLIP-animates) into the same order.
  async function persistOrder(next: AgentProfile[]) {
    try {
      await invoke("agents_reorder", { ids: next.map((a) => a.id) });
      onRosterChange?.();
    } catch { void refresh(); } // on failure snap back to the stored order
  }

  // Begin a drag on `id` (from the grip handle). We capture the pointer so all
  // subsequent move/up events land here even if the cursor leaves the row.
  function startDrag(id: string, e: React.PointerEvent) {
    e.preventDefault();
    setDragId(id);
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
  }

  // While dragging: find which row the pointer's Y is over and, if it's a
  // different slot than the dragged row currently occupies, splice the dragged
  // row into that slot NOW — so the other rows animate open around it.
  function onDragMove(e: React.PointerEvent) {
    if (!dragId) return;
    const y = e.clientY;
    const cur = orderRef.current;
    const fromIdx = cur.findIndex((a) => a.id === dragId);
    if (fromIdx < 0) return;
    // Which row index is the pointer over? Compare against each row's midpoint.
    let target = fromIdx;
    for (let i = 0; i < cur.length; i++) {
      const el = rowRefs.current.get(cur[i].id);
      if (!el) continue;
      const r = el.getBoundingClientRect();
      const mid = r.top + r.height / 2;
      if (y < mid) { target = i; break; }
      target = i + 1; // past the last midpoint => goes after this row
    }
    // Clamp + convert "insertion index" to a final position accounting for the
    // removal of the dragged item.
    let to = target;
    if (to > fromIdx) to -= 1;
    to = Math.max(0, Math.min(cur.length - 1, to));
    if (to === fromIdx) return;
    const next = [...cur];
    const [moved] = next.splice(fromIdx, 1);
    next.splice(to, 0, moved);
    orderRef.current = next;
    setAgents(next); // live reorder — rows slide to open the slot
  }

  // Release: commit the order that ended up in the ref.
  function endDrag() {
    if (!dragId) return;
    setDragId(null);
    void persistOrder(orderRef.current);
  }

  async function refresh() {
    try {
      const r = await invoke<{ agents: AgentProfile[]; activeId: string }>("agents_list");
      setAgents(r.agents || []);
    } catch { /* ignore */ }
  }
  useEffect(() => { refresh(); }, []);

  async function activate(a: AgentProfile) {
    try {
      const updated = await invoke<AgentProfile | null>("agents_set_active", { id: a.id });
      if (updated) onActiveChange(updated);
    } catch { /* ignore */ }
  }

  async function remove(a: AgentProfile) {
    if (agents.length <= 1) return; // keep at least one
    try {
      await invoke("agents_delete", { id: a.id });
      await refresh();
      onRosterChange?.(); // tell App → AgentRail re-lists, panes prune the deleted agent
    } catch { /* ignore */ }
  }

  if (creating || editing) {
    return (
      <AgentForm
        initial={editing}
        pendingFolder={pendingFolder}
        onPickFolder={onPickFolder}
        onRosterChange={onRosterChange}
        onDone={async (saved) => {
          setCreating(false); setEditing(null);
          await refresh();
          onRosterChange?.(); // create/edit also changes the rail (new chip / renamed / new icon)
          if (saved) onActiveChange(saved);
        }}
        onCancel={() => { setCreating(false); setEditing(null); }}
      />
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "var(--space-4)", maxWidth: 720 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <h2 style={{ margin: 0, fontSize: "var(--text-h1)", fontWeight: "var(--weight-heading)" }}>Agents</h2>
        <Button onClick={() => setCreating(true)}>+ New Agent</Button>
      </div>
      <p style={hint}>
        Each agent is a profile — its own model, folder, and personality. Switching agents
        switches the folder it can touch. All agents run in one app.
      </p>

      {agents.length === 0 && (
        <Card><p style={hint}>No agents yet. Create one to get started.</p></Card>
      )}

      {agents.map((a) => (
        <div
          key={a.id}
          ref={(el) => { if (el) rowRefs.current.set(a.id, el); else rowRefs.current.delete(a.id); }}
          onPointerMove={onDragMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          style={{
            // The dragged row lifts (raised, slightly scaled, above siblings);
            // the others get a smooth slide as the array reorders around them.
            opacity: dragId === a.id ? 0.9 : 1,
            // Only the dragged row gets an inline transform (lift). Non-dragged
            // rows leave transform to the FLIP effect above (don't set it here,
            // or it overrides the slide).
            ...(dragId === a.id ? { transform: "scale(1.02)" } : {}),
            boxShadow: dragId === a.id ? "var(--elevation)" : "none",
            zIndex: dragId === a.id ? 2 : 1,
            position: "relative",
            borderRadius: "var(--radius-card)",
            touchAction: "none",
          }}
        >
          <Card style={{ flexDirection: "row", alignItems: "center", gap: 14 }}>
            {/* drag handle — press + drag it to reorder (pointer-driven) */}
            <div
              title="Drag to reorder"
              onPointerDown={(e) => startDrag(a.id, e)}
              style={{ flexShrink: 0, color: "var(--text-faint)", cursor: dragId === a.id ? "grabbing" : "grab", fontSize: 18, lineHeight: 1, userSelect: "none", padding: "0 4px", touchAction: "none" }}
            >⋮⋮</div>
            <div style={{
              width: 44, height: 44, flexShrink: 0, color: "var(--accent)",
              display: "flex", alignItems: "center", justifyContent: "center",
            }}><Icon name={(a.icon as IconName) || "sparkles"} size={26} /></div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ fontWeight: 700, fontSize: 16 }}>{a.name}</span>
              </div>
              <div style={{ fontSize: 12, color: "var(--text-faint)", fontFamily: "ui-monospace, monospace", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {a.folder_path || "no folder"} · {a.model || "auto"}{a.model_variant ? ` · ${a.model_variant}` : ""} · {a.provider || "anthropic"} · {a.context_mode}
              </div>
            </div>
            <div style={{ display: "flex", gap: 8, flexShrink: 0 }}>
              <Button onClick={() => (onOpenChat ? onOpenChat(a) : activate(a))}>Open</Button>
              <Button variant="secondary" onClick={() => setEditing(a)}>Edit</Button>
              {agents.length > 1 && <Button variant="secondary" onClick={() => remove(a)}>Delete</Button>}
            </div>
          </Card>
        </div>
      ))}
    </div>
  );
}

// Exported so ONBOARDING reuses the EXACT same agent-creation form (Generate
// Soul + context files + provider/model + Pro Mode) — one source of truth, no
// slimmed duplicate that drifts out of 1:1 parity with the in-app screen.
export function AgentForm({
  initial, pendingFolder, onPickFolder, onDone, onCancel, onRosterChange,
}: {
  initial: AgentProfile | null;
  pendingFolder: string | null;
  // Optional `name` arg so onboarding can DERIVE the agent home (<root>/<Name>/)
  // from the current name. The in-app native picker ignores the arg.
  onPickFolder: (name?: string) => void;
  onDone: (saved: AgentProfile | null) => void;
  onCancel: () => void;
  // Fired when ensureSaved() auto-creates a draft agent so the rail shows the
  // new chip immediately (threaded from the parent Agents component).
  onRosterChange?: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [icon, setIcon] = useState(initial?.icon ?? "sparkles");
  const [color, setColor] = useState(initial?.color ?? "#5b8cff");
  const [folder, setFolder] = useState(initial?.folder_path ?? "");
  const [model, setModel] = useState(initial?.model ?? "");
  const [variant, setVariant] = useState(initial?.model_variant ?? "");
  const [provider, setProvider] = useState(initial?.provider ?? "anthropic");
  const [contextMode, setContextMode] = useState(initial?.context_mode ?? "isolated");
  const [systemPrompt, setSystemPrompt] = useState(initial?.system_prompt ?? "");
  const [saving, setSaving] = useState(false);

  // M1.4: agents that SHARE this folder (share save-point history + write-lock).
  const [sharedWith, setSharedWith] = useState<AgentProfile[]>([]);
  useEffect(() => {
    if (!folder) { setSharedWith([]); return; }
    invoke<AgentProfile[]>("agents_sharing_folder", { folderPath: folder, agentId: initial?.id ?? "" })
      .then((a) => setSharedWith(a || [])).catch(() => setSharedWith([]));
  }, [folder, initial?.id]);

  // The agent's live id. Starts from `initial` (edit) or null (create). When a
  // create-form action needs a saved agent (Generate Soul / attach a document),
  // ensureSaved() auto-creates the draft ONCE and stashes the id here, so those
  // features work in a brand-new agent without a manual Save first (bug: they
  // were dead in the create form because they gated on initial?.id).
  const [savedId, setSavedId] = useState<string | null>(initial?.id ?? null);
  const savingDraftRef = useRef<Promise<string | null> | null>(null);

  // SHARED CONTEXT: read-only mounts of OTHER agents' folders. This is how a
  // second agent (different model) can read everything the first one knows
  // about a project without sharing a home — separate memory/, separate daily
  // notes, no write collisions, and an honest A/B comparison between models.
  type MountRow = { id: number; path: string; label: string; source_agent_id: string | null; ok: boolean };
  const [mounts, setMounts] = useState<MountRow[]>([]);
  const [allAgents, setAllAgents] = useState<AgentProfile[]>([]);
  const [mountBusy, setMountBusy] = useState(false);
  const [mountMsg, setMountMsg] = useState("");

  async function loadMounts() {
    const id = savedId;
    if (!id) { setMounts([]); return; }
    try { setMounts((await invoke<MountRow[]>("agent_mounts_list", { agentId: id })) || []); }
    catch { setMounts([]); }
  }
  useEffect(() => { void loadMounts(); /* eslint-disable-next-line */ }, [savedId]);
  useEffect(() => {
    invoke<{ agents: AgentProfile[]; activeId: string }>("agents_list")
      .then((r) => setAllAgents(r.agents || [])).catch(() => setAllAgents([]));
  }, []);

  async function addAgentMount(sourceId: string) {
    const id = await ensureSaved();
    if (!id) { setMountMsg("Save the agent first."); return; }
    const src = allAgents.find((a) => a.id === sourceId);
    if (!src) return;
    setMountBusy(true); setMountMsg("");
    try {
      await invoke("agent_mount_add", {
        agentId: id, path: src.folder_path, label: src.name, sourceAgentId: src.id,
      });
      await loadMounts();
    } catch (e) { setMountMsg("✗ " + String(e)); }
    finally { setMountBusy(false); }
  }

  async function removeMount(mountId: number) {
    setMountBusy(true);
    try { await invoke("agent_mount_remove", { id: mountId }); await loadMounts(); }
    catch (e) { setMountMsg("✗ " + String(e)); }
    finally { setMountBusy(false); }
  }

  // M1.4 #4: per-agent context documents (uploaded reference files).
  const [ctxDocs, setCtxDocs] = useState<Array<{ id: number; filename: string; bytes: number; char_count: number }>>([]);
  const [ctxBusy, setCtxBusy] = useState(false);
  async function loadCtx() {
    const id = savedId;
    if (!id) { setCtxDocs([]); return; }
    try { setCtxDocs(await invoke("agent_context_list", { agentId: id }) || []); } catch { setCtxDocs([]); }
  }
  useEffect(() => { void loadCtx(); /* eslint-disable-next-line */ }, [savedId]);

  // Ensure a draft agent exists (create-form actions need an id). Idempotent:
  // returns the existing id if already saved, otherwise creates once (guarded by
  // a ref so rapid double-clicks don't create two). Requires a name.
  async function ensureSaved(): Promise<string | null> {
    if (savedId) return savedId;
    if (!name.trim()) return null;
    if (savingDraftRef.current) return savingDraftRef.current;
    const p = (async () => {
      try {
        const created = await invoke<AgentProfile>("agents_create", {
          name: name.trim(), icon, color, folderPath: folder, model, provider,
          modelVariant: variant, contextMode, systemPrompt,
        });
        setSavedId(created.id);
        onRosterChange?.(); // new chip appears in the rail immediately
        return created.id;
      } catch { return null; }
      finally { savingDraftRef.current = null; }
    })();
    savingDraftRef.current = p;
    return p;
  }
  async function uploadCtx(file: File) {
    const id = await ensureSaved();
    if (!id) return;
    setCtxBusy(true);
    try {
      const buf = new Uint8Array(await file.arrayBuffer());
      let bin = ""; for (let i = 0; i < buf.length; i++) bin += String.fromCharCode(buf[i]);
      const b64 = btoa(bin);
      await invoke("agent_context_add", { agentId: id, filename: file.name, bytesB64: b64 });
      await loadCtx();
    } catch { /* surfaced via reload */ }
    finally { setCtxBusy(false); }
  }
  async function removeCtx(id: number) {
    if (!initial?.id) return;
    try { await invoke("agent_context_remove", { agentId: initial.id, id }); await loadCtx(); } catch { /* ignore */ }
  }

  // IMPORT MEMORY (2026-07-31): one-click port of an existing memory bundle
  // (a CleoPort-style vault or any Obsidian folder) INTO this agent's folder,
  // then auto-ingest so retrieval + graph expansion light up. The AYGENT-native
  // "bring my memory / restore me" flow — no terminal. Needs a saved agent (for
  // the id) + a folder (the destination jail).
  const [importBusy, setImportBusy] = useState(false);
  const [importMsg, setImportMsg] = useState<string | null>(null);
  async function importMemory() {
    const id = await ensureSaved();
    if (!id) { setImportMsg("Give the agent a name first."); return; }
    if (!folder) { setImportMsg("Set the agent's folder first."); return; }
    setImportBusy(true); setImportMsg(null);
    try {
      const r = await invoke<{
        cancelled?: boolean; files_copied?: number;
        ingest?: { parsed: number; embedded: number; links: number };
      }>("import_memory", { agentId: id, agentFolder: folder });
      if (r.cancelled) { setImportMsg(null); }
      else {
        const ing = r.ingest;
        setImportMsg(
          `Imported ${r.files_copied ?? 0} files — ${ing?.parsed ?? 0} notes, ` +
          `${ing?.embedded ?? 0} embedded, ${ing?.links ?? 0} links. Memory is live.`
        );
      }
    } catch (e) { setImportMsg(`Import failed: ${String(e)}`); }
    finally { setImportBusy(false); }
  }

  // PRO MODE (2026-07-31): shell.exec consent for THIS agent's folder. Scary-
  // honest — flips the folder from zero-shell Folder Mode to "can run programs."
  // Backed by pro_mode_get/set (writes a per-folder flag; the Rust exec broker
  // cap-gates authoritatively). Keyed on folder, not agent id, matching the jail.
  const [proMode, setProMode] = useState(false);
  const [proBusy, setProBusy] = useState(false);
  useEffect(() => {
    if (!folder) { setProMode(false); return; }
    invoke<boolean>("pro_mode_get", { folder }).then(setProMode).catch(() => setProMode(false));
  }, [folder]);
  async function toggleProMode(next: boolean) {
    if (!folder) return;
    if (next) {
      const ok = window.confirm(
        "Enable Pro Mode for this agent?\n\n" +
        "This agent will be able to RUN PROGRAMS on your Mac — including build tools, " +
        "git, and anything on your PATH — rooted in this folder. Commands run from the " +
        "folder and cannot leave it, secrets are never shared with them, and you can kill " +
        "any process at any time. But this relaxes the zero-shell guarantee.\n\n" +
        "Only enable this for an agent you're using to build or run code."
      );
      if (!ok) return;
    }
    setProBusy(true);
    try {
      const saved = await invoke<boolean>("pro_mode_set", { folder, enabled: next });
      setProMode(saved);
    } catch { /* ignore */ }
    finally { setProBusy(false); }
  }

  // M1.4 #5: generate a Soul.md (agent authors its own personality/values).
  const [soulBrief, setSoulBrief] = useState("");
  const [soulBusy, setSoulBusy] = useState(false);
  const [soulErr, setSoulErr] = useState<string | null>(null);
  async function generateSoul() {
    const id = await ensureSaved();
    if (!id) { setSoulErr("Give the agent a name first."); return; }
    setSoulBusy(true); setSoulErr(null);
    try {
      const soul = await invoke<string>("agent_generate_soul", { agentId: id, brief: soulBrief });
      setSystemPrompt(soul); // drop it into the editable field so the user can tweak before Save
    } catch (e) { setSoulErr(String(e)); }
    finally { setSoulBusy(false); }
  }

  // Live model list for the chosen provider, most-powerful-first.
  const [models, setModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsErr, setModelsErr] = useState<string | null>(null);

  // Locally-downloaded GGUF models (for provider === "local"): show a dropdown
  // of what's already on disk instead of forcing the user to type a path.
  const [localModels, setLocalModels] = useState<Array<{ filename: string; path: string; size_gb: number }>>([]);
  const [localLoading, setLocalLoading] = useState(false);
  async function loadLocalModels(): Promise<void> {
    setLocalLoading(true);
    try {
      const list = await invoke<Array<{ filename: string; path: string; size_gb: number }>>("local_downloaded");
      setLocalModels(list || []);
    } catch { setLocalModels([]); }
    finally { setLocalLoading(false); }
  }

  // When the user picks a folder via the native picker, adopt it — whether
  // CREATING or EDITING an agent. The old `!initial` guard meant edits never
  // picked up the new path (the form kept showing the stale folder). We only
  // adopt a pick that ARRIVES while this form is open (pickSeq), so re-mounting
  // an edit form doesn't overwrite the agent's saved folder with a stale
  // app-wide value.
  const mountedRef = useRef(false);
  useEffect(() => {
    // Skip the very first run (initial mount) so we don't clobber initial.folder_path.
    if (!mountedRef.current) { mountedRef.current = true; return; }
    if (pendingFolder) setFolder(pendingFolder);
  }, [pendingFolder]);

  // Fetch this provider's models. Anthropic uses its own command; openai/
  // openrouter share one. Local models are file paths (handled elsewhere) so we
  // skip the live fetch. RESILIENT: the create form can mount before the daemon/
  // keychain handshake settles, so the first call may throw "no key" spuriously.
  // We retry a few times with backoff, and also expose a manual refetch that the
  // dropdown fires on focus — so it can never come up permanently empty.
  async function loadModels(prov: string): Promise<void> {
    if (prov === "local" || prov === "mlx") { setModels([]); setModelsErr(null); return; }
    setModelsLoading(true); setModelsErr(null);
    const attempt = () =>
      prov === "anthropic"
        ? invoke<string[]>("anthropic_models")
        : invoke<string[]>("openai_models", { provider: prov });
    let lastErr: unknown = null;
    // Cold-mount: the daemon/keychain handshake can lag a beat, so the first few
    // calls may throw spuriously. Retry with a longer, wider backoff before we
    // surface an error (this was failing on initial Anthropic load).
    for (let i = 0; i < 6; i++) {
      try {
        const list = await attempt();
        const sorted = [...(list || [])].sort((a, b) => modelRank(b) - modelRank(a));
        setModels(sorted);
        setModelsErr(null);
        setModelsLoading(false);
        return;
      } catch (e) {
        lastErr = e;
        await new Promise((r) => setTimeout(r, 400 * (i + 1)));
      }
    }
    setModels([]);
    setModelsErr(String(lastErr));
    setModelsLoading(false);
  }

  useEffect(() => {
    if (provider === "local") void loadLocalModels();
    else if (provider !== "mlx") void loadModels(provider);
    else { setModels([]); setModelsErr(null); }
    /* eslint-disable-next-line */
  }, [provider]);

  async function save() {
    if (!name.trim()) return;
    setSaving(true);
    try {
      if (initial) {
        const updated: AgentProfile = {
          ...initial, name: name.trim(), icon, color,
          folder_path: folder, model, provider, model_variant: variant, context_mode: contextMode, system_prompt: systemPrompt,
        };
        await invoke("agents_update", { profile: updated });
        onDone(updated);
      } else {
        const created = await invoke<AgentProfile>("agents_create", {
          name: name.trim(), icon, color, folderPath: folder, model, provider,
          modelVariant: variant, contextMode, systemPrompt,
        });
        onDone(created);
      }
    } catch { onDone(null); }
    finally { setSaving(false); }
  }

  return (
    <div style={{ maxWidth: 640 }}>
      <Card title={initial ? `Edit ${initial.name}` : "New agent"}>
        <label style={fieldLabel}>Name
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Work, Journal, Research…" />
        </label>

        <label style={{ ...fieldLabel, flex: 0 }}>Icon
          <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Shown in your accent color (set in Settings → Appearance).</span>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginTop: 4 }}>
            {ICONS.map((i) => {
              const sel = icon === i;
              return (
                <button key={i} onClick={() => setIcon(i)} title={i} style={{
                  width: 40, height: 40, borderRadius: "var(--radius-control)", cursor: "pointer",
                  display: "flex", alignItems: "center", justifyContent: "center",
                  color: sel ? "var(--accent)" : "var(--text-muted)",
                  border: sel ? "var(--border-width) solid var(--accent)" : "var(--border-width) solid var(--line)",
                  background: sel ? "color-mix(in srgb, var(--accent) 10%, var(--bg))" : "var(--bg)",
                  boxShadow: sel ? "var(--elevation)" : "none",
                  transition: "border-color .12s, box-shadow .12s, color .12s",
                }}><Icon name={i} size={20} /></button>
              );
            })}
          </div>
        </label>

        <label style={fieldLabel}>Agent Folder (its jail)
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Input value={folder} onChange={(e) => setFolder(e.target.value)} mono placeholder="pick a folder…" />
            <Button variant="secondary" onClick={() => onPickFolder(name)}>Pick…</Button>
          </div>
          <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
            This agent can only ever touch files inside this folder.
          </span>
          {sharedWith.length > 0 && (
            <span style={{ fontSize: 12, color: "var(--accent)", background: "color-mix(in srgb, var(--accent) 10%, transparent)", border: "1px solid color-mix(in srgb, var(--accent) 30%, transparent)", borderRadius: "var(--radius-control)", padding: "7px 10px" }}>
              🔗 Shares this folder with {sharedWith.map((a) => a.name).join(", ")} — they share the same edit history &amp; save points, and take turns writing so their changes never collide.
            </span>
          )}
        </label>

        <div style={{ display: "flex", gap: 12 }}>
          <label style={fieldLabel}>Provider
            <select value={provider} onChange={(e) => { setProvider(e.target.value); setModel(""); setVariant(""); }} style={selectStyle}>
              <option value="anthropic">Anthropic</option>
              <option value="openai">OpenAI</option>
              <option value="openrouter">OpenRouter</option>
              <option value="meta">Muse (Meta)</option>
              <option value="local">Local (GGUF)</option>
              <option value="mlx">Local (MLX)</option>
            </select>
          </label>
          <label style={fieldLabel}>Model
            {provider === "mlx" ? (
              <>
                <Input value={model} onChange={(e) => setModel(e.target.value)} mono placeholder="mlx-community/Qwen3-4B-4bit" />
                <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Hugging Face repo id — weights auto-download on first chat (GBs, one-time). Pull ahead in Settings to watch progress.</span>
              </>
            ) : provider === "local" ? (
              localModels.length > 0 ? (
                <select
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  onMouseDown={() => { if (!localLoading) void loadLocalModels(); }}
                  style={selectStyle}
                >
                  <option value="">{localLoading ? "scanning…" : "Select a downloaded model"}</option>
                  {localModels.map((m) => (
                    <option key={m.path} value={m.path}>{m.filename} ({m.size_gb.toFixed(1)} GB)</option>
                  ))}
                  {/* keep a manually-set path visible even if it's not in the folder */}
                  {model && !localModels.some((m) => m.path === model) && <option value={model}>{model}</option>}
                </select>
              ) : (
                <Input value={model} onChange={(e) => setModel(e.target.value)} mono
                  placeholder={localLoading ? "scanning…" : "path to .gguf (none downloaded yet)"} />
              )
            ) : provider === "openrouter" ? (
              // OpenRouter has THOUSANDS of models — a dropdown is unusable. Let
              // the user paste the exact model id (the “author/model” slug shown
              // on openrouter.ai, e.g. anthropic/claude-3.5-sonnet). A datalist
              // offers the fetched list as suggestions without forcing a pick.
              <>
                <Input value={model} onChange={(e) => setModel(e.target.value)} mono
                  list="openrouter-models"
                  placeholder="paste a model id, e.g. anthropic/claude-3.5-sonnet" />
                <datalist id="openrouter-models">
                  {models.map((m) => <option key={m} value={m} />)}
                </datalist>
              </>
            ) : (
              <select
                value={model}
                onChange={(e) => setModel(e.target.value)}
                onMouseDown={() => { if (!modelsLoading && models.length === 0) void loadModels(provider); }}
                style={selectStyle}
              >
                <option value="">{modelsLoading ? "loading models…" : "Auto (recommended)"}</option>
                {models.map((m) => <option key={m} value={m}>{modelLabel(m)}</option>)}
                {/* keep a saved model visible even if the live list didn't return it */}
                {model && !models.includes(model) && <option value={model}>{modelLabel(model)}</option>}
              </select>
            )}
            {modelsErr && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Couldn’t load {provider} models: {modelsErr}. Using “Auto” — click the menu to retry.</span>}
            {!modelsErr && !modelsLoading && provider !== "local" && provider !== "mlx" && models.length > 0 && (
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Most capable first.</span>
            )}
            {provider === "local" && !localLoading && localModels.length === 0 && (
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>No downloaded models found — grab one in Settings, or paste a .gguf path.</span>
            )}
            {provider === "local" && localModels.length > 0 && (
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Downloaded models on this Mac.</span>
            )}
            {provider === "openrouter" && (
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
                Paste the model id from openrouter.ai (the “author/model” slug). Suggestions come from the live list; leave blank for Auto.
              </span>
            )}
          </label>
          {(provider === "meta" || provider === "openai" || provider === "openrouter") && (
            <label style={fieldLabel}>Variant
              <select value={variant} onChange={(e) => setVariant(e.target.value)} style={selectStyle}>
                <option value="">Auto (provider default)</option>
                {["minimal", "low", "medium", "high", "xhigh"].map((v) => <option key={v} value={v}>{v}</option>)}
                {provider === "meta" && <option value="max">max</option>}
              </select>
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
                {provider === "meta"
                  ? "Reasoning effort for Muse Spark — same model id, more or less thinking. Higher = smarter + slower + pricier."
                  : "Reasoning-effort passthrough — only some models honor it; others error honestly."}
              </span>
            </label>
          )}
        </div>

        {/* CONTEXT MODE (2026-08-03): how much conversation context this agent
           carries between chats. Wired end-to-end now (was a dead column). */}
        <label style={fieldLabel}>Context mode
          <select value={contextMode} onChange={(e) => setContextMode(e.target.value)} style={selectStyle}>
            <option value="isolated">Isolated — each chat starts fresh (recommended)</option>
            <option value="continuous">Continuous — new chats carry context forward</option>
          </select>
          <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
            {contextMode === "continuous"
              ? "A new chat inherits the running context of the chat you started it from — the agent picks up mid-thought without re-explaining. Tradeoff: that carried history rides into every message, so each turn costs more tokens and cost grows with the thread. Best for one ongoing project where continuity matters more than price."
              : "Each chat is its own session: nothing from other chats is sent with it, so tokens stay cheap and context stays clean. The agent still keeps long-term memory (its Memory notes) across all chats. The right default for most use."}
          </span>
        </label>

        {/* PRO MODE consent (scary-honest). Only meaningful once a folder is set. */}
        <div style={{
          display: "flex", flexDirection: "column", gap: 8,
          background: proMode ? "color-mix(in srgb, var(--danger, #ef4444) 8%, var(--surface))" : "var(--surface)",
          border: `var(--border-width) solid ${proMode ? "color-mix(in srgb, var(--danger, #ef4444) 40%, transparent)" : "var(--line)"}`,
          borderRadius: "var(--radius-control)", padding: "12px 14px",
        }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
            <div style={{ fontSize: 13, fontWeight: 600 }}>⚡ Pro Mode — run shell commands</div>
            <label style={{ display: "inline-flex", alignItems: "center", gap: 8, cursor: folder ? "pointer" : "not-allowed", opacity: folder ? 1 : 0.5 }}>
              <input type="checkbox" checked={proMode} disabled={!folder || proBusy}
                onChange={(e) => toggleProMode(e.target.checked)} />
              <span style={{ fontSize: 13 }}>{proMode ? "Enabled" : "Off"}</span>
            </label>
          </div>
          <span style={{ ...hint, fontSize: 12 }}>
            Lets this agent run programs on your Mac (build tools, git, anything on your PATH),
            rooted in this folder. Commands can’t leave the folder and your API keys are never
            shared with them — but this relaxes the zero-shell guarantee. Use it for an agent that
            builds or runs code (e.g. an “AYGENT Dev” agent).
          </span>
          {!folder && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Pick a folder first.</span>}
        </div>

        <label style={fieldLabel}>Custom instructions / Soul
          <textarea value={systemPrompt} onChange={(e) => setSystemPrompt(e.target.value)}
            rows={5} placeholder="This agent's personality, values & instructions… or generate a Soul below."
            style={{
              background: "var(--bg)", border: "var(--border-width) solid var(--line)",
              borderRadius: "var(--radius-control)", color: "var(--text)", padding: "9px 12px",
              fontSize: 14, resize: "vertical", fontFamily: "inherit",
            }} />
        </label>

        {/* M1.4 #5: Generate a Soul.md — the agent authors its own personality. */}
        <div style={{ display: "flex", flexDirection: "column", gap: 8, background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", padding: "12px 14px" }}>
          <div style={{ fontSize: 13, fontWeight: 600 }}>✨ Generate a Soul</div>
          <span style={{ ...hint, fontSize: 12 }}>Let the agent write its own personality &amp; values. It fills the field above — you can edit before saving.</span>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Input value={soulBrief} onChange={(e) => setSoulBrief(e.target.value)}
              placeholder="optional vibe: 'warm, rigorous research partner'…" />
            <Button variant="secondary" onClick={generateSoul} disabled={soulBusy || !initial?.id}>
              {soulBusy ? "Writing…" : "Generate Soul"}
            </Button>
          </div>
          {!initial?.id && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Save the agent first, then generate its soul.</span>}
          {soulErr && <span style={{ ...hint, fontSize: 12, color: "var(--danger)" }}>{soulErr}</span>}
        </div>

        {/* M1.4 #4: Per-agent context documents. */}
        <div style={{ display: "flex", flexDirection: "column", gap: 8, background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", padding: "12px 14px" }}>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Context documents</div>
          <span style={{ ...hint, fontSize: 12 }}>Reference files this agent always has in mind (text, markdown, code, JSON…). Stored privately — never inside your folder.</span>
          {ctxDocs.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              {ctxDocs.map((d) => (
                <div key={d.id} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
                  <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{d.filename}</span>
                  <span style={{ ...hint, fontSize: 11, color: "var(--text-faint)" }}>{d.char_count > 0 ? `${d.char_count.toLocaleString()} chars` : "stored (not readable)"}</span>
                  <button onClick={() => removeCtx(d.id)} title="Remove" style={{ background: "none", border: "none", cursor: "pointer", color: "var(--danger)", fontSize: 13 }}>✕</button>
                </div>
              ))}
            </div>
          )}
          <label style={{ display: "inline-flex", alignItems: "center", gap: 8, cursor: initial?.id ? "pointer" : "not-allowed", opacity: initial?.id ? 1 : 0.5 }}>
            <input type="file" style={{ display: "none" }} disabled={!initial?.id || ctxBusy}
              onChange={(e) => { const f = e.target.files?.[0]; if (f) void uploadCtx(f); e.currentTarget.value = ""; }} />
            <span style={{ fontSize: 13, padding: "6px 12px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--bg)" }}>
              {ctxBusy ? "Uploading…" : "+ Upload document"}
            </span>
          </label>
          {!initial?.id && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Save the agent first, then attach documents.</span>}
        </div>

        {/* SHARED CONTEXT — read-only mounts of other agents' folders. */}
        <div style={{ display: "flex", flexDirection: "column", gap: 8, background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", padding: "12px 14px" }}>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Shared context</div>
          <span style={{ ...hint, fontSize: 12 }}>
            Let this agent <strong>read</strong> another agent’s folder — its memory, notes and project
            files — while keeping its own home. Read-only: it can never write there, so two agents can
            work the same project without overwriting each other’s memory.
          </span>

          {mounts.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              {mounts.map((m) => (
                <div key={m.id} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 13 }}>
                  <span style={{ fontSize: 11, padding: "1px 6px", borderRadius: 4, background: "var(--bg)", border: "var(--border-width) solid var(--line)", color: "var(--text-faint)" }}>read-only</span>
                  <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={m.path}>
                    {m.label || m.path}
                  </span>
                  {!m.ok && <span style={{ fontSize: 11, color: "var(--danger)" }}>folder missing</span>}
                  <button onClick={() => removeMount(m.id)} title="Remove" disabled={mountBusy}
                    style={{ background: "none", border: "none", cursor: "pointer", color: "var(--danger)", fontSize: 13 }}>✕</button>
                </div>
              ))}
            </div>
          )}

          {(() => {
            const mine = new Set(mounts.map((m) => m.source_agent_id).filter(Boolean));
            const available = allAgents.filter(
              (a) => !a.archived && a.folder_path && a.id !== savedId && !mine.has(a.id),
            );
            if (available.length === 0) {
              return (
                <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
                  {allAgents.length <= 1
                    ? "Create a second agent to share context with."
                    : "All other agents are already mounted."}
                </span>
              );
            }
            return (
              <select
                value=""
                disabled={mountBusy}
                onChange={(e) => { const v = e.target.value; if (v) void addAgentMount(v); e.currentTarget.value = ""; }}
                style={selectStyle}
              >
                <option value="">+ Give this agent read access to…</option>
                {available.map((a) => (
                  <option key={a.id} value={a.id}>{a.name}</option>
                ))}
              </select>
            );
          })()}
          {mountMsg && <span style={{ fontSize: 12, color: "var(--danger)" }}>{mountMsg}</span>}
        </div>

        {/* IMPORT MEMORY — bring an existing memory vault into this agent + ingest. */}
        <div style={{ display: "flex", flexDirection: "column", gap: 8, background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", padding: "12px 14px" }}>
          <div style={{ fontSize: 13, fontWeight: 600 }}>Import memory</div>
          <span style={{ ...hint, fontSize: 12 }}>
            Bring an existing memory vault (Memory/ + Daily/ notes) into this agent’s folder and
            index it. Use this to port an agent to a new machine, or give a fresh agent a past.
            Additive — it never deletes existing notes.
          </span>
          <div>
            <Button variant="secondary" onClick={importMemory} disabled={importBusy || !folder}>
              {importBusy ? "Importing & indexing…" : "📥 Import memory folder…"}
            </Button>
          </div>
          {importMsg && <span style={{ ...hint, fontSize: 12, color: importMsg.startsWith("Import failed") ? "var(--danger)" : "var(--accent)" }}>{importMsg}</span>}
          {!folder && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Set the agent’s folder first.</span>}
        </div>

        {/* TELEGRAM — one bot per agent (Fix 7). Token in Keychain, not in DB/files. */}
        <TelegramCard agentId={savedId ?? initial?.id ?? null} initial={initial} />

        <div style={{ display: "flex", gap: 8 }}>
          <Button onClick={save} disabled={saving || !name.trim()}>{initial ? "Save" : "Create agent"}</Button>
          <Button variant="secondary" onClick={onCancel}>Cancel</Button>
        </div>
      </Card>
    </div>
  );
}

function TelegramCard({ agentId, initial, onSaved }: { agentId: string | null; initial: AgentProfile | null; onSaved?: () => void }) {
  const [token, setToken] = useState("");
  const [allowed, setAllowed] = useState(initial?.telegram_allowed_chats ?? "");
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [status, setStatus] = useState<{ enabled: boolean; bot_username: string; has_token: boolean; allowed_chats: string } | null>(null);
  useEffect(() => {
    if (!agentId) { setStatus(null); return; }
    invoke<any>("telegram_status", { agentId }).then(setStatus).catch(() => setStatus(null));
    if (initial) setAllowed(initial.telegram_allowed_chats || "");
  }, [agentId]);
  // also sync allowed when initial changes
  useEffect(() => { if (initial) setAllowed(initial.telegram_allowed_chats || ""); }, [initial?.telegram_allowed_chats]);
  async function saveToken() {
    if (!agentId) { setMsg("Save the agent first."); return; }
    if (!token.trim()) { setMsg("Paste a Bot token from @BotFather first."); return; }
    setSaving(true); setMsg(null);
    try {
      await invoke("telegram_set_token", { agentId, token: token.trim() });
      const v = await invoke<any>("telegram_test_token", { token: token.trim() });
      const username = v?.bot_username || "";
      // persist wiring: enabled + username + allowlist
      const profile = { ...(initial as any), id: agentId, telegram_enabled: true, telegram_bot_username: username, telegram_allowed_chats: allowed };
      await invoke("agents_update", { profile: { ...profile, name: profile.name || "Agent" } });
      setToken(""); setMsg(`✓ Connected as @${username}. Message it on Telegram to chat with this agent.`);
      const st = await invoke<any>("telegram_status", { agentId }); setStatus(st);
      onSaved?.();
    } catch (e) { setMsg("✗ " + String(e)); }
    finally { setSaving(false); }
  }
  async function toggleEnabled(on: boolean) {
    if (!agentId || !initial) return;
    try {
      const profile = { ...initial, id: agentId, telegram_enabled: on } as any;
      await invoke("agents_update", { profile });
      setStatus((s) => s ? { ...s, enabled: on } : s);
    } catch (e) { setMsg(String(e)); }
  }
  async function saveAllowlist() {
    if (!agentId || !initial) { setMsg("Save the agent first."); return; }
    try {
      const profile = { ...initial, id: agentId, telegram_allowed_chats: allowed } as any;
      await invoke("agents_update", { profile });
      setMsg("✓ Telegram allowlist saved.");
    } catch (e) { setMsg(String(e)); }
  }
  async function clearToken() {
    if (!agentId) return;
    try { await invoke("telegram_set_token", { agentId, token: "" }); setStatus((s) => s ? { ...s, has_token: false, enabled: false, bot_username: "" } : s); setMsg("Telegram disconnected."); } catch (e) { setMsg(String(e)); }
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, background: "var(--surface)", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", padding: "12px 14px" }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
        <div style={{ fontSize: 13, fontWeight: 600 }}>Telegram — message this agent</div>
        {status?.has_token && (
          <label style={{ display: "inline-flex", alignItems: "center", gap: 8, cursor: "pointer" }}>
            <input type="checkbox" checked={!!status?.enabled} onChange={(e) => toggleEnabled(e.target.checked)} />
            <span style={{ fontSize: 12 }}>{status?.enabled ? "Enabled" : "Paused"}</span>
          </label>
        )}
      </div>
      <span style={{ ...hint, fontSize: 12 }}>
        One bot per agent. Create a bot with <b>@BotFather</b> on Telegram (send <code>/newbot</code>), paste its token here, and this agent will reply on Telegram. Token is stored in your macOS Keychain — never in files.
      </span>
      {status?.bot_username && <span style={{ fontSize: 12, color: "var(--ok)" }}>@{status.bot_username} {status.has_token ? "· token saved ✓" : ""}</span>}
      <div style={{ display: "flex", gap: 8 }}>
        <Input type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder={status?.has_token ? "token saved — paste to replace" : "123456789:AAH... (from @BotFather)"} />
        <Button onClick={saveToken} disabled={saving || !agentId}>{saving ? "Saving…" : status?.has_token ? "Update" : "Connect"}</Button>
      </div>
      {status?.has_token && <Button variant="secondary" onClick={clearToken}>Disconnect Telegram</Button>}
      <label style={fieldLabel as any}>Allowed chat IDs (optional)
        <Input value={allowed} onChange={(e) => setAllowed(e.target.value)} placeholder="e.g. 123456789, 987654321 (leave empty = allow any)" />
        <span style={{ ...hint, fontSize: 11, color: "var(--text-faint)" }}>Comma-separated Telegram chat IDs that may message this bot. Leave empty to allow anyone who knows the bot.</span>
      </label>
      <div style={{ display: "flex", gap: 8 }}>
        <Button variant="secondary" onClick={saveAllowlist}>Save allowlist</Button>
      </div>
      {msg && <span style={{ fontSize: 12, color: msg.startsWith("✓") ? "var(--ok)" : "var(--danger)" }}>{msg}</span>}
      {!agentId && <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Save the agent first, then connect Telegram.</span>}
    </div>
  );
}

const fieldLabel = { display: "flex", flexDirection: "column", gap: 6, fontSize: 13, fontWeight: 600, flex: 1 } as const;
const selectStyle = {
  padding: "9px 12px", borderRadius: "var(--radius-control)",
  border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)",
  width: "100%", maxWidth: "100%", boxSizing: "border-box",
  textOverflow: "ellipsis", overflow: "hidden", whiteSpace: "nowrap",
} as const;
