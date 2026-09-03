// AYGENT — VIDEO editor v0.2 (agentic Premiere Pro, Diffusion-referenced).
//
// Layout (mirrors the referenced OSS editor, rebuilt on OUR stack):
//   top bar: project · aspect switch · save · export
//   left: media / assets panel
//   center: preview canvas + transport + multi-track timeline
//   right: inspector (selected clip) + agent prompt bar (selected AYGENT agent edits via prompts)
//
// Data: Video/<project>/composition.json — the edit. Agent (file tools) and
// human (this UI) both read/write it through video_load / video_save, so every
// change is code, diffable, rewindable via Save Points. Renders run in Pro Mode
// (ffmpeg/HyperFrames); this screen never executes anything itself.
//
// Timing semantics mirror Diffusion: start/end = placement on the timeline,
// sourceIn/sourceOut = which part of the source plays (rate 1 assumed in UI math).
import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Pill, Input } from "../components/ui";

type TrackId = "V2" | "V1" | "T1" | "A1" | "A2";
const TRACKS: { id: TrackId; label: string; kind: "video" | "title" | "audio" }[] = [
  { id: "V2", label: "V2 · overlays", kind: "video" },
  { id: "V1", label: "V1 · a-roll", kind: "video" },
  { id: "T1", label: "T1 · titles", kind: "title" },
  { id: "A1", label: "A1 · audio", kind: "audio" },
  { id: "A2", label: "A2 · music", kind: "audio" },
];

type Clip = {
  id: string;
  track: TrackId;
  src: string;
  name?: string;
  start: number;
  end: number;
  sourceIn?: number;
  sourceOut?: number;
  volume?: number;
  muted?: boolean;
  hidden?: boolean;
  x?: number; y?: number; width?: number; height?: number; opacity?: number;
  text?: string;
};

type Composition = {
  scene: { width: number; height: number; fps: number; background?: string };
  sequence: Clip[];
  captions?: { preset?: string; animated?: boolean };
  color?: { lut?: string; sCurve?: boolean; adjustmentLayer?: boolean };
  audio?: { enhance?: string; ducking?: boolean; musicBed?: string };
  exports?: { size: string; bitrate: string; codec: string }[];
};

type Project = { name: string; modified: number };

const hint = { color: "var(--text-muted)", fontSize: 13, margin: 0 } as const;
const faint = { ...hint, fontSize: 11.5, color: "var(--text-faint)" } as const;

const uid = () => `c${Date.now().toString(36)}${Math.floor(Math.random() * 1e4).toString(36)}`;
const num = (v: unknown, fb: number) => (typeof v === "number" && isFinite(v) ? v : fb);
const dur = (c: Clip) => Math.max(0, c.end - c.start);

function blankComposition(): Composition {
  return {
    scene: { width: 1920, height: 1080, fps: 30, background: "#000" },
    sequence: [],
    captions: { preset: "hyperframes", animated: true },
    color: { lut: "", sCurve: true, adjustmentLayer: true },
    audio: { enhance: "auphonic", ducking: true, musicBed: "" },
    exports: [
      { size: "1920x1080", bitrate: "12M", codec: "avc" },
      { size: "1080x1920", bitrate: "8M", codec: "avc" },
    ],
  };
}

function normalizeComposition(raw: unknown): Composition {
  const b = blankComposition();
  if (typeof raw !== "object" || raw === null) return b;
  const r = raw as Record<string, unknown>;
  const scene = (r.scene as Composition["scene"]) || b.scene;
  const seq = Array.isArray(r.sequence) ? (r.sequence as Clip[]) : [];
  const sequence: Clip[] = seq
    .filter((c) => c && typeof c === "object")
    .map((c, i) => ({
      id: typeof c.id === "string" && c.id ? c.id : `clip-${i}`,
      track: (["V2", "V1", "T1", "A1", "A2"] as TrackId[]).includes(c.track as TrackId)
        ? (c.track as TrackId) : ("V1" as TrackId),
      src: String(c.src ?? ""),
      name: typeof c.name === "string" ? c.name : undefined,
      start: num(c.start, 0),
      end: num(c.end, num(c.start, 0) + 4),
      sourceIn: c.sourceIn === undefined ? undefined : num(c.sourceIn, 0),
      sourceOut: c.sourceOut === undefined ? undefined : num(c.sourceOut, 0),
      volume: c.volume === undefined ? undefined : num(c.volume, 0),
      muted: !!c.muted, hidden: !!c.hidden,
      x: c.x === undefined ? undefined : num(c.x, 0),
      y: c.y === undefined ? undefined : num(c.y, 0),
      width: c.width === undefined ? undefined : num(c.width, 0),
      height: c.height === undefined ? undefined : num(c.height, 0),
      opacity: c.opacity === undefined ? undefined : num(c.opacity, 1),
      text: typeof c.text === "string" ? c.text : undefined,
    }))
    .map((c) => (c.end <= c.start ? { ...c, end: c.start + 4 } : c));
  return {
    scene: {
      width: num(scene.width, 1920), height: num(scene.height, 1080),
      fps: num(scene.fps, 30), background: typeof scene.background === "string" ? scene.background : "#000",
    },
    sequence,
    captions: (r.captions as Composition["captions"]) ?? b.captions,
    color: (r.color as Composition["color"]) ?? b.color,
    audio: (r.audio as Composition["audio"]) ?? b.audio,
    exports: Array.isArray(r.exports) ? (r.exports as Composition["exports"]) : b.exports,
  };
}

export function Video({ agentId }: { agentId: string | null }) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [project, setProject] = useState<string | null>(null);
  const [comp, setComp] = useState<Composition>(() => blankComposition());
  const [dirty, setDirty] = useState(false);
  const [playhead, setPlayhead] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [zoom, setZoom] = useState(48); // px per second
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [newSrc, setNewSrc] = useState("");
  const [newTrack, setNewTrack] = useState<TrackId>("V1");
  const [agentPrompt, setAgentPrompt] = useState("");
  const [agentBusy, setAgentBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [ffmpegOk, setFfmpegOk] = useState<boolean | null>(null);
  const playRef = useRef<number | null>(null);

  const totalDur = useMemo(
    () => comp.sequence.reduce((m, c) => Math.max(m, c.end), 8),
    [comp.sequence]
  );
  const selected = useMemo(
    () => comp.sequence.find((c) => c.id === selectedId) ?? null,
    [comp.sequence, selectedId]
  );
  const assets = useMemo(() => {
    const seen = new Map<string, number>();
    for (const c of comp.sequence) if (c.src) seen.set(c.src, (seen.get(c.src) ?? 0) + 1);
    return [...seen.entries()].map(([src, uses]) => ({ src, uses }));
  }, [comp.sequence]);

  async function refreshProjects() {
    if (!agentId) { setProjects([]); return; }
    try {
      const list = await invoke<Project[]>("video_projects", { agentId });
      setProjects(Array.isArray(list) ? list : []);
    } catch (e) { setMsg("✗ video_projects: " + String(e)); }
  }
  async function checkToolchain() {
    try {
      const s = await invoke<{ ffmpeg: string | null }>("video_status");
      setFfmpegOk(!!s?.ffmpeg);
    } catch { setFfmpegOk(null); }
  }

  useEffect(() => {
    refreshProjects(); checkToolchain();
    setProject(null); setComp(blankComposition()); setDirty(false);
    setPlayhead(0); setPlaying(false); setSelectedId(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

  // transport
  useEffect(() => {
    if (!playing) { if (playRef.current) cancelAnimationFrame(playRef.current); playRef.current = null; return; }
    let last = performance.now();
    const tick = (t: number) => {
      const dt = (t - last) / 1000; last = t;
      setPlayhead((p) => (p + dt >= totalDur ? 0 : p + dt));
      playRef.current = requestAnimationFrame(tick);
    };
    playRef.current = requestAnimationFrame(tick);
    return () => { if (playRef.current) cancelAnimationFrame(playRef.current); };
  }, [playing, totalDur]);

  async function open(name: string) {
    if (!agentId) return;
    setMsg(null);
    try {
      const r = await invoke<{ project: string; composition: unknown }>("video_load", { agentId, project: name });
      setProject(r.project);
      setComp(normalizeComposition(r.composition));
      setDirty(false); setPlayhead(0); setSelectedId(null);
    } catch (e) { setMsg("✗ video_load: " + String(e)); }
  }

  async function save(silent = false) {
    if (!agentId || !project) { if (!silent) setMsg("Open or create a project first."); return; }
    try {
      await invoke("video_save", { agentId, project, composition: JSON.parse(JSON.stringify(comp)) });
      setDirty(false);
      if (!silent) setMsg(`saved ✓ Video/${project}/composition.json`);
      refreshProjects();
    } catch (e) { setMsg("✗ video_save: " + String(e)); }
  }

  function mutate(fn: (c: Composition) => Composition) {
    setComp((prev) => fn(JSON.parse(JSON.stringify(prev)) as Composition));
    setDirty(true);
  }
  function patchClip(id: string, patch: Partial<Clip>) {
    mutate((c) => ({ ...c, sequence: c.sequence.map((k) => (k.id === id ? { ...k, ...patch } : k)) }));
  }

  function addClip() {
    const src = newSrc.trim() || `a-roll-${comp.sequence.length + 1}.mp4`;
    const at = playhead;
    const clip: Clip = {
      id: uid(), track: newTrack, src, start: at, end: at + 4,
      sourceIn: 0, sourceOut: 4,
      ...(newTrack === "T1" ? { text: "New title" } : {}),
    };
    mutate((c) => ({ ...c, sequence: [...c.sequence, clip] }));
    setSelectedId(clip.id);
    setNewSrc("");
  }

  function splitAtPlayhead() {
    if (!selected || playhead <= selected.start || playhead >= selected.end) {
      setMsg("Select a clip and park the playhead inside it to split.");
      return;
    }
    const t = playhead;
    const si = selected.sourceIn ?? 0;
    const a: Clip = { ...selected, end: t, sourceOut: si + (t - selected.start) };
    const b: Clip = { ...selected, id: uid(), start: t, sourceIn: si + (t - selected.start) };
    mutate((c) => ({ ...c, sequence: c.sequence.flatMap((k) => (k.id === selected.id ? [a, b] : [k])) }));
    setSelectedId(b.id);
  }

  function deleteSelected() {
    if (!selected) return;
    mutate((c) => ({ ...c, sequence: c.sequence.filter((k) => k.id !== selected.id) }));
    setSelectedId(null);
  }

  function setAspect(w: number, h: number) {
    mutate((c) => ({ ...c, scene: { ...c.scene, width: w, height: h } }));
  }

  // --- timeline drag: move + trim (rate-1 math) ---
  function onClipDrag(e: React.MouseEvent, clip: Clip, mode: "move" | "trimL" | "trimR") {
    e.stopPropagation();
    setSelectedId(clip.id);
    const startX = e.clientX;
    const orig = { ...clip };
    const onMove = (ev: MouseEvent) => {
      const dt = (ev.clientX - startX) / zoom;
      if (mode === "move") {
        const d = orig.end - orig.start;
        const ns = Math.max(0, orig.start + dt);
        patchClip(clip.id, { start: ns, end: ns + d });
      } else if (mode === "trimL") {
        const ns = Math.min(orig.end - 0.1, Math.max(0, orig.start + dt));
        const delta = ns - orig.start;
        patchClip(clip.id, { start: ns, sourceIn: (orig.sourceIn ?? 0) + delta });
      } else {
        const ne = Math.max(orig.start + 0.1, orig.end + dt);
        const delta = ne - orig.end;
        patchClip(clip.id, {
          end: ne,
          sourceOut: orig.sourceOut !== undefined ? orig.sourceOut + delta : undefined,
        });
      }
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  // --- agent edit: the selected agent rewrites composition.json via file tools ---
  async function askAgent() {
    if (!agentId) { setMsg("Select an agent first."); return; }
    if (!project) { setMsg("Open a project first."); return; }
    const task = agentPrompt.trim();
    if (!task) { setMsg("Type what you want the agent to do to this edit."); return; }
    setAgentBusy(true); setMsg(null);
    // persist current UI state first so the agent edits the latest
    try { await invoke("video_save", { agentId, project, composition: JSON.parse(JSON.stringify(comp)) }); setDirty(false); } catch { /* keep going */ }
    const brief =
      `You are editing a video project. The composition is the edit — read it, change it, save it.\n` +
      `Project file: Video/${project}/composition.json (read_file it now; write_file your updated version).\n` +
      `Schema: { scene:{width,height,fps}, sequence:[{id,track:V2|V1|T1|A1|A2, src, start,end, sourceIn,sourceOut, volume,muted,hidden,x,y,width,height,opacity,text}], captions, color{lut,sCurve,adjustmentLayer}, audio{enhance,ducking,musicBed}, exports }.\n` +
      `Timing: start/end = placement on timeline; sourceIn/sourceOut = which part of the source plays (rate 1). Keep ids stable; only add ids for new clips.\n` +
      `User request: ${task}\n` +
      `Rules: make the smallest correct change; keep valid JSON; reply with one line saying what changed (clip ids + times). Do not render — only edit the composition.`;
    try {
      await invoke("agent_stream", {
        channel: `video-${Date.now()}`, prompt: brief, history: [],
        model: null, provider: null, folder: null, sessionId: null,
        agentId, attachments: [],
      });
      const r = await invoke<{ composition: unknown }>("video_load", { agentId, project });
      setComp(normalizeComposition(r.composition));
      setMsg("agent edit applied ✓ — review the timeline, Save Points has the undo.");
    } catch (e) { setMsg("✗ agent edit: " + String(e)); }
    finally { setAgentBusy(false); }
  }

  async function askExport() {
    if (!agentId || !project) { setMsg("Open a project first."); return; }
    const size = comp.scene.width >= comp.scene.height ? "1920x1080" : "1080x1920";
    setAgentPrompt(`Export this project: render ${size} MP4 with a decent bitrate via Pro Mode ffmpeg/HyperFrames from Video/${project}/composition.json and report the output path.`);
    setMsg("Export brief is in the agent box — hit Ask agent.");
  }

  const aspect = comp.scene.width / Math.max(1, comp.scene.height);
  const activeAt = (t: number) => comp.sequence.filter((c) => !c.hidden && t >= c.start && t < c.end);

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12, minHeight: 0 }}>
      {/* top bar */}
      <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
        <h2 style={{ fontSize: 20, fontWeight: 800, margin: 0 }}>Video</h2>
        <select
          value={project ?? ""} onChange={(e) => e.target.value && void open(e.target.value)}
          style={{ fontSize: 13, padding: "7px 10px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--surface)", color: "var(--text)" }}
        >
          <option value="">{agentId ? "Open project…" : "Pick an agent first"}</option>
          {projects.map((p) => <option key={p.name} value={p.name}>{p.name}</option>)}
        </select>
        <Input
          value={project ?? ""} placeholder="new-project-name"
          onChange={(e) => { setProject(e.target.value.trim() || null); setComp(blankComposition()); setDirty(true); }}
          style={{ maxWidth: 200 }}
        />
        <div style={{ display: "flex", border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-control)", overflow: "hidden" }}>
          {[["16:9", 1920, 1080], ["9:16", 1080, 1920], ["1:1", 1080, 1080]].map(([label, w, h]) => {
            const on = comp.scene.width === w && comp.scene.height === h;
            return (
              <button key={label as string} onClick={() => setAspect(w as number, h as number)}
                style={{ padding: "7px 12px", fontSize: 12.5, fontWeight: 700, border: "none", cursor: "pointer", background: on ? "var(--accent)" : "transparent", color: on ? "var(--bg)" : "var(--text-muted)" }}>
                {label}
              </button>
            );
          })}
        </div>
        <span style={{ flex: 1 }} />
        {dirty && <Pill tone="muted">unsaved</Pill>}
        {ffmpegOk === false && <Pill tone="muted">ffmpeg not provisioned</Pill>}
        <Button variant="secondary" onClick={() => void save()} disabled={!project}>Save</Button>
        <Button onClick={() => void askExport()} disabled={!project}>Export…</Button>
      </div>

      {msg && <Pill tone={msg.startsWith("✗") ? "danger" : "muted"}>{msg}</Pill>}
      {!agentId && <Pill tone="muted">Pick an agent — the agent bar drives this edit.</Pill>}

      {/* main: assets | preview+timeline | inspector+agent */}
      <div style={{ display: "grid", gridTemplateColumns: "210px 1fr 300px", gap: 12, minHeight: 0, alignItems: "start" }}>
        {/* assets */}
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          <div style={{ border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)", background: "var(--surface)", padding: 12 }}>
            <div style={{ fontSize: 11, fontWeight: 800, letterSpacing: "0.1em", color: "var(--text-muted)", marginBottom: 8 }}>MEDIA</div>
            {assets.length === 0 && <p style={faint}>No clips yet — add one below.</p>}
            {assets.map((a) => (
              <div key={a.src} style={{ padding: "7px 8px", borderRadius: 6, background: "var(--bg)", border: "var(--border-width) solid var(--line)", marginBottom: 6 }}>
                <div style={{ fontSize: 12, fontWeight: 700, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{a.src}</div>
                <div style={faint}>× {a.uses}</div>
              </div>
            ))}
            <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
              <Input value={newSrc} onChange={(e) => setNewSrc(e.target.value)} placeholder="clip.mp4 or text…" style={{ fontSize: 12 }} />
            </div>
            <div style={{ display: "flex", gap: 6, marginTop: 6 }}>
              <select value={newTrack} onChange={(e) => setNewTrack(e.target.value as TrackId)}
                style={{ flex: 1, fontSize: 12, padding: "7px 8px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)" }}>
                {TRACKS.map((t) => <option key={t.id} value={t.id}>{t.id}</option>)}
              </select>
              <Button onClick={addClip} disabled={!project}>+ Add at ▶</Button>
            </div>
            <p style={{ ...faint, marginTop: 8 }}>Drop A-roll in, then: silence/takes → transcribe → graphics → captions → grade → mix → export.</p>
          </div>
          <div style={{ border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)", background: "var(--surface)", padding: 12 }}>
            <div style={{ fontSize: 11, fontWeight: 800, letterSpacing: "0.1em", color: "var(--text-muted)", marginBottom: 6 }}>PIPELINE</div>
            {[["captions", comp.captions?.preset ?? "hyperframes"], ["lut", comp.color?.lut || "none"], ["enhance", comp.audio?.enhance ?? "auphonic"]].map(([k, v]) => (
              <div key={k} style={{ display: "flex", fontSize: 12, padding: "3px 0" }}>
                <span style={{ color: "var(--text-muted)", width: 70 }}>{k}</span>
                <span style={{ fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis" }}>{v}</span>
              </div>
            ))}
          </div>
        </div>

        {/* preview + timeline */}
        <div style={{ display: "flex", flexDirection: "column", gap: 10, minWidth: 0 }}>
          <div style={{ border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)", background: "#0a0a0a", padding: 16, display: "flex", justifyContent: "center" }}>
            <div style={{
              position: "relative", width: aspect >= 1 ? 460 : 460 * aspect, aspectRatio: `${comp.scene.width} / ${comp.scene.height}`,
              background: comp.scene.background ?? "#000", borderRadius: 8, overflow: "hidden", border: "1px solid #222",
            }}>
              {activeAt(playhead).filter((c) => c.track !== "A1" && c.track !== "A2").map((c) => (
                <div key={c.id} style={{
                  position: "absolute",
                  left: `${((c.x ?? 0) / comp.scene.width) * 100}%`, top: `${((c.y ?? 0) / comp.scene.height) * 100}%`,
                  width: c.width ? `${(c.width / comp.scene.width) * 100}%` : "100%",
                  height: c.height ? `${(c.height / comp.scene.height) * 100}%` : c.track === "T1" ? "auto" : "100%",
                  opacity: c.opacity ?? 1, display: "flex", alignItems: "center", justifyContent: "center",
                  background: c.track === "T1" ? "transparent" : c.track === "V2" ? "rgba(80,120,255,.28)" : "rgba(255,255,255,.08)",
                  border: selectedId === c.id ? "2px solid var(--accent)" : "1px solid rgba(255,255,255,.18)",
                  color: "#fff", fontSize: c.track === "T1" ? 22 : 12, fontWeight: 700, textAlign: "center", padding: 6,
                }}>
                  {c.track === "T1" ? (c.text || c.src || "title") : (c.name || c.src || "clip")}
                </div>
              ))}
              {activeAt(playhead).length === 0 && (
                <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", color: "#666", fontSize: 13 }}>
                  black — nothing scheduled at {playhead.toFixed(2)}s
                </div>
              )}
              <div style={{ position: "absolute", left: 8, bottom: 6, color: "#999", fontSize: 11, fontFamily: "ui-monospace, monospace" }}>
                {playhead.toFixed(2)}s / {totalDur.toFixed(1)}s · {comp.scene.width}×{comp.scene.height}
              </div>
            </div>
          </div>

          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <Button variant="secondary" onClick={() => setPlaying((p) => !p)}>{playing ? "⏸ Pause" : "▶ Play"}</Button>
            <Button variant="secondary" onClick={() => setPlayhead(0)}>⏮</Button>
            <input type="range" min={0} max={Math.max(0.1, totalDur)} step={0.033} value={playhead}
              onChange={(e) => setPlayhead(Number(e.target.value))} style={{ flex: 1 }} />
            <span style={{ ...faint, fontFamily: "ui-monospace, monospace", minWidth: 90, textAlign: "right" }}>{playhead.toFixed(2)}s</span>
            <Button variant="secondary" onClick={() => setZoom((z) => Math.min(160, z + 16))}>+</Button>
            <Button variant="secondary" onClick={() => setZoom((z) => Math.max(16, z - 16))}>−</Button>
          </div>

          <div style={{ border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)", background: "var(--surface)", padding: "10px 10px 14px", overflowX: "auto" }}
            onClick={() => setSelectedId(null)}>
            {TRACKS.map((t) => (
              <div key={t.id} style={{ display: "flex", alignItems: "stretch", marginBottom: 6 }}>
                <div style={{ width: 92, flexShrink: 0, fontSize: 11, fontWeight: 800, color: "var(--text-muted)", paddingTop: 10 }}>{t.label}</div>
                <div style={{ position: "relative", flex: 1, minWidth: Math.max(300, totalDur * zoom), height: 52, background: "var(--bg)", borderRadius: 8, border: "var(--border-width) solid var(--line)", overflow: "hidden" }}>
                  {comp.sequence.filter((c) => c.track === t.id).map((c) => {
                    const sel = c.id === selectedId;
                    return (
                      <div key={c.id}
                        onMouseDown={(e) => onClipDrag(e, c, "move")}
                        onClick={(e) => { e.stopPropagation(); setSelectedId(c.id); }}
                        title={`${c.src} · ${c.start.toFixed(2)}→${c.end.toFixed(2)} (in ${c.sourceIn ?? 0})`}
                        style={{
                          position: "absolute", left: c.start * zoom, width: Math.max(8, dur(c) * zoom), top: 6, bottom: 6,
                          borderRadius: 6, padding: "4px 6px", fontSize: 11, fontWeight: 600, overflow: "hidden", whiteSpace: "nowrap",
                          textOverflow: "ellipsis", cursor: "grab", userSelect: "none",
                          background: t.kind === "audio" ? "rgba(46,160,90,.28)" : t.id === "T1" ? "rgba(190,120,40,.32)" : t.id === "V2" ? "rgba(80,120,255,.30)" : "rgba(120,120,140,.30)",
                          border: sel ? "2px solid var(--accent)" : "1px solid rgba(255,255,255,.16)",
                          opacity: c.hidden ? 0.35 : 1, color: "var(--text)",
                        }}>
                        {/* trim handles */}
                        <span onMouseDown={(e) => onClipDrag(e, c, "trimL")}
                          style={{ position: "absolute", left: 0, top: 0, bottom: 0, width: 8, cursor: "ew-resize", background: "rgba(255,255,255,.18)", borderRadius: "6px 0 0 6px" }} />
                        <span onMouseDown={(e) => onClipDrag(e, c, "trimR")}
                          style={{ position: "absolute", right: 0, top: 0, bottom: 0, width: 8, cursor: "ew-resize", background: "rgba(255,255,255,.18)", borderRadius: "0 6px 6px 0" }} />
                        <span style={{ padding: "0 10px" }}>{c.muted ? "🔇 " : ""}{c.name || c.src || "(empty)"}</span>
                      </div>
                    );
                  })}
                  {/* playhead */}
                  <div style={{ position: "absolute", left: playhead * zoom, top: 0, bottom: 0, width: 2, background: "var(--accent)", pointerEvents: "none" }} />
                </div>
              </div>
            ))}
            <div style={{ display: "flex", gap: 8, marginTop: 4, flexWrap: "wrap" }}>
              <Button variant="secondary" onClick={splitAtPlayhead} disabled={!selected}>✂ Split at ▶</Button>
              <Button variant="secondary" onClick={deleteSelected} disabled={!selected}>Delete clip</Button>
              <span style={faint}>drag body = move · drag edges = trim (sourceIn/out follow) · click empty = deselect</span>
            </div>
          </div>
        </div>

        {/* inspector + agent */}
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          <div style={{ border: "var(--border-width) solid var(--line)", borderRadius: "var(--radius-card)", background: "var(--surface)", padding: 12 }}>
            <div style={{ fontSize: 11, fontWeight: 800, letterSpacing: "0.1em", color: "var(--text-muted)", marginBottom: 8 }}>INSPECTOR</div>
            {!selected ? <p style={faint}>Select a clip on the timeline.</p> : (
              <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                <label style={faint}>label<Input value={selected.name ?? ""} onChange={(e) => patchClip(selected.id, { name: e.target.value })} /></label>
                <label style={faint}>src<Input value={selected.src} onChange={(e) => patchClip(selected.id, { src: e.target.value })} /></label>
                {selected.track === "T1" && (
                  <label style={faint}>text<Input value={selected.text ?? ""} onChange={(e) => patchClip(selected.id, { text: e.target.value })} /></label>
                )}
                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
                  {[["start", selected.start], ["end", selected.end], ["in", selected.sourceIn ?? 0], ["out", selected.sourceOut ?? selected.end - selected.start]].map(([k, v]) => (
                    <label key={k as string} style={faint}>{k}
                      <Input type="number" step="0.033" value={Number(v).toFixed(2)}
                        onChange={(e) => {
                          const n = Number(e.target.value);
                          if (!isFinite(n)) return;
                          if (k === "start") patchClip(selected.id, { start: n });
                          else if (k === "end") patchClip(selected.id, { end: n });
                          else if (k === "in") patchClip(selected.id, { sourceIn: n });
                          else patchClip(selected.id, { sourceOut: n });
                        }} />
                    </label>
                  ))}
                </div>
                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
                  <label style={faint}>track
                    <select value={selected.track} onChange={(e) => patchClip(selected.id, { track: e.target.value as TrackId })}
                      style={{ width: "100%", fontSize: 13, padding: "8px", borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)" }}>
                      {TRACKS.map((t) => <option key={t.id} value={t.id}>{t.id}</option>)}
                    </select>
                  </label>
                  <label style={faint}>vol dB
                    <Input type="number" step="1" value={selected.volume ?? 0}
                      onChange={(e) => patchClip(selected.id, { volume: Number(e.target.value) || 0 })} />
                  </label>
                </div>
                <div style={{ display: "flex", gap: 12, fontSize: 12.5 }}>
                  <label style={{ display: "flex", gap: 6, alignItems: "center" }}>
                    <input type="checkbox" checked={!!selected.muted} onChange={(e) => patchClip(selected.id, { muted: e.target.checked })} /> muted
                  </label>
                  <label style={{ display: "flex", gap: 6, alignItems: "center" }}>
                    <input type="checkbox" checked={!!selected.hidden} onChange={(e) => patchClip(selected.id, { hidden: e.target.checked })} /> hidden
                  </label>
                </div>
                <p style={faint}>dur {dur(selected).toFixed(2)}s · id {selected.id}</p>
              </div>
            )}
          </div>

          <div style={{ border: "2px solid var(--accent)", borderRadius: "var(--radius-card)", background: "var(--surface)", padding: 12, display: "flex", flexDirection: "column", gap: 8 }}>
            <div style={{ fontSize: 11, fontWeight: 800, letterSpacing: "0.1em" }}>✨ AGENT — {agentId ? "editing as this agent" : "pick an agent"}</div>
            <p style={faint}>Prompts edit the same composition.json you see. Manual timeline edits + agent edits are the same file.</p>
            <textarea value={agentPrompt} onChange={(e) => setAgentPrompt(e.target.value)} rows={4}
              placeholder="e.g. drop silences over 0.6s, keep the last take when lines repeat, add animated captions, reframe a 9:16 cut…"
              style={{ width: "100%", fontSize: 13, padding: 10, borderRadius: "var(--radius-control)", border: "var(--border-width) solid var(--line)", background: "var(--bg)", color: "var(--text)", resize: "vertical" }} />
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              <Button onClick={() => void askAgent()} disabled={agentBusy || !project}>
                {agentBusy ? "Agent working…" : "Ask agent to edit"}
              </Button>
              <Button variant="secondary" onClick={() => setAgentPrompt("Drop silences and dead space over 0.5s. When takes repeat, keep the last one or the longest coherent pass. Return tight selects on V1.")}>silence+take</Button>
              <Button variant="secondary" onClick={() => setAgentPrompt("Transcribe the V1 selects word-level into transcript.json, then propose 3 graphics overlays with timestamps for what needs extra explanation.")}>transcribe</Button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
