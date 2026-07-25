// AYGENT — Tools tab. Extensible agent capabilities as enable/disable/delete
// blocks. PDF Generator is the first built-in. Users build their own COMPOSED
// tools (saved instructions + a curated subset of base tools) — with Pro Mode
// to author them WITH the agent, in-app, never leaving.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Input, Pill } from "../components/ui";

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const BASE_TOOLS = ["read_file", "write_file", "list_files", "generate_pdf"];

type Tool = {
  id: string; name: string; display_name: string; description: string;
  kind: string; builtin: boolean; instructions: string; allowed_tools: string[];
  enabled: boolean;
};

export function Tools({ folder }: { folder: string | null }) {
  const [tools, setTools] = useState<Tool[]>([]);
  const [editing, setEditing] = useState<Tool | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  async function refresh() {
    try { setTools(await invoke<Tool[]>("tools_list", { folder })); } catch { /* ignore */ }
  }
  useEffect(() => { refresh(); /* eslint-disable-next-line */ }, [folder]);

  async function toggle(t: Tool) {
    if (!folder) { setMsg("Pick an Agent Folder first (tools enable per folder)."); return; }
    try { await invoke("tools_set_enabled", { folder, id: t.id, on: !t.enabled }); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
  }
  async function del(t: Tool) {
    try { await invoke("tools_delete", { id: t.id }); await refresh(); }
    catch (e) { setMsg("✗ " + String(e)); }
  }
  function newTool() {
    setEditing({
      id: `user.${Date.now()}`, name: "", display_name: "", description: "",
      kind: "composed", builtin: false, instructions: "", allowed_tools: ["read_file", "write_file", "list_files"],
      enabled: false,
    });
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 720 }}>
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between" }}>
        <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Tools</h2>
        <Button onClick={newTool}>+ New Tool</Button>
      </div>
      <p style={hint}>
        Tools are capabilities your agent can use. Turn them on per folder. Build your own — or open Pro Mode to design one with the agent.
      </p>
      {!folder && <Pill tone="muted">Pick an Agent Folder in Settings to enable tools.</Pill>}
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "muted"}>{msg}</Pill>}

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
        {tools.map((t) => (
          <div key={t.id} style={{
            display: "flex", flexDirection: "column", gap: 8, padding: 14,
            border: `var(--border-width) solid ${t.enabled ? "var(--accent, var(--text))" : "var(--line)"}`,
            borderRadius: "var(--radius-control)", background: t.enabled ? "var(--bg)" : "transparent",
            boxShadow: t.enabled ? "var(--elevation)" : "none",
          }}>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
              <span style={{ fontWeight: 800, fontSize: 15 }}>{t.display_name || t.name}</span>
              {t.builtin ? <Pill tone="muted">built-in</Pill> : <Pill tone="muted">custom</Pill>}
            </div>
            <p style={{ ...hint, fontSize: 13, flex: 1 }}>{t.description}</p>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <Button variant={t.enabled ? "primary" : "secondary"} onClick={() => toggle(t)}>
                {t.enabled ? "Enabled ✓" : "Enable"}
              </Button>
              {!t.builtin && <Button variant="secondary" onClick={() => setEditing(t)}>Edit</Button>}
              {!t.builtin && <Button variant="secondary" onClick={() => del(t)}>Delete</Button>}
            </div>
          </div>
        ))}
      </div>

      {editing && (
        <ToolEditor
          tool={editing} folder={folder}
          onClose={() => setEditing(null)}
          onSaved={async () => { setEditing(null); await refresh(); }}
        />
      )}
    </div>
  );
}

// Editor + Pro Mode: define a composed tool, and optionally ask the agent to
// draft its fields for you (Pro Mode) using the currently selected model.
function ToolEditor({ tool, folder, onClose, onSaved }: {
  tool: Tool; folder: string | null; onClose: () => void; onSaved: () => void;
}) {
  const [t, setT] = useState<Tool>(tool);
  const [pro, setPro] = useState("");
  const [drafting, setDrafting] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  function set<K extends keyof Tool>(k: K, v: Tool[K]) { setT((x) => ({ ...x, [k]: v })); }
  function toggleAllowed(name: string) {
    setT((x) => ({ ...x, allowed_tools: x.allowed_tools.includes(name)
      ? x.allowed_tools.filter((n) => n !== name) : [...x.allowed_tools, name] }));
  }

  async function save() {
    setErr(null);
    const name = t.name.trim();
    if (!/^[a-z][a-z0-9_]*$/.test(name)) { setErr("Tool name must be snake_case (letters, digits, underscores)."); return; }
    try { await invoke("tools_upsert", { tool: { ...t, name } }); onSaved(); }
    catch (e) { setErr("✗ " + String(e)); }
  }

  // PRO MODE: ask the currently-selected agent to draft this tool. We reuse the
  // agent_stream loop with a meta-prompt and parse a JSON block from the reply.
  async function draftWithAgent() {
    if (!folder || !pro.trim()) return;
    setDrafting(true); setErr(null);
    try {
      const sel = await invoke<{ provider: string; model: string }>("get_selection", { folder });
      const meta =
        `Design an AYGENT tool from this request: "${pro.trim()}".\n` +
        `Reply with ONLY a JSON object: {"name": snake_case, "display_name": short label, ` +
        `"description": one sentence, "instructions": how the agent should carry it out, ` +
        `"allowed_tools": subset of ${JSON.stringify(BASE_TOOLS)}}.`;
      const channel = `tooldraft-${Date.now()}`;
      const result = await invoke<any>("agent_stream", {
        channel, prompt: meta, history: [],
        model: sel.model || null, provider: sel.provider || null,
      });
      // Pull the assistant text out of the returned history + parse the JSON.
      const text = extractLastAssistantText(result);
      const obj = parseJsonObject(text);
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
        setErr("The agent didn't return a clean tool definition — try rephrasing, or fill it in by hand.");
      }
    } catch (e) { setErr("✗ " + String(e)); }
    finally { setDrafting(false); }
  }

  return (
    <Card title={tool.name ? "Edit tool" : "New tool"}>
      {/* PRO MODE */}
      <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: 12, border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)" }}>
        <span style={{ fontWeight: 700, fontSize: 14 }}>✨ Pro Mode — design it with the agent</span>
        <p style={{ ...hint, fontSize: 12 }}>Describe what you want the tool to do; the agent drafts the fields below. {!folder && "(needs an Agent Folder + selected model)"}</p>
        <div style={{ display: "flex", gap: 8 }}>
          <Input value={pro} onChange={(e) => setPro(e.target.value)} placeholder="e.g. summarize every .md file into a summary.md" />
          <Button onClick={draftWithAgent} disabled={!folder || drafting || !pro.trim()}>{drafting ? "Drafting…" : "Draft"}</Button>
        </div>
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 10, marginTop: 10 }}>
        <Field label="Tool name (snake_case)"><Input mono value={t.name} onChange={(e) => set("name", e.target.value)} placeholder="summarize_notes" /></Field>
        <Field label="Display name"><Input value={t.display_name} onChange={(e) => set("display_name", e.target.value)} placeholder="Notes Summarizer" /></Field>
        <Field label="Description"><Input value={t.description} onChange={(e) => set("description", e.target.value)} placeholder="Summarizes markdown notes into one file." /></Field>
        <Field label="Instructions (how the agent carries it out)">
          <textarea value={t.instructions} onChange={(e) => set("instructions", e.target.value)} rows={4}
            style={{ width: "100%", fontFamily: "inherit", fontSize: 14, padding: 10, borderRadius: "var(--radius-control)",
              border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)", resize: "vertical" }}
            placeholder="Read each .md file, write a concise combined summary to summary.md…" />
        </Field>
        <Field label="Allowed base tools">
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
          <Button onClick={save}>Save tool</Button>
          <Button variant="secondary" onClick={onClose}>Cancel</Button>
        </div>
      </div>
    </Card>
  );
}

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
