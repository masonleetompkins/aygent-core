// AYGENT — TOOLS + SKILLS. Two tabs, because they answer different questions:
//
//   TOOLS  = "what can my agent actually call?"  (machine capabilities)
//   SKILLS = "how do I want work done?"          (saved procedures, no secrets)
//
// The old single tab mixed them: user-authored `composed` entries looked like
// tools but behaved like instructions, so neither idea was legible. Tools is now
// a read-mostly INVENTORY with an origin badge per row (built-in / connection /
// mcp) — you never add a connection tool here, you enable the Connection and it
// appears. Skills is where you author.
import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";
import { AygentBrowser } from "../components/AygentBrowser";
import { AygentHyperFrames } from "../components/AygentHyperFrames";

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const faint = { ...hint, fontSize: 12, color: "var(--text-faint)" } as const;
const BASE_TOOLS = ["read_file", "write_file", "list_files", "generate_pdf", "fetch_url", "web_search"];

type Capability = {
  id: string; name: string; display_name: string; description: string;
  origin: "built-in" | "connection" | "mcp";
  source: string; enabled: boolean; toggleable: boolean;
  access: "Read" | "Write"; has_config?: boolean;
};
type Skill = {
  id: string; name: string; display_name: string; description: string;
  instructions: string; allowed_tools: string[]; enabled: boolean; builtin: boolean;
};

export function Tools({ folder, agentId }: { folder: string | null; agentId: string | null }) {
  // TOOLS tab: the capability inventory + AYGENT-branded tools (the browser).
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 760 }}>
      <ToolInventory folder={folder} agentId={agentId} />
    </div>
  );
}

export function Skills({ folder, agentId }: { folder: string | null; agentId: string | null }) {
  // SKILLS tab: user-authored ways of working + AYGENT-branded skills (HyperFrames).
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 760 }}>
      <SkillList folder={folder} agentId={agentId} />
    </div>
  );
}

function TabButton({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button onClick={onClick} style={{
      fontSize: 15, fontWeight: 800, padding: "6px 14px", cursor: "pointer",
      borderRadius: "var(--radius-control)", color: "var(--text)",
      border: `var(--border-width) solid ${active ? "var(--accent, var(--text))" : "transparent"}`,
      background: active ? "var(--bg)" : "transparent",
      boxShadow: active ? "var(--elevation)" : "none",
    }}>{children}</button>
  );
}

// ---------------------------------------------------------------------------
// TOOLS — the inventory. Grouped by origin, with a badge per row, because the
// most common real question is "where did this capability come from?"
// ---------------------------------------------------------------------------

function ToolInventory({ folder, agentId }: { folder: string | null; agentId: string | null }) {
  const [caps, setCaps] = useState<Capability[]>([]);
  const [configuring, setConfiguring] = useState<Capability | null>(null);
  const [filter, setFilter] = useState<"all" | "built-in" | "connection" | "mcp">("all");
  // POLISH #6 (Mason v1.0.1): connection tools are grouped per provider and
  // COLLAPSED by default — they have no switches here (control lives in
  // Connections), so a flat list was a wall of noise burying the rows a user
  // can actually act on.
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({});
  const [msg, setMsg] = useState<string | null>(null);

  async function refresh() {
    try { setCaps(await invoke<Capability[]>("capabilities_list", { agentId, folder })); }
    catch (e) { setMsg("✗ " + String(e)); }
  }
  useEffect(() => { refresh(); /* eslint-disable-next-line */ }, [folder, agentId]);

  async function toggle(c: Capability) {
    if (!agentId) { setMsg("Select an agent first — tools enable per agent."); return; }
    try { await invoke("tools_set_enabled", { agentId, folder, id: c.id, on: !c.enabled }); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
  }

  const shown = useMemo(
    () => (filter === "all" ? caps : caps.filter((c) => c.origin === filter)),
    [caps, filter]
  );
  const counts = useMemo(() => {
    const by: Record<string, number> = {};
    for (const c of caps) by[c.origin] = (by[c.origin] ?? 0) + 1;
    return by;
  }, [caps]);

  return (
    <>
      <div>
        <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Tools</h2>
        <p style={{ ...hint, marginTop: 6 }}>
          Everything this agent can actually call, and where it came from. Connection tools appear
          here automatically when you enable a service in <b>Connections</b> — there's nothing to add.
        </p>
      </div>

      {!agentId && <Pill tone="muted">Pick an agent to see its tools.</Pill>}
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "muted"}>{msg}</Pill>}

      <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
        <FilterChip on={filter === "all"} onClick={() => setFilter("all")}>All {caps.length}</FilterChip>
        <FilterChip on={filter === "built-in"} onClick={() => setFilter("built-in")}>
          Built-in {counts["built-in"] ?? 0}
        </FilterChip>
        <FilterChip on={filter === "connection"} onClick={() => setFilter("connection")}>
          From connections {counts["connection"] ?? 0}
        </FilterChip>
        <FilterChip on={filter === "mcp"} onClick={() => setFilter("mcp")}>
          MCP {counts["mcp"] ?? 0}
        </FilterChip>
      </div>

      {filter === "mcp" && (counts["mcp"] ?? 0) === 0 && (
        <Pill tone="muted">No MCP servers yet — that library is coming.</Pill>
      )}

      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {/* Non-connection tools: flat, actionable rows first. */}
        {shown.filter((c) => c.origin !== "connection").map((c) => (
          <ToolRow key={c.id} c={c} onConfigure={() => setConfiguring(c)} onToggle={() => toggle(c)} />
        ))}

        {/* Connection tools: one collapsible section per provider. */}
        {filter !== "built-in" && filter !== "mcp" && (() => {
          const groups = new Map<string, Capability[]>();
          for (const c of shown.filter((x) => x.origin === "connection")) {
            const key = c.source || "connection";
            if (!groups.has(key)) groups.set(key, []);
            groups.get(key)!.push(c);
          }
          return [...groups.entries()].map(([source, tools]) => {
            const open = !!openGroups[source];
            return (
              <div key={source} style={{
                border: "var(--border-width) solid var(--line)",
                borderRadius: "var(--radius-control)", overflow: "hidden",
              }}>
                <button
                  onClick={() => setOpenGroups((g) => ({ ...g, [source]: !open }))}
                  style={{
                    display: "flex", alignItems: "center", gap: 10, width: "100%",
                    padding: "10px 12px", border: "none", background: "transparent",
                    color: "var(--text)", cursor: "pointer", textAlign: "left",
                  }}
                >
                  <span style={{ fontSize: 12, color: "var(--text-muted)", width: 12 }}>
                    {open ? "▾" : "▸"}
                  </span>
                  <span style={{ fontSize: 13, fontWeight: 800, textTransform: "capitalize" }}>{source}</span>
                  <Pill tone="muted">{tools.length} tools</Pill>
                  <span style={{ flex: 1 }} />
                  <span style={faint}>on/off per tool lives in Connections</span>
                </button>
                {open && (
                  <div style={{ display: "flex", flexDirection: "column", gap: 4, padding: "0 12px 10px" }}>
                    {tools.map((c) => (
                      <div key={c.id} style={{
                        display: "flex", alignItems: "baseline", gap: 8, flexWrap: "wrap",
                        padding: "6px 0", borderTop: "var(--border-width) solid color-mix(in srgb, var(--line) 30%, transparent)",
                        opacity: c.enabled ? 1 : 0.55,
                      }}>
                        <code style={{ fontSize: 12.5, fontWeight: 700 }}>{c.name}</code>
                        {c.access === "Write" && <Pill tone="danger">write</Pill>}
                        {!c.enabled && <Pill tone="muted">off</Pill>}
                        <span style={{ ...faint, fontSize: 12 }}>{c.description}</span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          });
        })()}
      </div>

      {/* AYGENT-branded tools: the in-app browser (HyperFrames lives in Skills). */}
      <AygentBrowser />

      {configuring && (
        <ToolConfig tool={configuring} folder={folder} agentId={agentId}
                    onClose={() => setConfiguring(null)} />
      )}
    </>
  );
}

function ToolRow({ c, onConfigure, onToggle }: { c: Capability; onConfigure: () => void; onToggle: () => void }) {
  return (
    <div style={{
      display: "flex", alignItems: "center", gap: 10, padding: "10px 12px", flexWrap: "wrap",
      border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)",
      opacity: c.enabled ? 1 : 0.55,
    }}>
      <code style={{ fontSize: 13, fontWeight: 700 }}>{c.name}</code>
      <OriginBadge origin={c.origin} source={c.source} />
      <span style={{ flex: 1 }} />
      {c.has_config && (
        <Button variant="secondary" onClick={onConfigure}>Configure</Button>
      )}
      {c.toggleable ? (
        <Button variant={c.enabled ? "primary" : "secondary"} onClick={onToggle}>
          {c.enabled ? "On" : "Off"}
        </Button>
      ) : (
        <span style={faint}>always on</span>
      )}
      <p style={{ ...faint, flexBasis: "100%", margin: 0 }}>{c.description}</p>
    </div>
  );
}

function FilterChip({ on, onClick, children }: { on: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button onClick={onClick} style={{
      fontSize: 12, fontWeight: 700, padding: "5px 10px", cursor: "pointer", borderRadius: 999,
      color: "var(--text)", background: on ? "var(--bg)" : "transparent",
      border: `var(--border-width) solid ${on ? "var(--accent, var(--text))" : "var(--line)"}`,
    }}>{children}</button>
  );
}

function OriginBadge({ origin, source }: { origin: string; source: string }) {
  // The badge answers "where did this come from?" — a connection tool naming its
  // service is the difference between a mystery tool and an obvious one.
  const label = origin === "connection" ? source : origin === "mcp" ? `MCP · ${source}` : source;
  return <Pill tone="muted">{label}</Pill>;
}

// ---------------------------------------------------------------------------
// SKILLS — saved procedures. Instructions plus the tools they're allowed to use.
// No credentials, no network of their own: a skill can only do what the agent
// could already do, in a particular way. That's why this list is safe to author
// freely while Connections needs a write gate.
// ---------------------------------------------------------------------------

function SkillList({ folder, agentId }: { folder: string | null; agentId: string | null }) {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [editing, setEditing] = useState<Skill | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  async function refresh() {
    try {
      const all = await invoke<Skill[]>("skills_list", { agentId, folder });
      // The HyperFrames skill is MANAGED by its own card below (install/enable/
      // remove) — hide it from the generic user-skill list so it isn't shown twice.
      setSkills(all.filter((k) => k.id !== "skill.hyperframes"));
    }
    catch (e) { setMsg("✗ " + String(e)); }
  }
  useEffect(() => { refresh(); /* eslint-disable-next-line */ }, [folder, agentId]);

  async function toggle(s: Skill) {
    if (!agentId) { setMsg("Select an agent first — skills enable per agent."); return; }
    try { await invoke("tools_set_enabled", { agentId, folder, id: s.id, on: !s.enabled }); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
  }
  async function del(s: Skill) {
    try { await invoke("tools_delete", { id: s.id }); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
  }
  function newSkill() {
    setEditing({
      id: `user.${Date.now()}`, name: "", display_name: "", description: "",
      instructions: "", allowed_tools: ["read_file", "write_file", "list_files"],
      enabled: false, builtin: false,
    });
  }

  return (
    <>
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 12 }}>
        <div>
          <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Skills</h2>
          <p style={{ ...hint, marginTop: 6 }}>
            A skill is a way of working you've taught the agent — saved instructions plus the tools
            it may use. No credentials, and it can't do anything the agent couldn't already do.
          </p>
        </div>
        <Button onClick={newSkill} style={{ whiteSpace: "nowrap", flexShrink: 0 }}>+ New Skill</Button>
      </div>

      {!folder && <Pill tone="muted">Pick an Agent Folder in Settings to use skills.</Pill>}
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "muted"}>{msg}</Pill>}

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
        {skills.map((s) => (
          <div key={s.id} style={{
            display: "flex", flexDirection: "column", gap: 8, padding: 14,
            border: `var(--border-width) solid ${s.enabled ? "var(--accent, var(--text))" : "var(--line)"}`,
            borderRadius: "var(--radius-control)", background: s.enabled ? "var(--bg)" : "transparent",
            boxShadow: s.enabled ? "var(--elevation)" : "none",
          }}>
            <span style={{ fontWeight: 800, fontSize: 15 }}>{s.display_name || s.name}</span>
            <p style={{ ...faint, flex: 1 }}>{s.description}</p>
            <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
              {s.allowed_tools.map((t) => <Pill key={t} tone="muted">{t}</Pill>)}
            </div>
            <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
              <Button variant={s.enabled ? "primary" : "secondary"} onClick={() => toggle(s)}>
                {s.enabled ? "On" : "Off"}
              </Button>
              <Button variant="secondary" onClick={() => setEditing(s)}>Edit</Button>
              <Button variant="secondary" onClick={() => del(s)}>Delete</Button>
            </div>
          </div>
        ))}
      </div>

      {editing && (
        <SkillEditor
          skill={editing} folder={folder}
          onClose={() => setEditing(null)}
          onSaved={async () => { setEditing(null); await refresh(); }}
        />
      )}

      {/* AYGENT-branded skill: HyperFrames video/motion-graphics (one-click enable). */}
      <AygentHyperFrames agentId={agentId} folder={folder} />
    </>
  );
}

// ---------------------------------------------------------------------------
// SKILL EDITOR (+ Pro Mode drafting) and the per-tool CONFIG panel. Both carried
// over from the old Tools tab — the config panel is still schema-driven from the
// backend, which is the pattern every future configurable tool inherits.
// ---------------------------------------------------------------------------

function SkillEditor({ skill, folder, onClose, onSaved }: {
  skill: Skill; folder: string | null; onClose: () => void; onSaved: () => void;
}) {
  const [t, setT] = useState<Skill>(skill);
  const [pro, setPro] = useState("");
  const [drafting, setDrafting] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  function set<K extends keyof Skill>(k: K, v: Skill[K]) { setT((x) => ({ ...x, [k]: v })); }
  function toggleAllowed(name: string) {
    setT((x) => ({ ...x, allowed_tools: x.allowed_tools.includes(name)
      ? x.allowed_tools.filter((n) => n !== name) : [...x.allowed_tools, name] }));
  }

  async function save() {
    setErr(null);
    const name = t.name.trim();
    if (!/^[a-z][a-z0-9_]*$/.test(name)) { setErr("Skill name must be snake_case (letters, digits, underscores)."); return; }
    try {
      // Storage is unchanged: a skill is still kind:"composed" in the registry.
      await invoke("tools_upsert", { tool: { ...t, name, kind: "composed", builtin: false } });
      onSaved();
    } catch (e) { setErr("✗ " + String(e)); }
  }

  // PRO MODE: let the agent draft the skill from a plain description.
  async function draftWithAgent() {
    if (!folder || !pro.trim()) return;
    setDrafting(true); setErr(null);
    try {
      const sel = await invoke<{ provider: string; model: string }>("get_selection", { folder });
      const meta =
        `Design an AYGENT skill from this request: "${pro.trim()}".\n` +
        `Reply with ONLY a JSON object: {"name": snake_case, "display_name": short label, ` +
        `"description": one sentence, "instructions": how the agent should carry it out, ` +
        `"allowed_tools": subset of ${JSON.stringify(BASE_TOOLS)}}.`;
      const result = await invoke<any>("agent_stream", {
        channel: `skilldraft-${Date.now()}`, prompt: meta, history: [],
        model: sel.model || null, provider: sel.provider || null,
      });
      const obj = parseJsonObject(extractLastAssistantText(result));
      if (obj) {
        setT((x) => ({
          ...x,
          name: obj.name || x.name,
          display_name: obj.display_name || x.display_name,
          description: obj.description || x.description,
          instructions: obj.instructions || x.instructions,
          allowed_tools: Array.isArray(obj.allowed_tools) ? obj.allowed_tools : x.allowed_tools,
        }));
      } else {
        setErr("The agent didn't return a clean skill definition — try rephrasing, or fill it in by hand.");
      }
    } catch (e) { setErr("✗ " + String(e)); }
    finally { setDrafting(false); }
  }

  return (
    <Card title={skill.name ? "Edit skill" : "New skill"}>
      <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: 12,
                    border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)" }}>
        <span style={{ fontWeight: 700, fontSize: 14 }}>✨ Draft it with the agent</span>
        <p style={{ ...faint }}>
          Describe what you want in your own words; the agent fills in the fields below.
          {!folder && " (needs an Agent Folder + selected model)"}
        </p>
        <div style={{ display: "flex", gap: 8 }}>
          <Input value={pro} onChange={(e) => setPro(e.target.value)}
                 placeholder="e.g. summarize every .md file into summary.md" />
          <Button onClick={draftWithAgent} disabled={!folder || drafting || !pro.trim()}>
            {drafting ? "Drafting…" : "Draft"}
          </Button>
        </div>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 10, marginTop: 10 }}>
        <Field label="Skill name (snake_case)">
          <Input mono value={t.name} onChange={(e) => set("name", e.target.value)} placeholder="summarize_notes" />
        </Field>
        <Field label="Display name">
          <Input value={t.display_name} onChange={(e) => set("display_name", e.target.value)} placeholder="Notes Summarizer" />
        </Field>
        <Field label="Description">
          <Input value={t.description} onChange={(e) => set("description", e.target.value)}
                 placeholder="Summarizes markdown notes into one file." />
        </Field>
        <Field label="Instructions (how the agent carries it out)">
          <textarea value={t.instructions} onChange={(e) => set("instructions", e.target.value)} rows={5}
            style={{ width: "100%", fontFamily: "inherit", fontSize: 14, padding: 10,
                     borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)",
                     background: "var(--bg)", color: "var(--text)", resize: "vertical" }}
            placeholder="Read each .md file, write a concise combined summary to summary.md…" />
        </Field>
        <Field label="Tools this skill may use">
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            {BASE_TOOLS.map((n) => (
              <button key={n} onClick={() => toggleAllowed(n)} style={{
                fontSize: 12, fontFamily: "ui-monospace, monospace", padding: "5px 10px", cursor: "pointer",
                borderRadius: 999, color: "var(--text)",
                border: `var(--border-width) solid ${t.allowed_tools.includes(n) ? "var(--accent, var(--text))" : "var(--line)"}`,
                background: t.allowed_tools.includes(n) ? "var(--bg)" : "transparent",
              }}>{t.allowed_tools.includes(n) ? "✓ " : ""}{n}</button>
            ))}
          </div>
        </Field>
        {err && <Pill tone="danger">{err}</Pill>}
        <div style={{ display: "flex", gap: 8 }}>
          <Button onClick={save}>Save skill</Button>
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
        </div>
      </div>
    </Card>
  );
}

// Per-tool config, rendered from the backend's declared SCHEMA so the UI stays
// generic: add a config field in Rust and the control appears here.
type ConfigField = { key: string; label: string; type: string; default: any; help?: string; options?: string[]; min?: number; max?: number };

function ToolConfig({ tool, folder, agentId, onClose }: {
  tool: Capability; folder: string | null; agentId: string | null; onClose: () => void;
}) {
  const [schema, setSchema] = useState<ConfigField[]>([]);
  const [values, setValues] = useState<Record<string, any>>({});
  const [fonts, setFonts] = useState<string[]>([]);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    if (!agentId) return;
    invoke<{ schema: ConfigField[]; values: Record<string, any>; fonts: string[] }>(
      "tools_config", { agentId, folder, id: tool.id }
    ).then((r) => {
      setSchema(r.schema || []);
      setFonts(r.fonts || []);
      const v: Record<string, any> = { ...(r.values || {}) };
      for (const f of r.schema || []) if (v[f.key] === undefined) v[f.key] = f.default;
      setValues(v);
    }).catch(() => {});
  }, [tool.id, folder, agentId]);

  async function save() {
    if (!agentId) return;
    try {
      await invoke("tools_set_config", { agentId, folder, id: tool.id, values });
      setSaved(true); setTimeout(() => setSaved(false), 1500);
    } catch { /* ignore */ }
  }
  function set(k: string, v: any) { setValues((x) => ({ ...x, [k]: v })); }

  if (!agentId) return (
    <Card title={`Configure ${tool.display_name}`}>
      <p style={hint}>Select an agent first.</p>
      <Button variant="secondary" onClick={onClose}>Close</Button>
    </Card>
  );
  if (schema.length === 0) return (
    <Card title={`Configure ${tool.display_name}`}>
      <p style={hint}>This tool has no settings.</p>
      <Button variant="secondary" onClick={onClose}>Close</Button>
    </Card>
  );

  return (
    <Card title={`Configure ${tool.display_name}`}>
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        {schema.map((f) => (
          <label key={f.key} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>{f.label}</span>
            {f.type === "font" ? (
              <select value={values[f.key] ?? "bundled"} onChange={(e) => set(f.key, e.target.value)} style={selectStyle}>
                {fonts.map((fn) => (
                  <option key={fn} value={fn}>
                    {fn === "bundled" ? "Bundled (DejaVu Sans) — always works" : fn}
                  </option>
                ))}
              </select>
            ) : f.type === "select" ? (
              <select value={values[f.key] ?? f.default} onChange={(e) => set(f.key, e.target.value)} style={selectStyle}>
                {(f.options || []).map((o) => <option key={o} value={o}>{o}</option>)}
              </select>
            ) : f.type === "color" ? (
              <input type="color" value={values[f.key] ?? f.default} onChange={(e) => set(f.key, e.target.value)}
                style={{ width: 52, height: 32, padding: 0, border: "var(--border-width) solid var(--line)",
                         borderRadius: 8, background: "none", cursor: "pointer" }} />
            ) : f.type === "number" ? (
              <input type="number" value={values[f.key] ?? f.default} min={f.min} max={f.max}
                onChange={(e) => set(f.key, Number(e.target.value))} style={{ width: 100, ...numStyle }} />
            ) : (
              <Input value={values[f.key] ?? ""} onChange={(e) => set(f.key, e.target.value)} />
            )}
            {f.help && <span style={faint}>{f.help}</span>}
          </label>
        ))}
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <Button onClick={save}>Save settings</Button>
          <Button variant="secondary" onClick={onClose}>Close</Button>
          {saved && <Pill tone="ok">saved ✓</Pill>}
        </div>
      </div>
    </Card>
  );
}

const selectStyle: React.CSSProperties = {
  fontSize: 14, padding: "8px 10px", borderRadius: "var(--radius-control)",
  border: "var(--border-width) solid var(--line)", background: "var(--bg)",
  color: "var(--text)", maxWidth: 320,
};
const numStyle: React.CSSProperties = {
  fontSize: 14, padding: "8px 10px", borderRadius: "var(--radius-control)",
  border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)",
};

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 13, fontWeight: 600 }}>{label}</span>
      {children}
    </label>
  );
}

function extractLastAssistantText(history: any): string {
  if (!Array.isArray(history)) return "";
  for (let i = history.length - 1; i >= 0; i--) {
    const m = history[i];
    if (m?.role !== "assistant") continue;
    const c = m.content;
    if (typeof c === "string") return c;
    if (Array.isArray(c)) return c.filter((b: any) => b?.text).map((b: any) => b.text).join("\n");
  }
  return "";
}

function parseJsonObject(text: string): any | null {
  const s = text.indexOf("{");
  const e = text.lastIndexOf("}");
  if (s < 0 || e <= s) return null;
  try { return JSON.parse(text.slice(s, e + 1)); } catch { return null; }
}
