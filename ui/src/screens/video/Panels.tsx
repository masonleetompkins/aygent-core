// AYGENT — VIDEO v0.3 side panels: Media · Text · Captions · Color · Audio · Export.
// Each panel edits the composition through the store (autosaved) or triggers a
// video_* tool — the SAME core the agent uses, so button == prompt.
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Upload, Plus, X, Type, Wand2, FolderOpen, Download, Square, Sparkles, Music, Film, Image as ImageIcon, Mic2, RefreshCw, Link2, Layers, Unplug } from "lucide-react";
import { useVideo, mutate, importPick, removeAsset, relinkAsset, reload, addAssetToTimeline, addTitle, runTool, pickLut, startRender, cancelRender, reveal, toast, set, refreshRenders, refreshThumbs } from "./store";
import { type Asset, type Composition, fmtDur, fmtBytes, mediaUrl } from "./model";

// ---- small field kit ------------------------------------------------------
export function Field({ label, val, children }: { label: string; val?: string | number; children: React.ReactNode }) {
  return <div className="ve-field"><label>{label}{val !== undefined && <span className="val">{val}</span>}</label>{children}</div>;
}
export function Slider({ label, value, min, max, step = 0.01, onChange, fmt }: { label: string; value: number; min: number; max: number; step?: number; onChange: (v: number) => void; fmt?: (v: number) => string }) {
  return <Field label={label} val={fmt ? fmt(value) : value.toFixed(2)}><input type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} onDoubleClick={() => onChange(0)} /></Field>;
}
export function Num({ label, value, step = 1, min, max, onChange, suffix }: { label: string; value: number; step?: number; min?: number; max?: number; onChange: (v: number) => void; suffix?: string }) {
  const [txt, setTxt] = useState(String(round(value)));
  useEffect(() => setTxt(String(round(value))), [value]);
  const commit = () => { const n = Number(txt); if (isFinite(n)) onChange(Math.max(min ?? -Infinity, Math.min(max ?? Infinity, n))); else setTxt(String(round(value))); };
  return <Field label={label + (suffix ? ` (${suffix})` : "")}><input type="number" step={step} min={min} max={max} value={txt} onChange={(e) => setTxt(e.target.value)} onBlur={commit} onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} /></Field>;
}
const round = (v: number) => Math.round(v * 1000) / 1000;
export function Check({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return <label className="ve-check"><input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />{label}</label>;
}
export function Seg<T extends string>({ value, options, onChange }: { value: T; options: { v: T; l: string }[]; onChange: (v: T) => void }) {
  return <div className="ve-seg">{options.map((o) => <button key={o.v} className={o.v === value ? "on" : ""} onClick={() => onChange(o.v)}>{o.l}</button>)}</div>;
}
export function Section({ title, right, children }: { title: string; right?: React.ReactNode; children: React.ReactNode }) {
  return <div className="ve-section"><h4>{title}{right}</h4>{children}</div>;
}
/** "/Volumes/Mason_2022/…" → "Mason_2022"; internal disk → "Macintosh HD". */
export const volumeOf = (p: string) => { const m = /^\/Volumes\/([^/]+)/.exec(p); return m ? m[1] : "Macintosh HD"; };
const setPath = (path: string, value: unknown) => mutate((c) => { const parts = path.split("."); let cur: any = c; for (let i = 0; i < parts.length - 1; i++) cur = cur[parts[i]]; cur[parts[parts.length - 1]] = value; });

// ---- MEDIA ----------------------------------------------------------------
export function MediaPanel() {
  const s = useVideo();
  const [sel, setSel] = useState<string | null>(null);
  const files = s.assets.filter((a) => !a.name.includes("(alpha)") && !a.name.includes("(enhanced)"));
  const generated = s.assets.filter((a) => a.name.includes("(alpha)") || a.name.includes("(enhanced)"));
  const offline = s.assets.filter((a) => a.online === false);
  return (
    <>
      <div className="ve-panel-head">Media<span className="spacer" /><button className="ve-icon-btn" title="Rebuild thumbnails" onClick={() => void refreshThumbs()}><RefreshCw size={14} /></button><button className="ve-btn sm primary" onClick={() => void importPick()}><Upload size={13} /> Import</button></div>
      <div className="ve-panel-body">
        <div className="ve-drop" onClick={() => void importPick()}>
          <Link2 size={18} />
          <div><b>Drop footage here</b> or click to pick</div>
          <div className="ve-faint" style={{ fontSize: 11 }}>Files are <b>hardlinked</b>, never copied. Same-volume only; others link by reference.</div>
        </div>
        {offline.length > 0 && (
          <div className="ve-offline">
            <div className="hd"><Unplug size={14} /> {offline.length === 1 ? "1 file is offline" : `${offline.length} files are offline`}</div>
            {offline.map((a) => (
              <div key={a.id} className="row">
                <div className="nm">{a.name}</div>
                <div className="pth" title={a.path}>{a.path}</div>
                <div className="acts">
                  <span className="ve-faint">{a.linked ? "hardlink missing" : `plug in ${volumeOf(a.path)}`}</span>
                  <button className="ve-btn sm" onClick={() => void relinkAsset(a.id)}><Link2 size={12} /> Relink…</button>
                </div>
              </div>
            ))}
            <button className="ve-btn sm" style={{ alignSelf: "flex-start" }} onClick={() => void reload()}><RefreshCw size={12} /> Check again</button>
          </div>
        )}
        {files.length > 0 && <AssetGrid assets={files} sel={sel} setSel={setSel} />}
        {generated.length > 0 && <Section title="Generated"><AssetGrid assets={generated} sel={sel} setSel={setSel} /></Section>}
        <p className="ve-hint">Drag a clip onto a lane, or double-click to append at the end. <kbd className="ve-kbd">⌫</kbd> on a card unlinks it.</p>
      </div>
    </>
  );
}
function AssetGrid({ assets, sel, setSel }: { assets: Asset[]; sel: string | null; setSel: (v: string | null) => void }) {
  const s = useVideo();
  return (
    <div className="ve-media">
      {assets.map((a) => {
        const thumb = a.thumbs?.strip && s.agentId && s.project ? mediaUrl(s.agentId, s.project, "cache", a.thumbs.strip, s.cacheBust) : null;
        const fw = a.thumbs?.frameW ?? 114, fh = a.thumbs?.frameH ?? 64;
        return (
          <div key={a.id} className={`item ${sel === a.id ? "sel" : ""} ${a.online === false ? "offline" : ""}`} draggable onDragStart={(e) => { e.dataTransfer.setData("application/aygent-asset", a.id); e.dataTransfer.effectAllowed = "copy"; }}
            onClick={() => setSel(a.id)} onDoubleClick={() => addAssetToTimeline(a)} tabIndex={0}
            onKeyDown={(e) => { if ((e.key === "Backspace" || e.key === "Delete") && sel === a.id) { e.preventDefault(); if (confirm(`Unlink ${a.name}? Clips using it are removed.`)) void removeAsset(a.id); } }}>
            <div className={`thumb ${a.kind}`} style={thumb && a.kind !== "audio" ? { backgroundImage: `url(${thumb})`, backgroundSize: `${(fw / fh) * 100 * (a.thumbs?.stripFrames ?? 1)}% 100%`, backgroundPosition: "left center" } : undefined}>
              {a.kind === "audio" ? <Music size={22} /> : !thumb ? (a.kind === "image" ? <ImageIcon size={22} /> : <Film size={22} />) : null}
            </div>
            <span className="badge">{a.kind === "video" ? `${a.height}p` : a.kind.toUpperCase()}{!a.linked ? " · REF" : ""}</span>
              {a.online === false && <span className="badge off" title={a.path}>OFFLINE</span>}
            <button className="x" title="Unlink" onClick={(e) => { e.stopPropagation(); if (confirm(`Unlink ${a.name}? Clips using it are removed.`)) void removeAsset(a.id); }}><X size={12} /></button>
            <div className="meta"><div className="n" title={a.path}>{a.name}</div><div className="d"><span>{a.kind === "image" ? `${a.width}×${a.height}` : fmtDur(a.duration)}</span><span>{a.fps ? `${Math.round(a.fps)}fps` : fmtBytes(a.size)}</span></div></div>
          </div>
        );
      })}
    </div>
  );
}

// ---- TEXT -----------------------------------------------------------------
const TITLE_PRESETS = [
  { l: "Big centered", t: { size: 120, weight: 800, y: 0.5, align: "center", bg: "", shadow: true } },
  { l: "Lower third", t: { size: 56, weight: 700, y: 0.82, x: 0.5, align: "center", bg: "#000000", bgOpacity: 0.55, padding: 20 } },
  { l: "Top label", t: { size: 44, weight: 700, y: 0.12, align: "center", bg: "", shadow: true } },
  { l: "Kicker (left)", t: { size: 48, weight: 800, y: 0.85, x: 0.08, align: "left", bg: "#ffffff", bgOpacity: 0.92, padding: 18 } },
];
export function TextPanel() {
  const s = useVideo();
  return (
    <>
      <div className="ve-panel-head">Text<span className="spacer" /><button className="ve-btn sm primary" onClick={() => addTitle()}><Plus size={13} /> Title at playhead</button></div>
      <div className="ve-panel-body">
        <Section title="Presets">
          {TITLE_PRESETS.map((p) => <button key={p.l} className="ve-btn" style={{ justifyContent: "flex-start" }} onClick={() => { const c = addTitle(s.playhead, p.l === "Lower third" ? "Name\nWhat they do" : "Your title"); mutate((k) => { const x = k.clips.find((z) => z.id === c.id)!; Object.assign(x.text, p.t); }); }}><Type size={13} /> {p.l}</button>)}
        </Section>
        <Section title="Graphics">
          <p className="ve-hint">Overlay images/videos go on <b>V2</b> (B-roll) or <b>V3</b> (Graphics). Import them in Media and drag onto a lane, or ask the agent: <i>"add a lower-third at 12s saying…"</i>.</p>
          <p className="ve-hint">Text uses <b>drawtext</b> on export; the preview font is the same family, so what you see is what renders.</p>
        </Section>
      </div>
    </>
  );
}

// ---- CAPTIONS -------------------------------------------------------------
export function CaptionsPanel() {
  const s = useVideo(); const cap = s.comp.captions;
  const [kw, setKw] = useState(cap.keyWords.join(", "));
  useEffect(() => setKw(cap.keyWords.join(", ")), [cap.keyWords]);
  const hasTx = !!s.transcript;
  return (
    <>
      <div className="ve-panel-head">Captions<span className="spacer" /><Check label="On" checked={cap.enabled} onChange={(v) => setPath("captions.enabled", v)} /></div>
      <div className="ve-panel-body">
        <Section title="Transcript" right={hasTx ? <span className="ve-pill ok">{s.transcript!.words.length} words</span> : <span className="ve-pill">none</span>}>
          <button className="ve-btn primary" disabled={!s.assets.some((a) => a.hasAudio) || !!s.toolProgress} onClick={() => void runTool("video_transcribe", {}, "transcribing…")}><Mic2 size={13} /> {hasTx ? "Re-transcribe A-roll" : "Transcribe A-roll"}</button>
          {!s.status?.whisper && <p className="ve-hint" style={{ color: "var(--warn)" }}>Needs an OpenAI key (Settings) — Whisper word timestamps.</p>}
          {hasTx && <p className="ve-hint" style={{ maxHeight: 90, overflow: "auto", userSelect: "text" }}>{s.transcript!.text}</p>}
        </Section>
        <Section title="Style">
          <Seg value={cap.preset} options={[{ v: "pop", l: "Pop (Hyperframes)" }, { v: "karaoke", l: "Karaoke" }, { v: "plain", l: "Plain" }]} onChange={(v) => setPath("captions.preset", v)} />
          <div className="ve-row2">
            <Field label="Font"><input value={cap.font} onChange={(e) => setPath("captions.font", e.target.value)} /></Field>
            <Num label="Weight" value={cap.weight} step={100} min={100} max={900} onChange={(v) => setPath("captions.weight", v)} />
          </div>
          <Slider label="Size" value={cap.size} min={24} max={160} step={1} fmt={(v) => `${v}px`} onChange={(v) => setPath("captions.size", v)} />
          <Slider label="Vertical position" value={cap.y} min={0.05} max={0.95} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => setPath("captions.y", v)} />
          <div className="ve-row2">
            <Field label="Color"><div style={{ display: "flex", gap: 6, alignItems: "center" }}><input type="color" value={cap.color} onChange={(e) => setPath("captions.color", e.target.value)} /><input value={cap.color} onChange={(e) => setPath("captions.color", e.target.value)} /></div></Field>
            <Field label="Key color"><div style={{ display: "flex", gap: 6, alignItems: "center" }}><input type="color" value={cap.keyColor} onChange={(e) => setPath("captions.keyColor", e.target.value)} /><input value={cap.keyColor} onChange={(e) => setPath("captions.keyColor", e.target.value)} /></div></Field>
          </div>
          <div className="ve-row2">
            <Num label="Words / line" value={cap.wordsPerLine} min={1} max={10} onChange={(v) => setPath("captions.wordsPerLine", v)} />
            <Num label="Max chars" value={cap.maxChars} min={6} max={60} onChange={(v) => setPath("captions.maxChars", v)} />
          </div>
          <Field label="Key words (highlighted)"><input value={kw} placeholder="agent, AYGENT, free" onChange={(e) => setKw(e.target.value)} onBlur={() => setPath("captions.keyWords", kw.split(",").map((x) => x.trim()).filter(Boolean))} /></Field>
          <div style={{ display: "flex", gap: 14 }}>
            <Check label="Uppercase" checked={cap.uppercase} onChange={(v) => setPath("captions.uppercase", v)} />
            <Check label="Shadow" checked={cap.shadow} onChange={(v) => setPath("captions.shadow", v)} />
            <Check label="Behind subject" checked={cap.behindSubject} onChange={(v) => setPath("captions.behindSubject", v)} />
          </div>
          {cap.behindSubject && !s.comp.matte.enabled && <p className="ve-hint" style={{ color: "var(--warn)" }}>Needs a matte — Color → Subject matte.</p>}
        </Section>
        <p className="ve-hint">Captions follow the A-roll cuts automatically (word times are remapped through each clip's in/out). Export renders them with libass; the preview shows the same timing.</p>
      </div>
    </>
  );
}

// ---- COLOR ----------------------------------------------------------------
export function ColorPanel() {
  const s = useVideo(); const g = s.comp.color;
  const sel = s.comp.clips.find((c) => c.id === s.selection[0]);
  return (
    <>
      <div className="ve-panel-head">Color<span className="spacer" /><Check label="Adjustment layer" checked={s.comp.adjustmentLayer} onChange={(v) => setPath("adjustmentLayer", v)} /></div>
      <div className="ve-panel-body">
        <Section title="LUT (on the adjustment layer)" right={<button className="ve-btn sm" onClick={() => void pickLut().then((r) => { if (r) setPath("color.lut", r); })}><FolderOpen size={12} /> .cube</button>}>
          <select value={g.lut} onChange={(e) => setPath("color.lut", e.target.value)}>
            <option value="">none</option>
            {s.luts.map((l) => <option key={l} value={l}>{l.replace(/^luts\//, "")}</option>)}
          </select>
          <Slider label="Intensity" value={g.lutIntensity} min={0} max={1} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => setPath("color.lutIntensity", v)} />
          <p className="ve-hint">Drop a .cube into Media to add it here. The LUT applies on export (lut3d, tetrahedral) — the preview approximates exposure/contrast/saturation only.</p>
        </Section>
        <Section title="Global grade">
          <Slider label="S-curve" value={g.sCurve} min={0} max={1} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => setPath("color.sCurve", v)} />
          <Slider label="Exposure" value={g.exposure} min={-1} max={1} onChange={(v) => setPath("color.exposure", v)} />
          <Slider label="Contrast" value={g.contrast} min={-1} max={1} onChange={(v) => setPath("color.contrast", v)} />
          <Slider label="Saturation" value={g.saturation} min={-1} max={1} onChange={(v) => setPath("color.saturation", v)} />
          <Slider label="Temperature" value={g.temperature} min={-1} max={1} onChange={(v) => setPath("color.temperature", v)} />
        </Section>
        {sel && sel.type !== "text" && sel.type !== "audio" && (
          <Section title={`Clip grade · ${sel.name || sel.id}`}>
            <Slider label="S-curve" value={sel.color.sCurve} min={0} max={1} fmt={(v) => `${Math.round(v * 100)}%`} onChange={(v) => mutate((c) => { c.clips.find((k) => k.id === sel.id)!.color.sCurve = v; })} />
            <Slider label="Exposure" value={sel.color.exposure} min={-1} max={1} onChange={(v) => mutate((c) => { c.clips.find((k) => k.id === sel.id)!.color.exposure = v; })} />
            <Slider label="Contrast" value={sel.color.contrast} min={-1} max={1} onChange={(v) => mutate((c) => { c.clips.find((k) => k.id === sel.id)!.color.contrast = v; })} />
            <Slider label="Saturation" value={sel.color.saturation} min={-1} max={1} onChange={(v) => mutate((c) => { c.clips.find((k) => k.id === sel.id)!.color.saturation = v; })} />
            <Slider label="Temperature" value={sel.color.temperature} min={-1} max={1} onChange={(v) => mutate((c) => { c.clips.find((k) => k.id === sel.id)!.color.temperature = v; })} />
            <Field label="Clip LUT"><select value={sel.color.lut} onChange={(e) => mutate((c) => { c.clips.find((k) => k.id === sel.id)!.color.lut = e.target.value; })}><option value="">none</option>{s.luts.map((l) => <option key={l} value={l}>{l.replace(/^luts\//, "")}</option>)}</select></Field>
          </Section>
        )}
        <Section title="Subject matte (experimental)" right={s.comp.matte.enabled ? <span className="ve-pill ok">on</span> : null}>
          <p className="ve-hint">RobustVideoMatting separates you from the background so captions/graphics flagged <b>behind subject</b> render behind you. First run downloads PyTorch via uv (slow).</p>
          <button className="ve-btn" disabled={!s.status?.uv || !!s.toolProgress || !s.assets.some((a) => a.kind === "video")} onClick={() => { const a = s.comp.clips.find((c) => c.track === "V1" && c.type === "video")?.asset ?? s.assets.find((x) => x.kind === "video")?.id; if (a) void runTool("video_matte", { asset: a }, "matting (RVM)…"); }}><Layers size={13} /> Generate matte for A-roll</button>
          {!s.status?.uv && <p className="ve-hint" style={{ color: "var(--warn)" }}>uv isn't provisioned yet — enable any Python MCP server once (MCP Connections) to install it.</p>}
          {s.comp.matte.enabled && <><Slider label="Edge feather" value={s.comp.matte.feather} min={0} max={12} step={0.5} fmt={(v) => `${v}px`} onChange={(v) => setPath("matte.feather", v)} /><Check label="Matte enabled" checked={s.comp.matte.enabled} onChange={(v) => setPath("matte.enabled", v)} /></>}
        </Section>
      </div>
    </>
  );
}

// ---- AUDIO ----------------------------------------------------------------
export function AudioPanel() {
  const s = useVideo(); const a = s.comp.audio;
  const [creds, setCreds] = useState("");
  const aroll = s.comp.clips.find((c) => c.track === "V1" && c.type === "video");
  const enhanced = aroll && s.assets.find((x) => x.id === aroll.audioAsset);
  return (
    <>
      <div className="ve-panel-head">Audio</div>
      <div className="ve-panel-body">
        <Section title="Enhance dialogue" right={enhanced ? <span className="ve-pill ok">{a.enhance}</span> : null}>
          <div className="ve-row2">
            <button className="ve-btn primary" disabled={!aroll || !!s.toolProgress} onClick={() => aroll && void runTool("video_audio_enhance", { asset: aroll.asset, engine: "auphonic" }, "Auphonic…")}><Sparkles size={13} /> Auphonic</button>
            <button className="ve-btn" disabled={!aroll || !!s.toolProgress} onClick={() => aroll && void runTool("video_audio_enhance", { asset: aroll.asset, engine: "local" }, "enhancing…")}><Wand2 size={13} /> Local (ffmpeg)</button>
          </div>
          <Field label="Auphonic credentials (user:password or API token) — stored in Keychain"><div style={{ display: "flex", gap: 6 }}><input type="password" value={creds} placeholder="••••••" onChange={(e) => setCreds(e.target.value)} /><button className="ve-btn sm" onClick={() => { void invoke("video_set_auphonic", { credentials: creds }).then(() => { toast("Auphonic credentials saved", "ok"); setCreds(""); }).catch((e) => toast(String(e), "err")); }}>Save</button></div></Field>
          {enhanced && <p className="ve-hint">A-roll clips now play <b>{enhanced.name}</b>. Clear via Inspector → Audio asset.</p>}
        </Section>
        <Section title="Music ducking" right={<Check label="" checked={a.duck.enabled} onChange={(v) => setPath("audio.duck.enabled", v)} />}>
          <p className="ve-hint">Anything on <b>A2</b> is treated as music and ducks under voice (sidechain).</p>
          <Slider label="Music level" value={a.duck.musicDb} min={-40} max={0} step={0.5} fmt={(v) => `${v} dB`} onChange={(v) => setPath("audio.duck.musicDb", v)} />
          <Slider label="Ducked level" value={a.duck.duckedDb} min={-50} max={-6} step={0.5} fmt={(v) => `${v} dB`} onChange={(v) => setPath("audio.duck.duckedDb", v)} />
          <div className="ve-row2">
            <Num label="Attack" value={a.duck.attack} step={0.01} min={0.001} max={1} suffix="s" onChange={(v) => setPath("audio.duck.attack", v)} />
            <Num label="Release" value={a.duck.release} step={0.05} min={0.05} max={3} suffix="s" onChange={(v) => setPath("audio.duck.release", v)} />
          </div>
        </Section>
        <Section title="Master">
          <Slider label="Master gain" value={a.masterDb} min={-12} max={12} step={0.5} fmt={(v) => `${v} dB`} onChange={(v) => setPath("audio.masterDb", v)} />
          <Check label="Loudness normalize on export (−16 LUFS)" checked={a.loudnorm} onChange={(v) => setPath("audio.loudnorm", v)} />
        </Section>
      </div>
    </>
  );
}

// ---- EXPORT ---------------------------------------------------------------
const SIZES = [["16:9 · 1080p", 1920, 1080], ["16:9 · 4K", 3840, 2160], ["9:16 · 1080×1920", 1080, 1920], ["1:1 · 1080", 1080, 1080], ["4:5 · 1080×1350", 1080, 1350]] as const;
export function ExportPanel() {
  const s = useVideo(); const ex = s.comp.exports;
  useEffect(() => { void refreshRenders(); }, [s.project]);
  const upd = (i: number, p: Partial<Composition["exports"][number]>) => mutate((c) => { c.exports[i] = { ...c.exports[i], ...p }; });
  return (
    <>
      <div className="ve-panel-head">Export<span className="spacer" /><button className="ve-btn sm" onClick={() => mutate((c) => { c.exports.push({ name: `preset-${c.exports.length + 1}`, width: c.scene.width, height: c.scene.height, bitrate: "12M", codec: "h264", fps: 0 }); })}><Plus size={13} /> Preset</button></div>
      <div className="ve-panel-body">
        {s.render && (s.render.running || s.render.error) && (
          <Section title={s.render.running ? "Rendering" : "Failed"}>
            {s.render.running && <div className="ve-progress"><div className="bar"><i style={{ width: `${s.render.pct}%` }} /></div><span className="ve-mono">{s.render.pct}%</span><button className="ve-icon-btn" onClick={() => void cancelRender()} title="Cancel"><Square size={13} /></button></div>}
            {s.render.error && <p className="ve-hint" style={{ color: "var(--danger)", userSelect: "text" }}>{s.render.error}</p>}
          </Section>
        )}
        {ex.map((p, i) => (
          <Section key={i} title={p.name} right={<button className="ve-icon-btn" style={{ width: 22, height: 22 }} onClick={() => mutate((c) => { c.exports.splice(i, 1); })}><X size={12} /></button>}>
            <Field label="Name"><input value={p.name} onChange={(e) => upd(i, { name: e.target.value })} /></Field>
            <Field label="Size">
              <select value={`${p.width}x${p.height}`} onChange={(e) => { const [w, h] = e.target.value.split("x").map(Number); if (w && h) upd(i, { width: w, height: h }); }}>
                {SIZES.map(([l, w, h]) => <option key={l} value={`${w}x${h}`}>{l}</option>)}
                {!SIZES.some(([, w, h]) => w === p.width && h === p.height) && <option value={`${p.width}x${p.height}`}>{p.width}×{p.height}</option>}
              </select>
            </Field>
            <div className="ve-row3">
              <Num label="W" value={p.width} step={2} min={16} onChange={(v) => upd(i, { width: v })} />
              <Num label="H" value={p.height} step={2} min={16} onChange={(v) => upd(i, { height: v })} />
              <Field label="Bitrate"><input value={p.bitrate} onChange={(e) => upd(i, { bitrate: e.target.value })} /></Field>
            </div>
            <div className="ve-row2">
              <Field label="Codec"><select value={p.codec} onChange={(e) => upd(i, { codec: e.target.value as any })}><option value="h264">H.264 (VideoToolbox)</option><option value="hevc">HEVC</option><option value="prores">ProRes 422 HQ</option></select></Field>
              <Num label="FPS (0 = scene)" value={p.fps} step={1} min={0} max={120} onChange={(v) => upd(i, { fps: v })} />
            </div>
            <button className="ve-btn primary" disabled={!!s.render?.running || !s.comp.clips.length} onClick={() => void startRender(p)}><Download size={13} /> Export {p.width}×{p.height}</button>
            {p.width / p.height !== s.comp.scene.width / s.comp.scene.height && <p className="ve-hint" style={{ color: "var(--warn)" }}>Aspect differs from the scene ({s.comp.scene.width}×{s.comp.scene.height}) — clips will be fit with <b>cover</b>. For a proper reframe, ask the agent: "make a 9:16 version".</p>}
          </Section>
        ))}
        <Section title="Renders" right={<button className="ve-icon-btn" style={{ width: 22, height: 22 }} onClick={() => void reveal("renders")}><FolderOpen size={12} /></button>}>
          {s.renders.length === 0 && <p className="ve-hint">Nothing exported yet.</p>}
          {s.renders.map((r) => <button key={r.path} className="ve-btn" style={{ justifyContent: "space-between" }} onClick={() => void reveal(r.path)} title="Reveal in Finder"><span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{r.path.replace(/^renders\//, "")}</span><span className="ve-faint">{fmtBytes(r.bytes)}</span></button>)}
        </Section>
      </div>
    </>
  );
}

export function useProjectEffect(fn: () => void) { const p = useVideo().project; const ref = useRef(fn); ref.current = fn; useEffect(() => { ref.current(); }, [p]); }
export { set as _set };
