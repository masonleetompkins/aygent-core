// AYGENT — VIDEO tab (v0.1 project store UI).
//
// Reads/writes file-backed video projects in the agent's jailed folder:
//   Video/<project>/composition.json (+ cutlist.json / transcript.json sidecars)
// Rendering stays the agent's job (Pro Mode shell + provisioned ffmpeg /
// HyperFrames) — this screen is the project store + toolchain status, not a
// renderer. Renders into App.tsx like every other This-Agent screen.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Button, Pill, Input } from "../components/ui";

type Project = { name: string; modified: number };
type Status = { ffmpeg: string | null; hyperframes: boolean };

const hint = { color: "var(--text-muted)", fontSize: 14, margin: 0 } as const;
const faint = { ...hint, fontSize: 12, color: "var(--text-faint)" } as const;

export function Video({ agentId }: { agentId: string | null }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [composition, setComposition] = useState<string>("");
  const [meta, setMeta] = useState<string>("");
  const [draftName, setDraftName] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  async function refresh() {
    setMsg(null);
    try {
      setStatus(await invoke<Status>("video_status"));
    } catch (e) {
      setMsg("✗ video_status: " + String(e));
    }
    if (!agentId) {
      setProjects([]);
      return;
    }
    try {
      const list = await invoke<Project[]>("video_projects", { agentId });
      setProjects(Array.isArray(list) ? list : []);
    } catch (e) {
      setMsg("✗ video_projects: " + String(e));
    }
  }

  useEffect(() => {
    refresh();
    setSelected(null);
    setComposition("");
    setMeta("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

  async function load(name: string) {
    if (!agentId) return;
    setBusy(true);
    setMsg(null);
    try {
      const r = await invoke<{ project: string; composition: unknown; cutlist: unknown; transcript: unknown }>(
        "video_load",
        { agentId, project: name }
      );
      setSelected(r.project);
      setComposition(JSON.stringify(r.composition, null, 2));
      const sidecars: string[] = [];
      if (r.cutlist !== null && r.cutlist !== undefined) sidecars.push("cutlist.json present");
      if (r.transcript !== null && r.transcript !== undefined) sidecars.push("transcript.json present");
      setMeta(sidecars.join(" · ") || "no sidecars");
    } catch (e) {
      setMsg("✗ video_load: " + String(e));
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    if (!agentId) {
      setMsg("Select an agent first.");
      return;
    }
    const name = (selected || draftName).trim();
    if (!name) {
      setMsg("Pick a project or type a new project name.");
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(composition);
    } catch {
      setMsg("✗ composition is not valid JSON — fix it before saving.");
      return;
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      setMsg("✗ composition must be a JSON object.");
      return;
    }
    setBusy(true);
    setMsg(null);
    try {
      await invoke("video_save", { agentId, project: name, composition: parsed });
      setSelected(name);
      setDraftName("");
      setMsg(`saved ✓ Video/${name}/composition.json`);
      await refresh();
    } catch (e) {
      setMsg("✗ video_save: " + String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 18, maxWidth: 860 }}>
      <div>
        <h2 style={{ fontSize: 22, fontWeight: 800, margin: 0 }}>Video</h2>
        <p style={{ ...hint, marginTop: 6 }}>
          Code-editable video projects. The composition is the edit — ask the agent to cut,
          caption, grade, and export it. Renders run in Pro Mode (shell + ffmpeg).
        </p>
      </div>

      {!agentId && <Pill tone="muted">Pick an agent to see its video projects.</Pill>}
      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "muted"}>{msg}</Pill>}

      <Card title="Toolchain">
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          {status?.ffmpeg ? (
            <Pill tone="ok">ffmpeg ✓</Pill>
          ) : (
            <Pill tone="muted">ffmpeg not provisioned</Pill>
          )}
          {status?.hyperframes ? (
            <Pill tone="ok">hyperframes ✓</Pill>
          ) : (
            <Pill tone="muted">hyperframes off</Pill>
          )}
          <span style={{ flex: 1 }} />
          <Button variant="secondary" onClick={() => void refresh()} disabled={busy}>
            Refresh
          </Button>
        </div>
        {status?.ffmpeg ? (
          <p style={faint}>ffmpeg: {status.ffmpeg}</p>
        ) : (
          <p style={faint}>
            Enable HyperFrames in Skills (or provision ffmpeg) to render. The project store works without it.
          </p>
        )}
      </Card>

      <Card title="Projects">
        {!agentId ? (
          <p style={hint}>Select an agent first.</p>
        ) : projects.length === 0 ? (
          <p style={hint}>
            No projects yet. Create one below, or ask the agent in Chat to scaffold
            <code style={{ fontFamily: "ui-monospace, monospace" }}> Video/&lt;name&gt;/composition.json</code>.
          </p>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {projects.map((p) => (
              <div
                key={p.name}
                style={{
                  display: "flex", alignItems: "center", gap: 10, padding: "8px 12px",
                  border: `var(--border-width) solid ${selected === p.name ? "var(--accent, var(--text))" : "var(--line)"}`,
                  borderRadius: "var(--radius-control)",
                  background: selected === p.name ? "var(--bg)" : "transparent",
                }}
              >
                <code style={{ fontSize: 13, fontWeight: 700 }}>{p.name}</code>
                <span style={{ ...faint }}>
                  {p.modified ? new Date(p.modified * 1000).toLocaleString() : ""}
                </span>
                <span style={{ flex: 1 }} />
                <Button variant="secondary" onClick={() => void load(p.name)} disabled={busy}>
                  {selected === p.name ? "Reload" : "Open"}
                </Button>
              </div>
            ))}
          </div>
        )}
      </Card>

      <Card title={selected ? `Composition — ${selected}` : "Composition"}>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          <Input
            value={draftName}
            onChange={(e) => setDraftName(e.target.value)}
            placeholder="new-project-name"
            style={{ maxWidth: 260 }}
          />
          <Button onClick={() => void save()} disabled={busy || !agentId}>
            {busy ? "…" : selected ? "Save composition" : "Create project"}
          </Button>
          {meta && <span style={faint}>{meta}</span>}
        </div>
        <textarea
          value={composition}
          onChange={(e) => setComposition(e.target.value)}
          rows={22}
          spellCheck={false}
          placeholder='{"scene":{"width":1920,"height":1080,"fps":30},"sequence":[],"captions":{},"color":{},"audio":{},"exports":[]}'
          style={{
            width: "100%", fontFamily: "ui-monospace, monospace", fontSize: 12.5,
            padding: 12, borderRadius: "var(--radius-control)",
            border: "var(--border-width) solid var(--line)",
            background: "var(--bg)", color: "var(--text)", resize: "vertical",
          }}
        />
        <p style={faint}>
          v0.1 schema: scene (width/height/fps) · sequence clips (src, start/end, sourceIn/sourceOut) ·
          captions · color (lut + s-curve) · audio (enhance + duck) · exports (size + bitrate).
        </p>
      </Card>
    </div>
  );
}
