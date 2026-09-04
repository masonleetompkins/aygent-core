// AYGENT — VIDEO v0.3 player. Real footage plays through the jailed
// aygent-media:// scheme: one <video>/<audio> element per active clip, kept in
// sync with the playhead (seek on scrub, play/pause on transport). Text clips
// and captions are drawn live as DOM so the look updates as you type; the
// exact ffmpeg composite (grade + LUT + ASS captions) is one click away via
// "Render frame" and shows as an overlay badge.
import { useEffect, useMemo, useRef, useState } from "react";
import { Play, Pause, SkipBack, SkipForward, ChevronLeft, ChevronRight, Repeat, Camera, Maximize2, Volume2, VolumeX, Proportions, X } from "lucide-react";
import { useVideo, seek, togglePlay, stepFrames, set, get, renderFrame, mutate } from "./store";
import { Num } from "./Panels";
import { duration as durOf } from "./model";
import { type Clip, TRACK_KIND, fmtTime, mediaUrl } from "./model";

const rank = (t: string) => (t.startsWith("V") ? Number(t.slice(1)) : t.startsWith("T") ? 100 + Number(t.slice(1)) : 50);

function useStageSize(ref: React.RefObject<HTMLDivElement>) {
  const [sz, setSz] = useState({ w: 640, h: 360 });
  useEffect(() => {
    const el = ref.current; if (!el) return;
    const ro = new ResizeObserver(() => setSz({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el); setSz({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, [ref]);
  return sz;
}

function MediaLayer({ clip, agentId, project, playhead, playing, sceneW, sceneH, scale, muted, selected }: {
  clip: Clip; agentId: string; project: string; playhead: number; playing: boolean; sceneW: number; sceneH: number; scale: number; muted: boolean; selected: boolean;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const url = useMemo(() => mediaUrl(agentId, project, "asset", clip.asset), [agentId, project, clip.asset]);
  const srcTime = clip.in + (playhead - clip.start) * clip.speed;
  // sync: when paused or after a seek, set currentTime; when playing, keep drift < 80ms
  useEffect(() => {
    const v = ref.current; if (!v) return;
    v.playbackRate = clip.speed;
    v.muted = muted || clip.muted;
    v.volume = Math.min(1, Math.pow(10, clip.volume / 20));
    if (!playing) { v.pause(); if (Math.abs(v.currentTime - srcTime) > 0.04) v.currentTime = srcTime; return; }
    if (Math.abs(v.currentTime - srcTime) > 0.12) v.currentTime = srcTime;
    if (v.paused) void v.play().catch(() => {});
  }, [playing, srcTime, clip.speed, clip.volume, clip.muted, muted]);
  useEffect(() => () => { ref.current?.pause(); }, []);
  const isImg = clip.type === "image";
  const fit = clip.fit === "contain" ? "contain" : clip.fit === "none" ? "none" : "cover";
  const style: React.CSSProperties = {
    width: sceneW * scale, height: sceneH * scale, objectFit: fit as any,
    transform: `translate(-50%,-50%) translate(${clip.transform.x * scale}px, ${clip.transform.y * scale}px) scale(${clip.transform.scale}) rotate(${clip.transform.rotation}deg)`,
    opacity: clip.transform.opacity, filter: gradeFilter(clip.color),
    outline: selected ? "1.5px solid var(--accent)" : undefined,
  };
  if (isImg) return <img className="layer" src={url} style={style} draggable={false} alt="" />;
  if (clip.type === "audio") return <audio ref={ref as any} src={url} preload="auto" />;
  return <video ref={ref} src={url} style={style} preload="auto" playsInline />;
}

function gradeFilter(g: Clip["color"]): string | undefined {
  const f: string[] = [];
  if (g.exposure) f.push(`brightness(${(1 + g.exposure * 0.5).toFixed(3)})`);
  if (g.contrast) f.push(`contrast(${(1 + g.contrast * 0.6).toFixed(3)})`);
  if (g.saturation) f.push(`saturate(${Math.max(0, 1 + g.saturation).toFixed(3)})`);
  if (g.temperature) f.push(`sepia(${Math.abs(g.temperature * 0.25).toFixed(3)})`);
  return f.length ? f.join(" ") : undefined;
}

export function Player() {
  const s = useVideo();
  const stageRef = useRef<HTMLDivElement>(null);
  const size = useStageSize(stageRef);
  const [muted, setMuted] = useState(false);
  const [safe, setSafe] = useState(false);
  const [sceneOpen, setSceneOpen] = useState(false);
  const { comp, playhead, playing, agentId, project } = s;
  const total = durOf(comp);
  const sceneW = comp.scene.width, sceneH = comp.scene.height;
  const fitScale = Math.min((size.w - 8) / sceneW, (size.h - 8) / sceneH, 1.6);
  const scale = Math.max(0.05, fitScale);

  // transport clock
  const raf = useRef<number | null>(null);
  useEffect(() => {
    if (!playing) { if (raf.current) cancelAnimationFrame(raf.current); raf.current = null; return; }
    let last = performance.now();
    const tick = (t: number) => {
      const dt = (t - last) / 1000; last = t;
      const st = get();
      const nxt = st.playhead + dt;
      if (nxt >= Math.max(total, 0.01)) { if (st.loop) seek(0); else { set({ playing: false }); seek(total); return; } }
      else set({ playhead: nxt });
      raf.current = requestAnimationFrame(tick);
    };
    raf.current = requestAnimationFrame(tick);
    return () => { if (raf.current) cancelAnimationFrame(raf.current); };
  }, [playing, total]);

  const active = useMemo(() => comp.clips.filter((c) => !c.hidden && playhead >= c.start && playhead < c.end).sort((a, b) => rank(a.track) - rank(b.track)), [comp.clips, playhead]);
  const visual = active.filter((c) => TRACK_KIND(c.track) !== "audio" && c.type !== "audio");
  const audioOnly = active.filter((c) => c.type === "audio" && !c.muted);
  const caption = s.comp.captions.enabled ? s.captionLines.find((l) => playhead >= l.s && playhead < l.e) : undefined;
  const showFrame = s.frame && Math.abs(s.frame.time - playhead) < 0.02;
  const fps = comp.scene.fps || 30;

  return (
    <div className="ve-player">
      <div className="ve-stage" ref={stageRef} onClick={() => set({ selection: [] })}>
        <div className="ve-canvas" style={{ width: sceneW * scale, height: sceneH * scale, background: comp.scene.background }} onClick={(e) => e.stopPropagation()}>
          {agentId && project && visual.map((c) => c.type === "text" ? (
            <div key={c.id} className="txt" style={{
              left: `${c.text.x * 100}%`, top: `${c.text.y * 100}%`, maxWidth: c.text.maxWidth > 0 ? `${c.text.maxWidth * 100}%` : undefined,
              fontFamily: `"${c.text.font}", -apple-system, system-ui, sans-serif`, fontSize: c.text.size * scale, fontWeight: c.text.weight, color: c.text.color,
              textAlign: c.text.align, textShadow: c.text.bg ? undefined : c.text.shadow ? `0 ${c.text.size * scale * 0.04}px ${c.text.size * scale * 0.12}px rgba(0,0,0,.6)` : undefined,
              background: c.text.bg ? hexA(c.text.bg, c.text.bgOpacity) : undefined, padding: c.text.bg ? c.text.padding * scale : 0, borderRadius: c.text.bg ? 8 * scale : 0,
              opacity: fadeAlpha(c, playhead), outline: s.selection.includes(c.id) ? "1.5px solid var(--accent)" : undefined,
            }}>{c.text.content}</div>
          ) : (
            <MediaLayer key={c.id} clip={c} agentId={agentId} project={project} playhead={playhead} playing={playing} sceneW={sceneW} sceneH={sceneH} scale={scale} muted={muted} selected={s.selection.includes(c.id)} />
          ))}
          {agentId && project && audioOnly.map((c) => <MediaLayer key={c.id} clip={c} agentId={agentId} project={project} playhead={playhead} playing={playing} sceneW={0} sceneH={0} scale={1} muted={muted} selected={false} />)}
          {caption && (
            <div className="cap" style={{ top: `${comp.captions.y * 100}%`, fontFamily: `"${comp.captions.font}", -apple-system, system-ui, sans-serif`, fontSize: comp.captions.size * scale, fontWeight: comp.captions.weight, color: comp.captions.color, textShadow: comp.captions.shadow ? `0 ${3 * scale}px ${16 * scale}px rgba(0,0,0,.65)` : undefined }}>
              {caption.words.map((w, i) => {
                const on = playhead >= w.s - 0.01;
                const key = comp.captions.keyWords.some((k) => k.toLowerCase() === w.w.toLowerCase().replace(/[^\p{L}\p{N}']/gu, ""));
                const pop = comp.captions.preset === "pop";
                return <span key={i} className="w" style={{ opacity: pop ? (on ? 1 : 0) : 1, transform: pop ? (on ? "none" : "translateY(8px) scale(.86)") : undefined, transition: "opacity 120ms, transform 160ms cubic-bezier(.2,1.6,.4,1)", color: key || (comp.captions.preset === "karaoke" && on) ? comp.captions.keyColor : undefined, textShadow: key ? `0 0 ${20 * scale}px ${comp.captions.keyColor}` : undefined }}>{comp.captions.uppercase ? w.w.toUpperCase() : w.w}</span>;
              })}
            </div>
          )}
          {showFrame && s.frame && agentId && project && <img className="layer" src={mediaUrl(agentId, project, "cache", s.frame.path, s.frame.at)} style={{ width: "100%", height: "100%", objectFit: "contain" }} alt="" />}
          {showFrame && <div className="frame-badge">FFMPEG FRAME</div>}
          {safe && <div className="safe" />}
          {visual.length === 0 && !showFrame && <div className="empty">{comp.clips.length ? `nothing at ${fmtTime(playhead, fps)}` : "drop footage into the timeline"}</div>}
        </div>
        {s.toolProgress && <div className="ve-pill" style={{ position: "absolute", top: 20, left: "50%", transform: "translateX(-50%)", background: "var(--surface)" }}><span className="ve-spinner" />{s.toolProgress}</div>}
      </div>
      <div className="ve-transport">
        <span className="tc ve-mono">{fmtTime(playhead, fps)} <span className="dim">/ {fmtTime(total, fps)}</span></span>
        <span className="spacer" />
        <button className="ve-icon-btn" title="Start (Home)" onClick={() => seek(0)}><SkipBack size={16} /></button>
        <button className="ve-icon-btn" title="Back 1 frame (←)" onClick={() => stepFrames(-1)}><ChevronLeft size={16} /></button>
        <button className="ve-icon-btn on" style={{ width: 36, height: 36 }} title="Play/Pause (Space)" onClick={togglePlay}>{playing ? <Pause size={18} /> : <Play size={18} />}</button>
        <button className="ve-icon-btn" title="Forward 1 frame (→)" onClick={() => stepFrames(1)}><ChevronRight size={16} /></button>
        <button className="ve-icon-btn" title="End (End)" onClick={() => seek(total)}><SkipForward size={16} /></button>
        <button className={`ve-icon-btn ${s.loop ? "on" : ""}`} title="Loop" onClick={() => set({ loop: !s.loop })}><Repeat size={15} /></button>
        <span className="spacer" />
        <button className={`ve-icon-btn ${muted ? "on" : ""}`} title="Mute preview" onClick={() => setMuted(!muted)}>{muted ? <VolumeX size={15} /> : <Volume2 size={15} />}</button>
        <button className={`ve-icon-btn ${safe ? "on" : ""}`} title="Safe margins" onClick={() => setSafe(!safe)}><Maximize2 size={15} /></button>
        <button className="ve-btn sm" title="Render this frame with the real ffmpeg pipeline (grade, LUT, captions)" onClick={() => void renderFrame()}><Camera size={13} /> Render frame</button>
        <button className={`scene-btn ve-mono ${sceneOpen ? "on" : ""}`} title="Canvas size · frame rate" onMouseDown={(e) => e.stopPropagation()} onClick={() => setSceneOpen(!sceneOpen)}><Proportions size={13} />{sceneW}×{sceneH} · {fps}fps</button>
      </div>
      {sceneOpen && <ScenePopover onClose={() => setSceneOpen(false)} />}
    </div>
  );
}

const SCENE_PRESETS: [string, number, number][] = [["16:9", 1920, 1080], ["16:9 4K", 3840, 2160], ["9:16", 1080, 1920], ["1:1", 1080, 1080], ["4:5", 1080, 1350], ["21:9", 2560, 1080]];
const even = (v: number) => Math.max(16, Math.round(v / 2) * 2);

/** Canvas size + fps. Exact pixel fields (any size; snapped to even for yuv420p encoders) plus quick presets. */
function ScenePopover({ onClose }: { onClose: () => void }) {
  const s = useVideo(); const sc = s.comp.scene;
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => { if (ref.current && !ref.current.contains(e.target as Node)) onClose(); };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("mousedown", onDown); window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("mousedown", onDown); window.removeEventListener("keydown", onKey); };
  }, [onClose]);
  const setSize = (w: number, h: number) => mutate((c) => { c.scene.width = even(w); c.scene.height = even(h); });
  return (
    <div className="ve-scene-pop" ref={ref}>
      <h4>Canvas<button className="ve-icon-btn" style={{ width: 22, height: 22 }} onClick={onClose}><X size={12} /></button></h4>
      <div className="ve-row3">
        <Num label="Width" value={sc.width} step={2} min={16} max={8192} onChange={(v) => setSize(v, sc.height)} />
        <Num label="Height" value={sc.height} step={2} min={16} max={8192} onChange={(v) => setSize(sc.width, v)} />
        <Num label="FPS" value={sc.fps} step={1} min={1} max={120} onChange={(v) => mutate((c) => { c.scene.fps = v; })} />
      </div>
      <div className="presets">
        {SCENE_PRESETS.map(([l, w, h]) => <button key={l} className={w === sc.width && h === sc.height ? "on" : ""} onClick={() => setSize(w, h)}>{l}<span>{w}×{h}</span></button>)}
      </div>
      <p className="ve-hint">Any size works; odd values snap to even (H.264/HEVC need it). Exports have their own size in the Export panel.</p>
    </div>
  );
}

function hexA(hex: string, a: number) { const h = hex.replace("#", ""); if (h.length !== 6) return hex; return `rgba(${parseInt(h.slice(0, 2), 16)},${parseInt(h.slice(2, 4), 16)},${parseInt(h.slice(4, 6), 16)},${a})`; }
function fadeAlpha(c: Clip, t: number) { if (c.text.animation !== "fade") return 1; const f = Math.min(0.25, (c.end - c.start) / 3); const a = Math.min(1, (t - c.start) / f, (c.end - t) / f); return Math.max(0, a); }
