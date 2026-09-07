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
//  · Gapless audio: pre-roll warms every incoming element (exact-arrival play for
//    clips with room, hold-at-head for clips near source 0), and the outgoing
//    element keeps a ≤350 ms tail past each cut until the new owner is confirmed
//    rolling — the tail covers any residual decoder spin-up, so no dropped audio.
//  · The VISIBLE A-roll <video> is the master clock while playing and its
//    playbackRate is never touched (each rate change makes AVPlayer re-sync →
//    stutter). The playhead is derived from its currentTime; the wall clock only
//    fills in when no video is on screen or the master stalls. Followers (audio
//    beds, B-roll) are corrected by a hard seek on real drift (>200 ms), never by
//    rate nudging. Pre-roll elements get ONE alignment seek (hidden seeks are
//    free) so the handoff at the cut is within a frame or two.
//  · The transport tick publishes on the store's narrow playhead channel
//    (tickPlayhead/usePlayhead); only the stage, timecode and playhead lines
//    re-render per frame — the timeline/inspector/dock stay idle.
// Text clips are drawn live as DOM; captions + graphics are Hyperframes overlay
// clips on T1/V3 (transparent video) and play through the normal slot path.
// "Render frame" shows the exact ffmpeg composite (grade + LUT + overlays).
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Play, Pause, SkipBack, SkipForward, ChevronLeft, ChevronRight, Repeat, Camera, Maximize2, Volume2, VolumeX, Proportions, X } from "lucide-react";
import { useVideo, usePlayhead, tickPlayhead, seek, togglePlay, stepFrames, set, get, renderFrame, mutate } from "./store";
import { Num, volumeOf } from "./Panels";
import { Unplug } from "lucide-react";
import { duration as durOf } from "./model";
import { type Asset, type Clip, type Composition, TRACK_KIND, fmtTime, mediaUrl, kfDbAt } from "./model";

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
const TAIL_MS = 350;       // gapless-audio tail: outgoing keeps playing this long past a cut
                         // until the incoming element is confirmed rolling
type Slot = { id: string; asset: string; kind: "video" | "audio" };
// Replacement-audio dubs: when cleanup is enabled, a video clip with audioAsset
// plays picture muted while a pooled audio slot plays the cleaned file in sync
// (same timing model as export). Bypassed/offline → no dub, original sound.
// Video-kind replacements (cleaned .mov proxies: picture stream-copied + cleaned
// audio) play as ONE element via playClips below — no dub, so no dual-element
// drift. Only audio-only replacements (legacy .wavs, audio sources) dub.
function cleanDubs(comp: Composition, assets: Asset[]): Clip[] {
  if (comp.audio.cleanEnabled === false) return [];
  const byId = new Map(assets.map((a) => [a.id, a]));
  const out: Clip[] = [];
  for (const c of comp.clips) {
    if (c.hidden || c.muted || c.type !== "video" || !c.audioAsset) continue;
    const ra = byId.get(c.audioAsset);
    if (!ra || ra.online === false || ra.kind === "video") continue;
    out.push({ ...c, id: `${c.id}#dub`, type: "audio", asset: c.audioAsset, link: "", name: `${c.name} \u00b7 cleaned` });
  }
  return out;
}
// Single-element preview: a video clip whose replacement carries picture plays
// the replacement file directly (same in/out — the proxy is full-length), so
// picture + cleaned sound share one clock through cuts and on full tracks.
function playClips(comp: Composition, assets: Asset[]): Clip[] {
  if (comp.audio.cleanEnabled === false) return comp.clips;
  const byId = new Map(assets.map((a) => [a.id, a]));
  let swapped = false;
  const out = comp.clips.map((c) => {
    if (c.hidden || c.muted || c.type !== "video" || !c.audioAsset) return c;
    const ra = byId.get(c.audioAsset);
    if (!ra || ra.online === false || ra.kind !== "video") return c;
    swapped = true;
    return { ...c, asset: ra.id };
  });
  return swapped ? out : comp.clips;
}
function assignSlots(comp: Composition, dubs: Clip[], assetKind: (id: string) => "video" | "audio" | "image" | undefined): { slots: Slot[]; slotOf: Map<string, string> } {
  const slots: Slot[] = []; const slotOf = new Map<string, string>();
  const lastEnd = new Map<string, number>();
  // Linked V+A pairs share one voice: the video element carries the sound, so
  // the linked audio clip needs no slot of its own (it would double-play).
  // (A clip WITH an active cleaned dub is the exception: its picture plays
  // muted and the dub pseudo below carries the sound.)
  const videoLinks = new Set(comp.clips.filter((c) => c.type === "video" && c.link).map((c) => c.link));
  const media = comp.clips.filter((c) => (c.type === "video" || c.type === "audio") && !c.hidden && c.asset && !(c.type === "audio" && c.link && videoLinks.has(c.link))).concat(dubs).sort((a, b) => a.start - b.start);
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
  const pcomp = useMemo(() => ({ ...comp, clips: playClips(comp, s.assets) }), [comp, s.assets]);
  const dubs = useMemo(() => cleanDubs(pcomp, s.assets), [pcomp, s.assets]);
  const dubbed = useMemo(() => new Set(dubs.map((d) => d.id.replace(/#dub$/, ""))), [dubs]);
  const { slots, slotOf } = useMemo(() => assignSlots(pcomp, dubs, assetKind), [pcomp, s.assets, dubs]);
  const els = useRef(new Map<string, HTMLMediaElement>());
  const prepared = useRef(new Map<string, string>()); // slot → clip id pre-seeked
  const retiring = useRef(new Map<string, number>()); // slot → tail deadline (performance.now ms)
  const headHeld = useRef(new Map<string, string>()); // slot → clip id seek-parked at head

  // per slot: the active clip, else the next upcoming clip (to pre-seek)
  const assigned = useMemo(() => {
    const m = new Map<string, Clip>();
    const next = new Map<string, Clip>();
    for (const c of [...pcomp.clips, ...dubs]) {
      const sid = slotOf.get(c.id); if (!sid) continue;
      if (playhead >= c.start && playhead < c.end) m.set(sid, c);
      else if (c.start >= playhead) { const n = next.get(sid); if (!n || c.start < n.start) next.set(sid, c); }
    }
    for (const [sid, c] of next) if (!m.has(sid)) m.set(sid, c);
    return m;
  }, [pcomp, dubs, slotOf, playhead]);

  const active = useMemo(() => pcomp.clips.filter((c) => !c.hidden && playhead >= c.start && playhead < c.end).sort((a, b) => rank(a.track) - rank(b.track)), [pcomp, playhead]);

  // imperative sync after every render (runs per playhead tick; only writes on change)
  const lastSeek = useRef(new Map<string, number>());
  const masterRef = useRef<string | null>(null);   // slot id driving the clock
  const setRate = (el: HTMLMediaElement, r: number) => { if (Math.abs(el.playbackRate - r) > 0.004) el.playbackRate = r; };
  const seekTo = (slotId: string, el: HTMLMediaElement, t: number) => { lastSeek.current.set(slotId, performance.now()); el.currentTime = Math.max(0, t); };
  // Gapless-audio gate: every audible clip under the playhead must be confirmed
  // rolling (unpaused, buffered, on-position) before a retiring tail may stop.
  // A clip with no slot (offline media) counts as vacuous — tails still end on time.
  const audibleOwnersRolling = () => {
    const owners = [...pcomp.clips, ...dubs].filter((k) => !k.hidden && !k.muted && (k.type === "video" || k.type === "audio") && playhead >= k.start && playhead < k.end);
    if (!owners.length) return true;
    return owners.every((k) => {
      const sid = slotOf.get(k.id); if (!sid) return true;
      const oel = els.current.get(sid); if (!oel) return false;
      if (oel.paused || oel.readyState < 3) return false;
      const st = k.in + (playhead - k.start) * k.speed;
      return Math.abs(oel.currentTime - st) < 0.5;
    });
  };
  const masterClip = active.find((c) => c.type === "video" && TRACK_KIND(c.track) === "video" && slotOf.has(c.id) && s.assets.find((a) => a.id === c.asset)?.online !== false);
  masterRef.current = masterClip ? slotOf.get(masterClip.id)! : null;
  useLayoutEffect(() => {
    const now = performance.now();
    const ownersRolling = audibleOwnersRolling();
    // Retiring tail: keep a just-ended audible element playing (unmuted) until
    // the new owners roll or the cap hits. Shared by unassigned slots and slots
    // already assigned to a future clip — both sound the previous clip.
    const keepTail = (slotId: string, el: HTMLMediaElement) => {
      if (!playing || muted || el.paused || el.muted) return false;
      let until = retiring.current.get(slotId);
      if (until === undefined) {
        until = performance.now() + TAIL_MS;
        if (retiring.current.size > 6) { const k = retiring.current.keys().next().value; if (k) retiring.current.delete(k); }
        retiring.current.set(slotId, until);
      }
      if (performance.now() <= until && !ownersRolling) return true;
      retiring.current.delete(slotId);
      return false;
    };
    for (const slot of slots) {
      const el = els.current.get(slot.id); if (!el) continue;
      const c = assigned.get(slot.id);
      if (!c) {
        if (keepTail(slot.id, el)) continue;
        if (!el.paused) el.pause(); continue;
      }
      const isActive = playhead >= c.start && playhead < c.end;
      const srcT = c.in + (playhead - c.start) * c.speed;   // negative offset before the clip starts
      if (isActive) retiring.current.delete(slot.id);
      // Slot already assigned to a FUTURE clip but still sounding the just-ended
      // one: keep the tail (checked before wantMuted mutes it below).
      if (!isActive && playhead < c.start && keepTail(slot.id, el)) continue;
      const hasDub = c.type === "video" && dubbed.has(c.id);
      const wantMuted = muted || c.muted || !isActive || hasDub;
      if (el.muted !== wantMuted) el.muted = wantMuted;
      // Preview mix mirrors the export graph: clip gain + track trim + the
      // manual keyframe envelope (evaluated at clip-local time, like ffmpeg).
      const tg = comp.audio.trackGain?.[c.track] ?? 0;
      const kdb = c.audio.keyframes.length ? kfDbAt(c.audio.keyframes, playhead) : 0;
      const vol = hasDub && slot.kind === "video" ? 0 : Math.max(0, Math.min(1, Math.pow(10, (c.volume + tg + kdb) / 20)));
      if (Math.abs(el.volume - vol) > 0.005) el.volume = vol;
      setRate(el, c.speed);

      if (!playing) {
        // paused / scrubbing: frame-accurate park (active → exact frame, upcoming → its first frame)
        if (!el.paused) el.pause();
        const park = isActive ? srcT : c.in;
        if (!el.seeking && Math.abs(el.currentTime - park) > 0.5 / fps) seekTo(slot.id, el, park);
        continue;
      }
      const toCut = c.start - playhead;
      // Pre-roll starts LEAD before the cut for every clip. Clips near source 0
      // (srcT < 0) warm below via hold-at-head instead of early play (below).
      const preroll = !isActive && toCut <= LEAD;
      if (!isActive && !preroll) {
        // upcoming, not yet in the pre-roll window: park where the pre-roll will begin
        if (!el.paused) el.pause();
        if (prepared.current.get(slot.id) !== c.id) { prepared.current.set(slot.id, c.id); lastSeek.current.delete(slot.id); const park = Math.max(0, c.in - LEAD * c.speed); if (Math.abs(el.currentTime - park) > 0.02) seekTo(slot.id, el, park); }
        continue;
      }
      prepared.current.set(slot.id, c.id);
      if (isActive) headHeld.current.delete(slot.id);
      if (!isActive && srcT < 0) {
        // Hold-at-head warm-up: this clip starts near source 0, so there is no
        // earlier media to pre-play. Seek straight to c.in (once), then keep the
        // decoder warm and hold paused exactly at head — the cut starts
        // instantly, no cold-start gap.
        if (headHeld.current.get(slot.id) !== c.id) {
          if (Math.abs(el.currentTime - c.in) > 0.05 && !el.seeking) seekTo(slot.id, el, c.in);
          else headHeld.current.set(slot.id, c.id);
        }
        if (headHeld.current.get(slot.id) === c.id) {
          if (el.paused && el.readyState >= 2) void el.play().catch(() => {});
          else if (!el.paused && Math.abs(el.currentTime - c.in) <= 0.06) el.pause();
        }
        continue;
      }
      if (el.paused) void el.play().catch(() => {});
      if (el.seeking) continue;
      if (slot.id === masterRef.current) continue;              // master drives the clock; never correct it
      const drift = srcT - el.currentTime;                     // > 0: element is behind the playhead
      const since = now - (lastSeek.current.get(slot.id) ?? 0);
      if (preroll) {
        // one alignment seek early in the pre-roll (hidden → free), then hands off as-is
        if (toCut > 0.35 && Math.abs(drift) > 0.04 && since > 600) seekTo(slot.id, el, srcT + 0.02);
      } else if (Math.abs(drift) > 0.2 && since > 1000) {
        seekTo(slot.id, el, srcT);                              // follower (audio bed / B-roll): rare hard correction only
      }
    }
  });
  useEffect(() => () => { els.current.forEach((el) => el.pause()); }, []);

  // transport clock — master <video> clock when one is on screen, else wall clock
  const total = durOf(comp);
  useEffect(() => {
    if (!playing) return;
    let raf = 0; let last = performance.now();
    const tick = (now: number) => {
      const dt = Math.min(0.1, (now - last) / 1000); last = now;
      const st = get(); if (!st.playing) return;
      const wall = st.playhead + dt;
      let t = wall;
      const mid = masterRef.current;
      if (mid) {
        const el = els.current.get(mid);
        const mc = pcomp.clips.find((c) => slotOf.get(c.id) === mid && st.playhead >= c.start && st.playhead < c.end);
        if (el && mc && !el.paused && !el.seeking && el.readyState >= 3) {
          const mt = mc.start + (el.currentTime - mc.in) / mc.speed;
          // trust the picture unless it has clearly stalled (then coast on the wall clock)
          if (mt > st.playhead - 0.15 && mt < wall + 0.25) t = Math.max(mt, st.playhead); // monotonic: never step the timeline backwards
        }
      }
      if (t >= Math.max(total, 0.01)) { if (st.loop) seek(0); else { set({ playing: false }); seek(total); return; } }
      else tickPlayhead(t);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, total, slotOf, pcomp]);

  const images = active.filter((c) => c.type === "image" && TRACK_KIND(c.track) !== "audio");
  const texts = active.filter((c) => c.type === "text");
  const showFrame = s.frame && Math.abs(s.frame.time - playhead) < 0.02;
  const anyVisual = active.some((c) => c.type !== "audio" && c.type !== "review" && TRACK_KIND(c.track) !== "audio");
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
        <button className="ve-btn sm" title="Render this frame with the real ffmpeg pipeline (grade, LUT, overlays)" onClick={() => void renderFrame()}><Camera size={13} /> Render frame</button>
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
