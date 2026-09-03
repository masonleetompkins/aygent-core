// AYGENT — VIDEO v0.3 editor shell. Full-bleed NLE layout:
//   top bar · tool rail | panel | player + timeline | dock (Agent / Inspector)
// Keyboard: Space play · J/K/L · ←/→ frame · Home/End · K or ⌘K split · ⌫ delete
//           ⇧⌫ ripple delete · ⌘D duplicate · ⌘Z/⇧⌘Z undo/redo · V select · C razor
//           S snap · +/− zoom · ⇧Z fit · I inspector · ⌘⏎ focus agent · ⌘S save
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Film, Type, Captions, Palette, AudioLines, Download, PanelLeftClose, PanelLeft, PanelRightClose, PanelRight, Plus, FolderOpen, Save } from "lucide-react";
import "./video.css";
import { useVideo, init, open, create, save, set, get, togglePlay, seek, stepFrames, splitAt, deleteSelected, duplicateSelected, undo, redo, importPaths, reload, refreshProjects, reveal, type Panel } from "./store";
import { duration as durOf } from "./model";
import { Player } from "./Player";
import { Timeline } from "./Timeline";
import { Inspector } from "./Inspector";
import { AgentDock } from "./AgentDock";
import { MediaPanel, TextPanel, CaptionsPanel, ColorPanel, AudioPanel, ExportPanel } from "./Panels";

const RAIL: { id: Panel; l: string; I: typeof Film }[] = [
  { id: "media", l: "Media", I: Film }, { id: "text", l: "Text", I: Type }, { id: "captions", l: "Caps", I: Captions },
  { id: "color", l: "Color", I: Palette }, { id: "audio", l: "Audio", I: AudioLines }, { id: "export", l: "Export", I: Download },
];

export function Editor({ agentId, agentName, folder }: { agentId: string | null; agentName?: string; folder: string | null }) {
  const s = useVideo();
  const rootRef = useRef<HTMLDivElement>(null);
  const [dropping, setDropping] = useState(false);
  const [newName, setNewName] = useState("");
  const [showNew, setShowNew] = useState(false);

  useEffect(() => { void init(agentId, folder); }, [agentId, folder]);

  // backend events: agent tools changed the project · render progress · frames
  useEffect(() => {
    const un: Promise<() => void>[] = [
      listen<{ project: string }>("video-project-changed", (e) => { if (e.payload.project === get().project) void reload(); }),
      listen<{ project: string }>("video-assets-changed", (e) => { if (e.payload.project === get().project) void reload(); }),
      listen<{ project: string; pct?: number; done?: boolean; path?: string; error?: string }>("video-render-progress", (e) => {
        if (e.payload.project !== get().project) return;
        const cur = get().render;
        if (e.payload.error) set({ render: { pct: 0, running: false, error: e.payload.error } });
        else if (e.payload.done) set({ render: { pct: 100, running: false, path: e.payload.path } });
        else set({ render: { pct: e.payload.pct ?? cur?.pct ?? 0, running: true } });
      }),
      listen<{ project: string; path: string; time: number }>("video-frame-ready", (e) => { if (e.payload.project === get().project) { set({ frame: { path: e.payload.path, time: e.payload.time, at: Date.now() } }); seek(e.payload.time); } }),
      listen<{ project: string; msg: string }>("video-tool-progress", (e) => { if (e.payload.project === get().project) set({ toolProgress: e.payload.msg }); }),
    ];
    return () => { un.forEach((p) => p.then((f) => f())); };
  }, []);

  // Finder drag-drop → hardlink import (Tauri native drop events carry real paths)
  useEffect(() => {
    let un: (() => void) | null = null;
    getCurrentWebview().onDragDropEvent((ev) => {
      const t = ev.payload.type;
      if (t === "enter" || t === "over") setDropping(true);
      else if (t === "leave") setDropping(false);
      else if (t === "drop") {
        setDropping(false);
        const paths = (ev.payload as { paths: string[] }).paths ?? [];
        if (paths.length && get().project && rootRef.current && document.body.contains(rootRef.current)) void importPaths(paths);
      }
    }).then((f) => { un = f; }).catch(() => {});
    return () => { if (un) un(); };
  }, []);

  // keyboard
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tgt = e.target as HTMLElement;
      const typing = tgt && (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.tagName === "SELECT" || tgt.isContentEditable);
      const meta = e.metaKey || e.ctrlKey;
      if (meta && e.key.toLowerCase() === "s") { e.preventDefault(); void save().then((ok) => ok && set({ toast: { text: "saved", tone: "ok", at: Date.now() } })); return; }
      if (meta && e.key === "Enter") { e.preventDefault(); set({ dockOpen: true, dockTab: "agent" }); requestAnimationFrame(() => rootRef.current?.querySelector<HTMLTextAreaElement>(".ve-compose textarea")?.focus()); return; }
      if (typing) return;
      if (meta && e.key.toLowerCase() === "z") { e.preventDefault(); if (e.shiftKey) redo(); else undo(); return; }
      if (meta && e.key.toLowerCase() === "d") { e.preventDefault(); duplicateSelected(); return; }
      if (meta && e.key.toLowerCase() === "k") { e.preventDefault(); splitAt(get().playhead); return; }
      if (meta) return;
      const st = get(); const fps = st.comp.scene.fps || 30;
      switch (e.key) {
        case " ": e.preventDefault(); togglePlay(); break;
        case "k": case "K": if (st.playing) set({ playing: false }); else splitAt(st.playhead); break;
        case "j": case "J": set({ playing: false }); seek(st.playhead - 1); break;
        case "l": case "L": set({ playing: false }); seek(st.playhead + 1); break;
        case "ArrowLeft": e.preventDefault(); stepFrames(e.shiftKey ? -fps : -1); break;
        case "ArrowRight": e.preventDefault(); stepFrames(e.shiftKey ? fps : 1); break;
        case "Home": seek(0); break;
        case "End": seek(durOf(st.comp)); break;
        case "Backspace": case "Delete": e.preventDefault(); deleteSelected(e.shiftKey); break;
        case "v": case "V": set({ tool: "select" }); break;
        case "c": case "C": set({ tool: "razor" }); break;
        case "s": case "S": set({ snap: !st.snap }); break;
        case "i": case "I": set({ dockOpen: true, dockTab: st.dockTab === "inspector" && st.dockOpen ? "agent" : "inspector" }); break;
        case "=": case "+": set({ zoom: Math.min(600, st.zoom * 1.3) }); break;
        case "-": case "_": set({ zoom: Math.max(6, st.zoom / 1.3) }); break;
        case "Z": { const el = rootRef.current?.querySelector<HTMLElement>(".ve-lanes"); if (el) set({ zoom: Math.max(6, (el.clientWidth - 40) / Math.max(1, durOf(st.comp))) }); break; }
        case "Escape": set({ selection: [] }); break;
        case "[": { const c = st.comp.clips.filter((k) => k.end <= st.playhead - 1e-3).map((k) => k.end); if (c.length) seek(Math.max(...c)); else seek(0); break; }
        case "]": { const c = st.comp.clips.filter((k) => k.start >= st.playhead + 1e-3).map((k) => k.start); if (c.length) seek(Math.min(...c)); break; }
        default: return;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const PanelBody = { media: MediaPanel, text: TextPanel, captions: CaptionsPanel, color: ColorPanel, audio: AudioPanel, export: ExportPanel }[s.panel];
  const noProject = !s.project;

  return (
    <div ref={rootRef} className={`ve ${dropping ? "dropping" : ""}`} style={noProject ? { gridTemplateAreas: '"top top top top" "rail panel center dock"' } : undefined}>
      {/* top bar */}
      <div className="ve-top">
        <span className="brand"><Film size={15} /> Video</span>
        <div className="proj">
          <select value={s.project ?? ""} onChange={(e) => { if (e.target.value === "__new") { setShowNew(true); } else if (e.target.value) void open(e.target.value); }} disabled={!agentId}>
            <option value="" disabled>{agentId ? "Open project…" : "Pick an agent"}</option>
            {s.projects.map((p) => <option key={p.name} value={p.name}>{p.name}</option>)}
            <option value="__new">＋ New project…</option>
          </select>
          {showNew && (
            <form style={{ display: "flex", gap: 6 }} onSubmit={(e) => { e.preventDefault(); const n = newName.trim().replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 80); if (n) { void create(n); setShowNew(false); setNewName(""); } }}>
              <input autoFocus value={newName} placeholder="project-name" onChange={(e) => setNewName(e.target.value)} onKeyDown={(e) => e.key === "Escape" && setShowNew(false)} style={{ width: 160, height: 30 }} />
              <button className="ve-btn sm primary" type="submit"><Plus size={12} /> Create</button>
              <button className="ve-btn sm" type="button" onClick={() => setShowNew(false)}>Cancel</button>
            </form>
          )}
          {s.project && <button className="ve-icon-btn" title="Reveal project folder" onClick={() => void reveal()}><FolderOpen size={14} /></button>}
        </div>
        <span className="spacer" />
        {s.status && !s.status.ffmpeg && <span className="ve-pill err">ffmpeg missing — install HyperFrames in Tools</span>}
        {s.render?.running && <span className="ve-pill"><span className="ve-spinner" /> export {s.render.pct}%</span>}
        <span className="savestate">{s.saving ? "saving…" : s.dirty ? "unsaved" : s.project ? "saved" : ""}</span>
        <button className="ve-icon-btn" title="Save (⌘S)" disabled={!s.dirty} onClick={() => void save()}><Save size={15} /></button>
        <button className="ve-icon-btn" title={s.panelOpen ? "Hide panel" : "Show panel"} onClick={() => set({ panelOpen: !s.panelOpen })}>{s.panelOpen ? <PanelLeftClose size={15} /> : <PanelLeft size={15} />}</button>
        <button className="ve-icon-btn" title={s.dockOpen ? "Hide agent/inspector" : "Show agent/inspector"} onClick={() => set({ dockOpen: !s.dockOpen })}>{s.dockOpen ? <PanelRightClose size={15} /> : <PanelRight size={15} />}</button>
      </div>

      {/* rail */}
      <div className="ve-rail">
        {RAIL.map(({ id, l, I }) => <button key={id} className={s.panel === id && s.panelOpen ? "on" : ""} title={l} onClick={() => set({ panel: id, panelOpen: s.panel === id ? !s.panelOpen : true })}><I size={17} /><span>{l}</span></button>)}
        <span className="grow" />
      </div>

      {noProject ? (
        <div className="ve-welcome">
          <div className="card">
            <h2>{agentId ? "Start a video project" : "Pick an agent"}</h2>
            <p className="ve-hint" style={{ fontSize: 13 }}>A project is a folder under <b>Video/</b> in the agent's workspace: the edit (composition.json), hardlinked media, transcript, and the agent conversation. Your agent edits the same file you do.</p>
            {agentId && (
              <form style={{ display: "flex", gap: 8 }} onSubmit={(e) => { e.preventDefault(); const n = newName.trim().replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 80); if (n) { void create(n); setNewName(""); } }}>
                <input autoFocus value={newName} placeholder="project-name" onChange={(e) => setNewName(e.target.value)} style={{ flex: 1, height: 34 }} />
                <button className="ve-btn primary" type="submit" style={{ height: 34 }}><Plus size={14} /> Create</button>
              </form>
            )}
            {s.projects.length > 0 && <div className="recent">{s.projects.map((p) => <button key={p.name} onClick={() => void open(p.name)}>{p.name}<span>{p.assets} media · {new Date(p.modified * 1000).toLocaleDateString()}</span></button>)}</div>}
            {agentId && <button className="ve-btn sm" style={{ alignSelf: "flex-start" }} onClick={() => void refreshProjects()}>Refresh list</button>}
          </div>
        </div>
      ) : (
        <>
          <div className={`ve-panel ${s.panelOpen ? "" : "closed"}`}><PanelBody /></div>
          <div className="ve-center">
            <Player />
            <Timeline />
          </div>
          <div className={`ve-dock ${s.dockOpen ? "" : "closed"}`}>
            <div className="ve-dock-tabs">
              <button className={s.dockTab === "agent" ? "on" : ""} onClick={() => set({ dockTab: "agent" })}>Agent</button>
              <button className={s.dockTab === "inspector" ? "on" : ""} onClick={() => set({ dockTab: "inspector" })}>Inspector{s.selection.length ? ` · ${s.selection.length}` : ""}</button>
              <span className="spacer" />
            </div>
            {s.dockTab === "agent" ? <AgentDock agentName={agentName} /> : <Inspector />}
          </div>
        </>
      )}

      {s.toast && <div className={`ve-toast ${s.toast.tone}`} onClick={() => set({ toast: null })}>{s.toast.text}</div>}
    </div>
  );
}
