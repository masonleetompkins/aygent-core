import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";
import type { AgentProfile } from "../components/AgentSwitcher";

// Agents management screen (§10.2): list all agent profiles, create/edit/delete,
// and pick each agent's jailed folder + model/provider. The switcher rail is the
// quick-switch; THIS is the full CRUD surface.

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const ICONS = ["🤖", "🧠", "📓", "🔬", "💼", "🎨", "📈", "🗂️", "⚙️", "🌱"];
const COLORS = ["#5b8cff", "#22c55e", "#f59e0b", "#ec4899", "#8b5cf6", "#14b8a6", "#ef4444", "#64748b"];

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
}: {
  activeId: string | null;
  onActiveChange: (a: AgentProfile) => void;
  onPickFolder: () => void;
  pendingFolder: string | null;
}) {
  const [agents, setAgents] = useState<AgentProfile[]>([]);
  const [editing, setEditing] = useState<AgentProfile | null>(null);
  const [creating, setCreating] = useState(false);

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
    try { await invoke("agents_delete", { id: a.id }); await refresh(); } catch { /* ignore */ }
  }

  if (creating || editing) {
    return (
      <AgentForm
        initial={editing}
        pendingFolder={pendingFolder}
        onPickFolder={onPickFolder}
        onDone={async (saved) => {
          setCreating(false); setEditing(null);
          await refresh();
          if (saved) onActiveChange(saved);
        }}
        onCancel={() => { setCreating(false); setEditing(null); }}
      />
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16, maxWidth: 720 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <h2 style={{ margin: 0, fontSize: 22, fontWeight: 800 }}>Agents</h2>
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
        <Card key={a.id} style={{ flexDirection: "row", alignItems: "center", gap: 14 }}>
          <div style={{
            width: 44, height: 44, borderRadius: 12, background: a.color, color: "#fff",
            display: "flex", alignItems: "center", justifyContent: "center", fontSize: 22, flexShrink: 0,
          }}>{a.icon || "🤖"}</div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <span style={{ fontWeight: 700, fontSize: 16 }}>{a.name}</span>
              {a.id === activeId && <Pill tone="ok">active</Pill>}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-faint)", fontFamily: "ui-monospace, monospace", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {a.folder_path || "no folder"} · {a.model || "auto"} · {a.provider || "anthropic"} · {a.context_mode}
            </div>
          </div>
          <div style={{ display: "flex", gap: 8, flexShrink: 0 }}>
            {a.id !== activeId && <Button variant="secondary" onClick={() => activate(a)}>Switch to</Button>}
            <Button variant="secondary" onClick={() => setEditing(a)}>Edit</Button>
            {agents.length > 1 && <Button variant="secondary" onClick={() => remove(a)}>Delete</Button>}
          </div>
        </Card>
      ))}
    </div>
  );
}

function AgentForm({
  initial, pendingFolder, onPickFolder, onDone, onCancel,
}: {
  initial: AgentProfile | null;
  pendingFolder: string | null;
  onPickFolder: () => void;
  onDone: (saved: AgentProfile | null) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [icon, setIcon] = useState(initial?.icon ?? "🤖");
  const [color, setColor] = useState(initial?.color ?? "#5b8cff");
  const [folder, setFolder] = useState(initial?.folder_path ?? "");
  const [model, setModel] = useState(initial?.model ?? "");
  const [provider, setProvider] = useState(initial?.provider ?? "anthropic");
  const [systemPrompt, setSystemPrompt] = useState(initial?.system_prompt ?? "");
  const [saving, setSaving] = useState(false);

  // M1.4: agents that SHARE this folder (share checkpoint history + write-lock).
  const [sharedWith, setSharedWith] = useState<AgentProfile[]>([]);
  useEffect(() => {
    if (!folder) { setSharedWith([]); return; }
    invoke<AgentProfile[]>("agents_sharing_folder", { folderPath: folder, agentId: initial?.id ?? "" })
      .then((a) => setSharedWith(a || [])).catch(() => setSharedWith([]));
  }, [folder, initial?.id]);

  // M1.4 #4: per-agent context documents (uploaded reference files).
  const [ctxDocs, setCtxDocs] = useState<Array<{ id: number; filename: string; bytes: number; char_count: number }>>([]);
  const [ctxBusy, setCtxBusy] = useState(false);
  async function loadCtx() {
    if (!initial?.id) { setCtxDocs([]); return; }
    try { setCtxDocs(await invoke("agent_context_list", { agentId: initial.id }) || []); } catch { setCtxDocs([]); }
  }
  useEffect(() => { void loadCtx(); /* eslint-disable-next-line */ }, [initial?.id]);
  async function uploadCtx(file: File) {
    if (!initial?.id) return;
    setCtxBusy(true);
    try {
      const buf = new Uint8Array(await file.arrayBuffer());
      let bin = ""; for (let i = 0; i < buf.length; i++) bin += String.fromCharCode(buf[i]);
      const b64 = btoa(bin);
      await invoke("agent_context_add", { agentId: initial.id, filename: file.name, bytesB64: b64 });
      await loadCtx();
    } catch { /* surfaced via reload */ }
    finally { setCtxBusy(false); }
  }
  async function removeCtx(id: number) {
    if (!initial?.id) return;
    try { await invoke("agent_context_remove", { agentId: initial.id, id }); await loadCtx(); } catch { /* ignore */ }
  }

  // M1.4 #5: generate a Soul.md (agent authors its own personality/values).
  const [soulBrief, setSoulBrief] = useState("");
  const [soulBusy, setSoulBusy] = useState(false);
  const [soulErr, setSoulErr] = useState<string | null>(null);
  async function generateSoul() {
    if (!initial?.id) { setSoulErr("Save the agent first, then generate its soul."); return; }
    setSoulBusy(true); setSoulErr(null);
    try {
      const soul = await invoke<string>("agent_generate_soul", { agentId: initial.id, brief: soulBrief });
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

  // If the user picks a folder via the native picker, adopt it here.
  useEffect(() => { if (pendingFolder && !initial) setFolder(pendingFolder); }, [pendingFolder, initial]);

  // Fetch this provider's models. Anthropic uses its own command; openai/
  // openrouter share one. Local models are file paths (handled elsewhere) so we
  // skip the live fetch. RESILIENT: the create form can mount before the daemon/
  // keychain handshake settles, so the first call may throw "no key" spuriously.
  // We retry a few times with backoff, and also expose a manual refetch that the
  // dropdown fires on focus — so it can never come up permanently empty.
  async function loadModels(prov: string): Promise<void> {
    if (prov === "local") { setModels([]); setModelsErr(null); return; }
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
    else void loadModels(provider);
    /* eslint-disable-next-line */
  }, [provider]);

  async function save() {
    if (!name.trim()) return;
    setSaving(true);
    try {
      if (initial) {
        const updated: AgentProfile = {
          ...initial, name: name.trim(), icon, color,
          folder_path: folder, model, provider, system_prompt: systemPrompt,
        };
        await invoke("agents_update", { profile: updated });
        onDone(updated);
      } else {
        const created = await invoke<AgentProfile>("agents_create", {
          name: name.trim(), icon, color, folderPath: folder, model, provider,
          contextMode: "isolated", systemPrompt,
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

        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <label style={{ ...fieldLabel, flex: 0 }}>Icon
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              {ICONS.map((i) => {
                const sel = icon === i;
                return (
                  <button key={i} onClick={() => setIcon(i)} style={{
                    width: 36, height: 36, borderRadius: 9, fontSize: 18, cursor: "pointer",
                    border: sel ? "2px solid var(--accent)" : "2px solid var(--line)",
                    background: sel ? "color-mix(in srgb, var(--accent) 14%, var(--bg))" : "var(--bg)",
                    boxShadow: sel ? "0 0 0 2px color-mix(in srgb, var(--accent) 35%, transparent)" : "none",
                    transition: "border-color .12s, box-shadow .12s, background .12s",
                  }}>{i}</button>
                );
              })}
            </div>
          </label>
          <label style={{ ...fieldLabel, flex: 0 }}>Color
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              {COLORS.map((c) => {
                const sel = color === c;
                return (
                  <button key={c} onClick={() => setColor(c)} title={c} style={{
                    width: 32, height: 32, borderRadius: 8, background: c, cursor: "pointer",
                    border: sel ? "2px solid var(--text)" : "2px solid transparent",
                    boxShadow: sel ? "0 0 0 2px color-mix(in srgb, var(--text) 30%, transparent)" : "none",
                    transition: "box-shadow .12s",
                  }} />
                );
              })}
            </div>
          </label>
        </div>

        <label style={fieldLabel}>Agent Folder (its jail)
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Input value={folder} onChange={(e) => setFolder(e.target.value)} mono placeholder="pick a folder…" />
            <Button variant="secondary" onClick={onPickFolder}>Pick…</Button>
          </div>
          <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>
            This agent can only ever touch files inside this folder.
          </span>
          {sharedWith.length > 0 && (
            <span style={{ fontSize: 12, color: "var(--accent)", background: "color-mix(in srgb, var(--accent) 10%, transparent)", border: "1px solid color-mix(in srgb, var(--accent) 30%, transparent)", borderRadius: "var(--radius-control)", padding: "7px 10px" }}>
              🔗 Shares this folder with {sharedWith.map((a) => a.name).join(", ")} — they share the same edit history &amp; checkpoints, and take turns writing so their changes never collide.
            </span>
          )}
        </label>

        <div style={{ display: "flex", gap: 12 }}>
          <label style={fieldLabel}>Provider
            <select value={provider} onChange={(e) => { setProvider(e.target.value); setModel(""); }} style={selectStyle}>
              <option value="anthropic">Anthropic</option>
              <option value="openai">OpenAI</option>
              <option value="openrouter">OpenRouter</option>
              <option value="local">Local</option>
            </select>
          </label>
          <label style={fieldLabel}>Model
            {provider === "local" ? (
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
            {!modelsErr && !modelsLoading && provider !== "local" && models.length > 0 && (
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Most capable first.</span>
            )}
            {provider === "local" && !localLoading && localModels.length === 0 && (
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>No downloaded models found — grab one in Settings, or paste a .gguf path.</span>
            )}
            {provider === "local" && localModels.length > 0 && (
              <span style={{ ...hint, fontSize: 12, color: "var(--text-faint)" }}>Downloaded models on this Mac.</span>
            )}
          </label>
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
          <div style={{ fontSize: 13, fontWeight: 600 }}>📎 Context documents</div>
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

        <div style={{ display: "flex", gap: 8 }}>
          <Button onClick={save} disabled={saving || !name.trim()}>{initial ? "Save" : "Create agent"}</Button>
          <Button variant="secondary" onClick={onCancel}>Cancel</Button>
        </div>
      </Card>
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
