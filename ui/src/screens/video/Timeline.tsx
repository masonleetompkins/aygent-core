// AYGENT — VIDEO v0.3 timeline. Multi-track, zoomable, snapping. Clip bars are
// flat accent blocks (audio shows its cached waveform); drag body = move (with
// track change), drag edges = trim (source in/out follow, rate-aware), razor tool
// or K splits, marquee-select on empty lane, drag an asset from Media to place.
import { useEffect, useMemo, useRef, useState } from "react";
import { Scissors, MousePointer2, Magnet, ZoomIn, ZoomOut, Trash2, Copy, AlignHorizontalSpaceAround, Eye, EyeOff, Volume2, VolumeX, Lock, Unlock, Undo2, Redo2 } from "lucide-react";
import { useVideo, useVideoSel, usePlayhead, set, get, seek, patchClips, beginGesture, splitAt, deleteSelected, duplicateSelected, closeGaps, addAssetToTimeline, undo, redo, canUndo, canRedo, mutate } from "./store";
import { type Clip, TRACK_ORDER, TRACK_KIND, TRACK_LABEL, duration as durOf, fmtTime, mediaUrl, clipDur } from "./model";

const LANE_H: Record<string, number> = { video: 56, text: 34, audio: 44 };
const MIN_ZOOM = 6, MAX_ZOOM = 600;

export function Timeline() {
  const s = useVideo();
  const { comp, zoom, playhead, selection } = s;
  const lanesRef = useRef<HTMLDivElement>(null);
  const rulerRef = useRef<HTMLDivElement>(null);
  const hdrsRef = useRef<HTMLDivElement>(null);
  const [snapLine, setSnapLine] = useState<number | null>(null);
  const [marquee, setMarquee] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  const [overLane, setOverLane] = useState<string | null>(null);
  const [locked, setLocked] = useState<Record<string, boolean>>({});
  const [height, setHeight] = useState(300);
  const [laneWidth, setLaneWidth] = useState(800);
  const total = durOf(comp);
  const fps = comp.scene.fps || 30;
  const tracks = useMemo(() => {
    const used = new Set(comp.clips.map((c) => c.track));
    const extra = [...used].filter((t) => !TRACK_ORDER.includes(t)).sort();
    // extra V tracks (V4+) slot in just under T1 so text always stays on top
    const extraV = extra.filter((t) => t.startsWith("V")).reverse();
    return ["T1", ...extraV, ...TRACK_ORDER.filter((t) => t !== "T1"), ...extra.filter((t) => !t.startsWith("V"))];
  }, [comp.clips]);
  const contentW = Math.max(laneWidth, (total + 10) * zoom + 200);

  useEffect(() => {
    const el = lanesRef.current; if (!el) return;
    const ro = new ResizeObserver(() => setLaneWidth(el.clientWidth));
    ro.observe(el); setLaneWidth(el.clientWidth);
    const onScroll = () => { set({ scrollX: el.scrollLeft }); if (rulerRef.current) rulerRef.current.scrollLeft = el.scrollLeft; if (hdrsRef.current) hdrsRef.current.scrollTop = el.scrollTop; };
    el.addEventListener("scroll", onScroll);
    return () => { ro.disconnect(); el.removeEventListener("scroll", onScroll); };
  }, []);

  const snapPoints = useMemo(() => {
    const pts = new Set<number>([0, playhead]);
    for (const c of comp.clips) { pts.add(c.start); pts.add(c.end); }
    return [...pts];
  }, [comp.clips, playhead]);
  function snap(t: number, exclude: Set<number> = new Set()): { t: number; hit: number | null } {
    if (!s.snap) return { t, hit: null };
    const tol = 8 / zoom;
    let best: number | null = null, bd = tol;
    for (const p of snapPoints) { if (exclude.has(p)) continue; const d = Math.abs(p - t); if (d < bd) { bd = d; best = p; } }
    return best === null ? { t, hit: null } : { t: best, hit: best };
  }
  const xToT = (clientX: number) => { const el = lanesRef.current!; const r = el.getBoundingClientRect(); return Math.max(0, (clientX - r.left + el.scrollLeft) / zoom); };
  const frameQ = (t: number) => Math.round(t * fps) / fps;

  // ---- ruler scrub ----
  function onRulerDown(e: React.MouseEvent) {
    e.preventDefault();
    const el = rulerRef.current!;
    const r = el.getBoundingClientRect();
    const to = (cx: number) => Math.max(0, (cx - r.left + (lanesRef.current?.scrollLeft ?? 0)) / zoom);
    set({ playing: false }); seek(frameQ(to(e.clientX)));
    const mv = (ev: MouseEvent) => seek(frameQ(to(ev.clientX)));
    const up = () => { window.removeEventListener("mousemove", mv); window.removeEventListener("mouseup", up); };
    window.addEventListener("mousemove", mv); window.addEventListener("mouseup", up);
  }

  // ---- clip drag: move / trim ----
  function onClipDown(e: React.MouseEvent, clip: Clip, mode: "move" | "l" | "r") {
    e.stopPropagation(); e.preventDefault();
    if (locked[clip.track]) return;
    if (s.tool === "razor") { splitAt(frameQ(xToT(e.clientX)), [clip.id]); return; }
    const multi = e.shiftKey || e.metaKey;
    let sel = selection.includes(clip.id) ? selection : multi ? [...selection, clip.id] : [clip.id];
    if (multi && selection.includes(clip.id) && mode === "move") { sel = selection.filter((i) => i !== clip.id); set({ selection: sel }); return; }
    set({ selection: sel, dockTab: s.dockTab === "inspector" || sel.length ? s.dockTab : "inspector" });
    const startX = e.clientX, startY = e.clientY;
    const orig = new Map(get().comp.clips.filter((c) => sel.includes(c.id)).map((c) => [c.id, { ...c }]));
    const anchor = orig.get(clip.id)!;
    const laneEls = Array.from(lanesRef.current!.querySelectorAll<HTMLElement>("[data-track]"));
    let began = false; let moved = false;
    const mv = (ev: MouseEvent) => {
      const dx = ev.clientX - startX;
      if (!moved && Math.abs(dx) < 3 && Math.abs(ev.clientY - startY) < 3) return;
      moved = true;
      if (!began) { beginGesture(); began = true; }
      let dt = dx / zoom;
      const excl = new Set<number>(); for (const o of orig.values()) { excl.add(o.start); excl.add(o.end); }
      if (mode === "move") {
        const a = snap(anchor.start + dt, excl); const b = snap(anchor.end + dt, excl);
        if (a.hit !== null) dt = a.t - anchor.start; else if (b.hit !== null) dt = b.t - anchor.end;
        setSnapLine(a.hit ?? b.hit);
        dt = Math.max(dt, -Math.min(...[...orig.values()].map((o) => o.start)));
        dt = frameQ(dt);
        // track change (single clip or same-kind group)
        let newTrack: string | null = null;
        const lane = laneEls.find((l) => { const r = l.getBoundingClientRect(); return ev.clientY >= r.top && ev.clientY < r.bottom; });
        if (lane) { const t = lane.dataset.track!; if (TRACK_KIND(t) === TRACK_KIND(anchor.track) && !locked[t]) newTrack = t; }
        patchClips(sel, (k) => { const o = orig.get(k.id)!; const d = clipDur(o); const ns = Math.max(0, o.start + dt); return { start: ns, end: ns + d, track: newTrack && sel.length === 1 ? newTrack : k.track }; }, { undo: false });
      } else if (mode === "l") {
        const o = anchor; const minStart = o.start - o.in / o.speed; // can't reveal before source 0
        let ns = Math.max(minStart, Math.min(o.end - 1 / fps, o.start + dt));
        const sn = snap(ns, excl); if (sn.hit !== null) ns = Math.max(minStart, Math.min(o.end - 1 / fps, sn.t)); setSnapLine(sn.hit);
        ns = frameQ(ns);
        const delta = ns - o.start;
        patchClips([clip.id], () => ({ start: ns, in: o.type === "text" ? 0 : o.in + delta * o.speed }), { undo: false });
      } else {
        const o = anchor;
        const asset = get().assets.find((a) => a.id === o.asset);
        const maxEnd = asset && o.type === "video" ? o.start + (asset.duration - o.in) / o.speed : Infinity;
        let ne = Math.min(maxEnd, Math.max(o.start + 1 / fps, o.end + dt));
        const sn = snap(ne, excl); if (sn.hit !== null) ne = Math.min(maxEnd, Math.max(o.start + 1 / fps, sn.t)); setSnapLine(sn.hit);
        ne = frameQ(ne);
        const delta = ne - o.end;
        patchClips([clip.id], () => ({ end: ne, out: o.type === "text" ? ne - o.start : o.out + delta * o.speed }), { undo: false });
      }
    };
    const up = () => { window.removeEventListener("mousemove", mv); window.removeEventListener("mouseup", up); setSnapLine(null); };
    window.addEventListener("mousemove", mv); window.addEventListener("mouseup", up);
  }

  // ---- empty lane: marquee select or razor/seek ----
  function onLaneDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    const el = lanesRef.current!; const r = el.getBoundingClientRect();
    const t0 = xToT(e.clientX);
    if (s.tool === "razor") { splitAt(frameQ(t0), comp.clips.filter((c) => t0 > c.start && t0 < c.end).map((c) => c.id)); return; }
    set({ selection: e.shiftKey ? selection : [] , playing: false });
    seek(frameQ(t0));
    const x0 = e.clientX - r.left + el.scrollLeft, y0 = e.clientY - r.top + el.scrollTop;
    let dragging = false;
    const mv = (ev: MouseEvent) => {
      const x1 = ev.clientX - r.left + el.scrollLeft, y1 = ev.clientY - r.top + el.scrollTop;
      if (!dragging && Math.hypot(x1 - x0, y1 - y0) < 5) return;
      dragging = true;
      setMarquee({ x0, y0, x1, y1 });
      // hit test
      const [mx0, mx1] = [Math.min(x0, x1), Math.max(x0, x1)]; const [my0, my1] = [Math.min(y0, y1), Math.max(y0, y1)];
      const hits: string[] = [];
      let y = 0;
      for (const t of tracks) {
        const h = LANE_H[TRACK_KIND(t)];
        if (y < my1 && y + h > my0) for (const c of comp.clips) if (c.track === t && c.start * zoom < mx1 && c.end * zoom > mx0) hits.push(c.id);
        y += h;
      }
      set({ selection: hits });
    };
    const up = () => { window.removeEventListener("mousemove", mv); window.removeEventListener("mouseup", up); setMarquee(null); if (!dragging) { /* click = seek only */ } };
    window.addEventListener("mousemove", mv); window.addEventListener("mouseup", up);
  }

  // ---- wheel: zoom with ⌘/ctrl, else scroll ----
  function onWheel(e: React.WheelEvent) {
    if (!(e.metaKey || e.ctrlKey)) return;
    e.preventDefault();
    const el = lanesRef.current!; const r = el.getBoundingClientRect();
    const tAtMouse = (e.clientX - r.left + el.scrollLeft) / zoom;
    const nz = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom * (e.deltaY < 0 ? 1.15 : 0.87)));
    set({ zoom: nz });
    requestAnimationFrame(() => { el.scrollLeft = Math.max(0, tAtMouse * nz - (e.clientX - r.left)); });
  }
  const zoomTo = (nz: number) => set({ zoom: Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, nz)) });
  const fit = () => { const el = lanesRef.current; if (!el) return; zoomTo((el.clientWidth - 40) / Math.max(1, total)); el.scrollLeft = 0; };

  // ---- drop from media panel / Finder ----
  function laneAt(clientY: number): string | null {
    const laneEls = Array.from(lanesRef.current?.querySelectorAll<HTMLElement>("[data-track]") ?? []);
    const l = laneEls.find((x) => { const r = x.getBoundingClientRect(); return clientY >= r.top && clientY < r.bottom; });
    return l?.dataset.track ?? null;
  }
  function onDragOver(e: React.DragEvent) {
    if (!e.dataTransfer.types.includes("application/aygent-asset")) return;
    e.preventDefault(); e.dataTransfer.dropEffect = "copy";
    setOverLane(laneAt(e.clientY));
  }
  function onDrop(e: React.DragEvent) {
    const id = e.dataTransfer.getData("application/aygent-asset"); setOverLane(null);
    if (!id) return;
    e.preventDefault();
    const a = get().assets.find((x) => x.id === id); if (!a) return;
    const lane = laneAt(e.clientY);
    const kind = a.kind === "audio" ? "audio" : "video";
    const track = lane && TRACK_KIND(lane) === kind ? lane : undefined;
    const t = snap(frameQ(xToT(e.clientX))).t;
    addAssetToTimeline(a, Math.max(0, t), track);
  }

  // ---- height resize ----
  function onResizeDown(e: React.MouseEvent) {
    e.preventDefault(); const y0 = e.clientY, h0 = height;
    const mv = (ev: MouseEvent) => setHeight(Math.max(160, Math.min(window.innerHeight * 0.7, h0 - (ev.clientY - y0))));
    const up = () => { window.removeEventListener("mousemove", mv); window.removeEventListener("mouseup", up); };
    window.addEventListener("mousemove", mv); window.addEventListener("mouseup", up);
  }

  // ruler ticks
  const ticks = useMemo(() => {
    const steps = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300];
    const major = steps.find((st) => st * zoom >= 90) ?? 300;
    const minor = major / (major >= 60 ? 6 : major >= 5 ? 5 : 4);
    const out: { t: number; major: boolean }[] = [];
    const end = contentW / zoom;
    for (let t = 0; t <= end; t += minor) { const isMajor = Math.abs(t / major - Math.round(t / major)) < 1e-6; out.push({ t, major: isMajor }); }
    return out;
  }, [zoom, contentW]);

  const toggleTrack = (t: string, key: "hidden" | "muted") => mutate((c) => { const ks = c.clips.filter((k) => k.track === t); const all = ks.length > 0 && ks.every((k) => k[key]); for (const k of ks) k[key] = !all; });
  const trackState = (t: string, key: "hidden" | "muted") => { const ks = comp.clips.filter((k) => k.track === t); return ks.length > 0 && ks.every((k) => k[key]); };

  return (
    <div className={`ve-tl ${s.tool === "razor" ? "ve-razor-cursor" : ""}`} style={{ height }}>
      <div className="resize" onMouseDown={onResizeDown} />
      <div className="ve-tl-bar">
        <div className="ve-seg">
          <button className={s.tool === "select" ? "on" : ""} title="Select (V)" onClick={() => set({ tool: "select" })}><MousePointer2 size={13} /></button>
          <button className={s.tool === "razor" ? "on" : ""} title="Razor (C)" onClick={() => set({ tool: "razor" })}><Scissors size={13} /></button>
        </div>
        <button className={`ve-icon-btn ${s.snap ? "on" : ""}`} title="Snap (S)" onClick={() => set({ snap: !s.snap })}><Magnet size={14} /></button>
        <span style={{ width: 6 }} />
        <button className="ve-icon-btn" title="Undo (⌘Z)" disabled={!canUndo()} onClick={undo}><Undo2 size={14} /></button>
        <button className="ve-icon-btn" title="Redo (⇧⌘Z)" disabled={!canRedo()} onClick={redo}><Redo2 size={14} /></button>
        <span style={{ width: 6 }} />
        <button className="ve-btn sm" title="Split at playhead (K)" disabled={!comp.clips.some((c) => playhead > c.start && playhead < c.end)} onClick={() => splitAt(playhead)}><Scissors size={12} /> Split</button>
        <button className="ve-icon-btn" title="Delete (⌫) · ripple (⇧⌫)" disabled={!selection.length} onClick={() => deleteSelected(false)}><Trash2 size={14} /></button>
        <button className="ve-icon-btn" title="Duplicate (⌘D)" disabled={!selection.length} onClick={duplicateSelected}><Copy size={14} /></button>
        <button className="ve-icon-btn" title="Close gaps on all tracks" disabled={!comp.clips.length} onClick={() => closeGaps()}><AlignHorizontalSpaceAround size={14} /></button>
        <span className="spacer" />
        <span className="ve-faint" style={{ fontSize: 11 }}>{selection.length ? `${selection.length} selected` : `${comp.clips.length} clips · ${fmtTime(total, fps)}`}</span>
        <span style={{ width: 8 }} />
        <button className="ve-icon-btn" title="Zoom out (−)" onClick={() => zoomTo(zoom / 1.3)}><ZoomOut size={14} /></button>
        <input type="range" min={Math.log(MIN_ZOOM)} max={Math.log(MAX_ZOOM)} step={0.01} value={Math.log(zoom)} onChange={(e) => zoomTo(Math.exp(Number(e.target.value)))} style={{ width: 110 }} />
        <button className="ve-icon-btn" title="Zoom in (+)" onClick={() => zoomTo(zoom * 1.3)}><ZoomIn size={14} /></button>
        <button className="ve-btn sm" onClick={fit} title="Fit timeline (⇧Z)">Fit</button>
      </div>

      <div className="ve-tl-hdr" />
      <div className="ve-ruler" ref={rulerRef} onMouseDown={onRulerDown}>
        <div style={{ position: "relative", width: contentW, height: "100%" }}>
          {ticks.map((k) => <div key={k.t} className={`tick ${k.major ? "" : "minor"}`} style={{ left: k.t * zoom }}>{k.major && <span>{fmtTime(k.t, fps).replace(/^00:/, "")}</span>}</div>)}
          <RulerPlayhead zoom={zoom} />
        </div>
      </div>

      <div className="ve-tracks">
        <div className="ve-track-hdrs" ref={hdrsRef}>
          {tracks.map((t) => {
            const kind = TRACK_KIND(t);
            return (
              <div key={t} className="ve-track-hdr" style={{ height: LANE_H[kind] }}>
                <span className="id">{t}</span>
                <span className="nm">{TRACK_LABEL[t] ?? (kind === "video" ? "Video" : kind === "text" ? "Text" : "Audio")}</span>
                {kind !== "audio" && <button className={`t ${trackState(t, "hidden") ? "off" : ""}`} title="Toggle visibility" onClick={() => toggleTrack(t, "hidden")}>{trackState(t, "hidden") ? <EyeOff size={13} /> : <Eye size={13} />}</button>}
                {kind !== "text" && <button className={`t ${trackState(t, "muted") ? "off" : ""}`} title="Toggle mute" onClick={() => toggleTrack(t, "muted")}>{trackState(t, "muted") ? <VolumeX size={13} /> : <Volume2 size={13} />}</button>}
                <button className={`t ${locked[t] ? "off" : ""}`} title="Lock track" onClick={() => setLocked({ ...locked, [t]: !locked[t] })}>{locked[t] ? <Lock size={12} /> : <Unlock size={12} />}</button>
              </div>
            );
          })}
          <div className="ve-track-hdr" style={{ height: 30 }}><span className="ve-faint" style={{ fontSize: 10.5 }}>inspector → track: V3, V4… adds lanes</span></div>
        </div>
        <div className="ve-lanes aygent-scroll" ref={lanesRef} onWheel={onWheel} onDragOver={onDragOver} onDragLeave={() => setOverLane(null)} onDrop={onDrop}>
          <div style={{ position: "relative", width: contentW, minHeight: "100%" }}>
            {tracks.map((t) => {
              const kind = TRACK_KIND(t);
              return (
                <div key={t} data-track={t} className={`ve-lane ${kind} ${overLane === t ? "over" : ""}`} style={{ height: LANE_H[kind], opacity: locked[t] ? 0.6 : 1 }} onMouseDown={onLaneDown}>
                  {comp.clips.filter((c) => c.track === t).map((c) => <ClipView key={c.id} clip={c} zoom={zoom} sel={selection.includes(c.id)} agentId={s.agentId} project={s.project} bust={s.cacheBust} onDown={onClipDown} />)}
                </div>
              );
            })}
            <div style={{ height: 30 }} />
            <LanePlayhead zoom={zoom} lanesRef={lanesRef} />
            {snapLine !== null && <div className="ve-snapline" style={{ left: snapLine * zoom }} />}
            {marquee && <div className="ve-marquee" style={{ left: Math.min(marquee.x0, marquee.x1), top: Math.min(marquee.y0, marquee.y1), width: Math.abs(marquee.x1 - marquee.x0), height: Math.abs(marquee.y1 - marquee.y0) }} />}
          </div>
        </div>
      </div>
    </div>
  );
}

// Playhead lines subscribe to the narrow playhead channel so a 60 Hz transport
// tick moves only these two nodes (+ auto-scroll), not the whole timeline.
function RulerPlayhead({ zoom }: { zoom: number }) {
  const playhead = usePlayhead();
  return <div className="ph" style={{ left: playhead * zoom }} />;
}
function LanePlayhead({ zoom, lanesRef }: { zoom: number; lanesRef: React.RefObject<HTMLDivElement> }) {
  const playhead = usePlayhead();
  const playing = useVideoSel((st) => st.playing);
  useEffect(() => {
    const el = lanesRef.current; if (!el || !playing) return;
    const x = playhead * zoom;
    if (x < el.scrollLeft || x > el.scrollLeft + el.clientWidth - 40) el.scrollLeft = Math.max(0, x - 80);
  }, [playhead, playing, zoom, lanesRef]);
  return <div className="ve-playhead" style={{ left: playhead * zoom }} />;
}

function ClipView({ clip: c, zoom, sel, agentId, project, bust, onDown }: { clip: Clip; zoom: number; sel: boolean; agentId: string | null; project: string | null; bust: number; onDown: (e: React.MouseEvent, c: Clip, m: "move" | "l" | "r") => void }) {
  const asset = useVideoSel((st) => st.assets.find((a) => a.id === c.asset));
  const w = Math.max(4, clipDur(c) * zoom);
  const th = asset?.thumbs;
  let wave: React.CSSProperties | undefined;
  if (th && th.wave && agentId && project && asset && c.type === "audio") {
    const totalW = (asset.duration / c.speed) * zoom;
    wave = { backgroundImage: `url(${mediaUrl(agentId, project, "cache", th.wave, bust)})`, backgroundSize: `${totalW}px 100%`, backgroundPosition: `${-(c.in / c.speed) * zoom}px 0` };
  }
  return (
    <div className={`ve-clip ${c.type} ${sel ? "sel" : ""} ${c.hidden ? "hidden" : ""}`} style={{ left: c.start * zoom, width: w }} onMouseDown={(e) => onDown(e, c, "move")} title={`${c.name || c.text?.content || c.asset}
${fmtTime(c.start)} → ${fmtTime(c.end)}  ·  src ${c.in.toFixed(2)}–${c.out.toFixed(2)}${c.speed !== 1 ? ` · ${c.speed}×` : ""}`}>
      {wave && <div className="wave" style={wave} />}
      <div className="lbl">{c.muted ? "🔇 " : ""}{c.type === "text" ? c.text.content || "Title" : c.name || asset?.name || "clip"}{c.behindSubject ? " · behind" : ""}</div>
      {c.transitionIn.duration > 0 && <div className="fade" style={{ left: 0, borderRight: `${c.transitionIn.duration * zoom}px solid transparent` }} />}
      {c.transitionOut.duration > 0 && <div className="fade" style={{ right: 0, borderLeft: `${c.transitionOut.duration * zoom}px solid transparent` }} />}
      <div className="h l" onMouseDown={(e) => onDown(e, c, "l")} />
      <div className="h r" onMouseDown={(e) => onDown(e, c, "r")} />
    </div>
  );
}
