// AYGENT — VIDEO v0.3 player. Real footage plays through the jailed
// aygent-media:// scheme.
//
// Smooth, gapless playback design:
//  · Media elements are PERSISTENT and pooled per asset (2–3 slots): every clip is
//    assigned a slot whose previous clip ended ≥ LEAD seconds earlier, so the next
//    clip's element is free to PRE-ROLL: it starts playing hidden + muted LEAD s
//    before the cut, already decoding in motion. The cut is a visibility swap +
//    unmute — no mount, no load, no seek, no decoder spin-up (that spin-up was
//    the black flash in v0.3 and the ~1 s freeze after the first fix).
//  · The WALL CLOCK is the master while playing (like an NLE): the playhead never
//    waits on a decoder. Every element chases the playhead with a small
//    playbackRate nudge (±6 %); a hard seek only on real drift (>250 ms visible,
//    >60 ms while still hidden in pre-roll where a seek costs nothing).
//  · The transport tick publishes on the store's narrow playhead channel
//    (tickPlayhead/usePlayhead); only the stage, timecode and playhead lines
//    re-render per frame — the timeline/inspector/dock stay idle.
// Text clips and captions are drawn live as DOM; "Render frame" shows the exact
// ffmpeg composite (grade + LUT + ASS captions) as an overlay badge.
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Play, Pause, SkipBack, SkipForward, ChevronLeft, ChevronRight, Repeat, Camera, Maximize2, Volume2, VolumeX, Proportions, X } from "lucide-react";
import { useVideo, usePlayhead, tickPlayhead, seek, togglePlay, stepFrames, set, get, renderFrame, mutate } from "./store";
import { Num, volumeOf } from "./Panels";
import { Unplug } from "lucide-react";
import { duration as durOf } from "./model";
import { type Clip, type Composition, TRACK_KIND, fmtTime, mediaUrl } from "./model";

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

// ---- slot assignment --------------------------------------------------------
// Greedy over clips sorted by start. A clip takes the slot of its asset that has
// been free the longest; if that slot freed up less than LEAD s before the clip
// starts (no room to pre-roll) and we have < 3 slots for this asset, open another.
const LEAD = 1.0;        // s of hidden pre-roll before a cut
const NUDGE_MAX = 0.06;  // max playbackRate deviation used to chase the clock
type Slot = { id: string; asset: string; kind: "video" | "audio" };
function assignSlots(comp: Composition, assetKind: (id: string) => "video" | "audio" | "image" | undefined): { slots: Slot[]; slotOf: Map<string, string> } {
  const slots: Slot[] = []; const slotOf = new Map<string, string>();
  const lastEnd = new Map<string, number>();
  const media = comp.clips.filter((c) => (c.type === "video" || c.type === "audio") && !c.hidden && c.asset).sort((a, b) => a.start - b.start);
  for (const c of media) {
    const k = assetKind(c.asset); if (!k || k === "image") continue;
    const mine = slots.filter((s) => s.asset === c.asset);
    let pick: Slot | undefined; let best = Infinity;
    for (const s of mine) { const le = lastEnd.get(s.id) ?? -Infinity; if (le <= c.start + 1e-6 && le < best) { best = le; pick = s; } }
    if ((!pick || best > c.start - LEAD) && mine.length < 3) { pick = { id: `${c.asset}#${mine.length}`, asset: c.asset, kind: k === "audio" ? "audio" : "video" }; slots.push(pick); }
    if (!pick) pick = mine.reduce((a, b) => ((lastEnd.get(a.id) ?? 0) <= (lastEnd.get(b.id) ?? 0) ? a : b)); // 3 overlapping clips of one asset: reuse
    slotOf.set(c.id, pick.id); lastEnd.set(pick.id, c.end);
  }
  return { slots, slotOf };
}

function gradeFilter(g: Clip["color"]): string | undefined {
  const f: string[] = [];
  if (g.exposure) f.push(`brightness(${(1 + g.exposure * 0.5).toFixed(3)})`);
  if (g.contrast) f.push(`contrast(${(1 + g.contrast * 0.6).toFixed(3)})`);
  if (g.saturation) f.push(`saturate(${Math.max(0, 1 + g.saturation).toFixed(3)})`);
  if (g.temperature) f.push(`sepia(${Math.abs(g.temperature * 0.25).toFixed(3)})`);
  return f.length ? f.join(" ") : undefined;
}

// ---- stage (re-renders per frame via usePlayhead; small tree) ----------------
function Stage({ scale, muted, safe }: { scale: number; muted: boolean; safe: boolean }) {
  const s = useVideo();
  const playhead = usePlayhead();
  const { comp, playing, agentId, project } = s;
  const sceneW = comp.scene.width, sceneH = comp.scene.height;
  const fps = comp.scene.fps || 30;
  const assetKind = (id: string) => s.assets.find((a) => a.id === id)?.kind;
  const { slots, slotOf } = useMemo(() => assignSlots(comp, assetKind), [comp.clips, s.assets]);
  const els = useRef(new Map<string, HTMLMediaElement>());
  const prepared = useRef(new Map<string, string>()); // slot → clip id pre-seeked

  // per slot: the active clip, else the next upcoming clip (to pre-seek)
  const assigned = useMemo(() => {
    const m = new Map<string, Clip>();
    const next = new Map<string, Clip>();
    for (const c of comp.clips) {
      const sid = slotOf.get(c.id); if (!sid) continue;
      if (playhead >= c.start && playhead < c.end) m.set(sid, c);
      else if (c.start >= playhead) { const n = next.get(sid); if (!n || c.start < n.start) next.set(sid, c); }
    }
    for (const [sid, c] of next) if (!m.has(sid)) m.set(sid, c);
    return m;
  }, [comp.clips, slotOf, playhead]);

  const active = useMemo(() => comp.clips.filter((c) => !c.hidden && playhead >= c.start && playhead < c.end).sort((a, b) => rank(a.track) - rank(b.track)), [comp.clips, playhead]);

  // imperative sync after every render (runs per playhead tick; cheap)
  const lastSeek = useRef(new Map<string, number>());
  const setRate = (el: HTMLMediaElement, r: number) => { if (Math.abs(el.playbackRate - r) > 0.004) el.playbackRate = r; };
  const seekTo = (slotId: string, el: HTMLMediaElement, t: number) => { lastSeek.current.set(slotId, performance.now()); el.currentTime = Math.max(0, t); };
  useLayoutEffect(() => {
    const now = performance.now();
    for (const slot of slots) {
      const el = els.current.get(slot.id); if (!el) continue;
      const c = assigned.get(slot.id);
      if (!c) { if (!el.paused) el.pause(); continue; }
      const isActive = playhead >= c.start && playhead < c.end;
      const srcT = c.in + (playhead - c.start) * c.speed;   // negative offset before the clip starts
      const wantMuted = muted || c.muted || !isActive;
      if (el.muted !== wantMuted) el.muted = wantMuted;
      const vol = Math.max(0, Math.min(1, Math.pow(10, c.volume / 20)));
      if (Math.abs(el.volume - vol) > 0.005) el.volume = vol;

      if (!playing) {
        // paused / scrubbing: frame-accurate park (active → exact frame, upcoming → its first frame)
        if (!el.paused) el.pause();
        setRate(el, c.speed);
        const park = isActive ? srcT : c.in;
        if (!el.seeking && Math.abs(el.currentTime - park) > 0.5 / fps) seekTo(slot.id, el, park);
        continue;
      }
      const preroll = !isActive && c.start - playhead <= LEAD && srcT >= 0;
      if (!isActive && !preroll) {
        // upcoming but not yet in the pre-roll window: park where the pre-roll will begin
        if (!el.paused) el.pause();
        setRate(el, c.speed);
        if (prepared.current.get(slot.id) !== c.id) { prepared.current.set(slot.id, c.id); const park = Math.max(0, c.in - LEAD * c.speed); if (Math.abs(el.currentTime - park) > 0.02) seekTo(slot.id, el, park); }
        continue;
      }
      // playing (visible) or pre-rolling (hidden): chase the wall clock
      prepared.current.set(slot.id, c.id);
      if (el.paused) void el.play().catch(() => {});
      if (el.seeking) continue;
      const drift = srcT - el.currentTime;                // > 0: element is behind the playhead
      const toCut = c.start - playhead;
      const hard = isActive ? 0.25 : toCut > 0.2 ? 0.06 : 0.25;   // hidden seeks are free, so be strict early in pre-roll
      if (Math.abs(drift) > hard && now - (lastSeek.current.get(slot.id) ?? 0) > 400) { seekTo(slot.id, el, srcT); setRate(el, c.speed); }
      else if (Math.abs(drift) < 0.5 / fps) setRate(el, c.speed);
      else setRate(el, c.speed * (1 + Math.max(-NUDGE_MAX, Math.min(NUDGE_MAX, drift * 0.6))));
    }
  });
  useEffect(() => () => { els.current.forEach((el) => el.pause()); }, []);

  // transport clock — wall clock is the master; media elements chase it (above)
  const total = durOf(comp);
  useEffect(() => {
    if (!playing) return;
    let raf = 0; let last = performance.now();
    const tick = (now: number) => {
      const dt = Math.min(0.1, (now - last) / 1000); last = now;   // clamp: a hidden tab shouldn't leap
      const st = get(); if (!st.playing) return;
      const t = st.playhead + dt;
      if (t >= Math.max(total, 0.01)) { if (st.loop) seek(0); else { set({ playing: false }); seek(total); return; } }
      else tickPlayhead(t);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, total]);

  const images = active.filter((c) => c.type === "image" && TRACK_KIND(c.track) !== "audio");
  const texts = active.filter((c) => c.type === "text");
  const caption = comp.captions.enabled ? s.captionLines.find((l) => playhead >= l.s && playhead < l.e) : undefined;
  const showFrame = s.frame && Math.abs(s.frame.time - playhead) < 0.02;
  const anyVisual = active.some((c) => c.type !== "audio" && TRACK_KIND(c.track) !== "audio");
  // media under the playhead whose file isn't readable right now (drive unplugged / moved)
  const offlineHere = active.map((c) => s.assets.find((a) => a.id === c.asset)).filter((a): a is NonNullable<typeof a> => !!a && a.online === false);

  const layerStyle = (c: Clip, visible: boolean): React.CSSProperties => ({
    width: sceneW * scale, height: sceneH * scale, objectFit: (c.fit === "contain" ? "contain" : c.fit === "none" ? "none" : "cover") as any,
    transform: `translate(-50%,-50%) translate(${c.transform.x * scale}px, ${c.transform.y * scale}px) scale(${c.transform.scale}) rotate(${c.transform.rotation}deg)`,
    opacity: visible ? c.transform.opacity : 0, visibility: visible ? "visible" : "hidden", zIndex: rank(c.track), filter: gradeFilter(c.color),
    outline: visible && s.selection.includes(c.id) ? "1.5px solid var(--accent)" : undefined,
  });

  return (
    <div className="ve-canvas" style={{ width: sceneW * scale, height: sceneH * scale, background: comp.scene.background }} onClick={(e) => e.stopPropagation()}>
      {agentId && project && slots.map((slot) => {
        const c = assigned.get(slot.id);
        const url = mediaUrl(agentId, project, "asset", slot.asset);
        const visible = !!c && c.type === "video" && TRACK_KIND(c.track) !== "audio" && playhead >= c.start && playhead < c.end;
        const setRef = (el: HTMLMediaElement | null) => { if (el) els.current.set(slot.id, el); else els.current.delete(slot.id); };
        if (slot.kind === "audio") return <audio key={slot.id} ref={setRef} src={url} preload="auto" />;
        return <video key={slot.id} ref={setRef} src={url} preload="auto" playsInline disablePictureInPicture style={c ? layerStyle(c, visible) : { visibility: "hidden" }} />;
      })}
      {agentId && project && images.map((c) => <img key={c.asset} className="layer" src={mediaUrl(agentId, project, "asset", c.asset)} style={layerStyle(c, true)} draggable={false} alt="" />)}
      {texts.map((c) => (
        <div key={c.id} className="txt" style={{
          left: `${c.text.x * 100}%`, top: `${c.text.y * 100}%`, maxWidth: c.text.maxWidth > 0 ? `${c.text.maxWidth * 100}%` : undefined, zIndex: rank(c.track),
          fontFamily: `"${c.text.font}", -apple-system, system-ui, sans-serif`, fontSize: c.text.size * scale, fontWeight: c.text.weight, color: c.text.color,
          textAlign: c.text.align, textShadow: c.text.bg ? undefined : c.text.shadow ? `0 ${c.text.size * scale * 0.04}px ${c.text.size * scale * 0.12}px rgba(0,0,0,.6)` : undefined,
          background: c.text.bg ? hexA(c.text.bg, c.text.bgOpacity) : undefined, padding: c.text.bg ? c.text.padding * scale : 0, borderRadius: c.text.bg ? 8 * scale : 0,
          opacity: fadeAlpha(c, playhead), outline: s.selection.includes(c.id) ? "1.5px solid var(--accent)" : undefined,
        }}>{c.text.content}</div>
      ))}
      {caption && (
        <div className="cap" style={{ top: `${comp.captions.y * 100}%`, zIndex: 200, fontFamily: `"${comp.captions.font}", -apple-system, system-ui, sans-serif`, fontSize: comp.captions.size * scale, fontWeight: comp.captions.weight, color: comp.captions.color, textShadow: comp.captions.shadow ? `0 ${3 * scale}px ${16 * scale}px rgba(0,0,0,.65)` : undefined }}>
          {caption.words.map((w, i) => {
            const on = playhead >= w.s - 0.01;
            const key = comp.captions.keyWords.some((k) => k.toLowerCase() === w.w.toLowerCase().replace(/[^\p{L}\p{N}']/gu, ""));
            const pop = comp.captions.preset === "pop";
            return <span key={i} className="w" style={{ opacity: pop ? (on ? 1 : 0) : 1, transform: pop ? (on ? "none" : "translateY(8px) scale(.86)") : undefined, transition: "opacity 120ms, transform 160ms cubic-bezier(.2,1.6,.4,1)", color: key || (comp.captions.preset === "karaoke" && on) ? comp.captions.keyColor : undefined, textShadow: key ? `0 0 ${20 * scale}px ${comp.captions.keyColor}` : undefined }}>{comp.captions.uppercase ? w.w.toUpperCase() : w.w}</span>;
          })}
        </div>
      )}
      {showFrame && s.frame && agentId && project && <img className="layer" src={mediaUrl(agentId, project, "cache", s.frame.path, s.frame.at)} style={{ width: "100%", height: "100%", objectFit: "contain", zIndex: 300 }} alt="" />}
      {showFrame && <div className="frame-badge" style={{ zIndex: 301 }}>FFMPEG FRAME</div>}
      {safe && <div className="safe" style={{ zIndex: 302 }} />}
      {offlineHere.length > 0 && !showFrame && (
        <div className="offline" style={{ zIndex: 250 }}>
          <Unplug size={22} />
          <b>Media offline</b>
          {offlineHere.slice(0, 2).map((a) => <div key={a.id} className="f"><span>{a.name}</span><code title={a.path}>{a.path}</code><em>{a.linked ? "hardlink missing — use Relink in Media" : `plug in “${volumeOf(a.path)}”, or Relink in Media`}</em></div>)}
          {offlineHere.length > 2 && <em>+{offlineHere.length - 2} more in the Media panel</em>}
        </div>
      )}
      {!anyVisual && !showFrame && offlineHere.length === 0 && <div className="empty">{comp.clips.length ? `nothing at ${fmtTime(playhead, fps)}` : "drop footage into the timeline"}</div>}
    </div>
  );
}

function Timecode({ total, fps }: { total: number; fps: number }) {
  const playhead = usePlayhead();
  return <span className="tc ve-mono">{fmtTime(playhead, fps)} <span className="dim">/ {fmtTime(total, fps)}</span></span>;
}

export function Player() {
  const s = useVideo();
  const stageRef = useRef<HTMLDivElement>(null);
  const size = useStageSize(stageRef);
  const [muted, setMuted] = useState(false);
  const [safe, setSafe] = useState(false);
  const [sceneOpen, setSceneOpen] = useState(false);
  const { comp, playing } = s;
  const total = durOf(comp);
  const sceneW = comp.scene.width, sceneH = comp.scene.height;
  const scale = Math.max(0.05, Math.min((size.w - 8) / sceneW, (size.h - 8) / sceneH, 1.6));
  const fps = comp.scene.fps || 30;

  return (
    <div className="ve-player">
      <div className="ve-stage" ref={stageRef} onClick={() => set({ selection: [] })}>
        <Stage scale={scale} muted={muted} safe={safe} />
        {s.toolProgress && <div className="ve-pill" style={{ position: "absolute", top: 20, left: "50%", transform: "translateX(-50%)", background: "var(--surface)" }}><span className="ve-spinner" />{s.toolProgress}</div>}
      </div>
      <div className="ve-transport">
        <Timecode total={total} fps={fps} />
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
