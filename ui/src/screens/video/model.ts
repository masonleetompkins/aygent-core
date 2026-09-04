// AYGENT — VIDEO v0.3: composition model (mirrors src-tauri/src/video_render.rs).
// The composition IS the edit. UI + agent both read/write it via video_* commands.

export type TrackId = string; // V1 V2 V3 … T1 A1 A2
export type ClipKind = "video" | "audio" | "image" | "text";

export type Transform = { x: number; y: number; scale: number; rotation: number; opacity: number };
export type TextStyle = {
  content: string; font: string; size: number; weight: number; color: string; align: "left" | "center" | "right";
  x: number; y: number; bg: string; bgOpacity: number; padding: number; animation: "fade" | "none"; shadow: boolean; maxWidth: number;
};
export type ColorGrade = { lut: string; lutIntensity: number; sCurve: number; exposure: number; contrast: number; saturation: number; temperature: number };
export type Keyframe = { t: number; db: number };
export type AudioFx = { fadeIn: number; fadeOut: number; keyframes: Keyframe[] };
export type Transition = { kind: string; duration: number };

export type Clip = {
  id: string; track: TrackId; type: ClipKind; asset: string; audioAsset: string; link: string; name: string;
  start: number; end: number; in: number; out: number; speed: number;
  volume: number; muted: boolean; hidden: boolean; fit: "cover" | "contain" | "none";
  transform: Transform; text: TextStyle; color: ColorGrade; audio: AudioFx;
  behindSubject: boolean; transitionIn: Transition; transitionOut: Transition;
};

export type GraphicsTheme = "bright-hud" | "eli5-dark";
export type GraphicsStyle = {
  instructions: string; styleGuide: string; styleGuideName: string;
  // structured brand style (stock = Mason Bright HUD)
  theme: GraphicsTheme; accent: string; accentInk: string; panel: string;
  ink: string; inkSoft: string; muted: string;
  positive: string; negative: string; amber: string;
  fontDisplay: string; fontMono: string;
  headlineWeight: number; captionSize: number; captionWeight: number; captionMaxWords: number;
  maxLines: number; align: string; maxVariations: number;
  motion: string; ease: string; placement: string; widthCapPct: number;
};
export function stockBrightHud(): GraphicsStyle {
  return {
    instructions: "", styleGuide: "", styleGuideName: "",
    theme: "bright-hud", accent: "#00cafc", accentInk: "#0090b4",
    panel: "#ffffff", ink: "#000000", inkSoft: "#3a3a3a", muted: "#6b6b6b",
    positive: "#3ddc84", negative: "#ff5470", amber: "#ffb020",
    fontDisplay: "SF Pro Display", fontMono: "SF Mono",
    headlineWeight: 800, captionSize: 58, captionWeight: 600, captionMaxWords: 3,
    maxLines: 2, align: "left", maxVariations: 2,
    motion: "subtle", ease: "cubic-bezier(0.16,1,0.3,1)",
    placement: "upper-right", widthCapPct: 80,
  };
}
/** ELI5 dark pack: the picture-locked dark system (one tap). */
export function eli5DarkPack(): GraphicsStyle {
  return { ...stockBrightHud(), theme: "eli5-dark", accent: "#00e6ff", accentInk: "#00b8d9", panel: "#0a0e15", ink: "#f2f8fb", inkSoft: "#eaf2f7", muted: "#7f93a3", headlineWeight: 900, ease: "power3.out", placement: "upper-third" };
}
export type Captions = {
  enabled: boolean; sourceAsset: string; keyWords: string[]; y: number;
};
export type Duck = { enabled: boolean; musicDb: number; duckedDb: number; attack: number; release: number };
export type AudioMix = { duck: Duck; enhance: string; masterDb: number; loudnorm: boolean };
export type Matte = { enabled: boolean; sourceAsset: string; alphaAsset: string; feather: number };
export type ExportPreset = { name: string; width: number; height: number; bitrate: string; codec: "h264" | "hevc" | "prores"; fps: number };

export type Composition = {
  version: 3;
  scene: { width: number; height: number; fps: number; background: string };
  clips: Clip[];
  graphics: GraphicsStyle;
  captions: Captions;
  audio: AudioMix;
  color: ColorGrade;
  adjustmentLayer: boolean;
  matte: Matte;
  exports: ExportPreset[];
};

export type Thumbs = { strip: string; stripFrames: number; stripStep: number; frameW: number; frameH: number; wave: string; waveW: number };
export type Asset = {
  id: string; name: string; kind: "video" | "audio" | "image"; rel: string; path: string; linked: boolean; size: number;
  duration: number; width: number; height: number; fps: number; hasVideo: boolean; hasAudio: boolean; audioChannels: number; codec: string;
  thumbs: Thumbs | null; imported: number;
  online?: boolean;        // computed by video_load: media readable right now (false = drive unplugged / moved)
};

export type Word = { w: string; s: number; e: number };
export type Transcript = { asset: string; language: string; text: string; words: Word[]; segments: { text: string; s: number; e: number }[]; created: number };

export const TRACK_ORDER: TrackId[] = ["T1", "V3", "V2", "V1", "A1", "A2"]; // top = composited on top: titles over graphics over B-roll over A-roll
export const TRACK_KIND = (t: TrackId): "video" | "text" | "audio" => t.startsWith("T") ? "text" : t.startsWith("A") ? "audio" : "video";
export const TRACK_LABEL: Record<string, string> = { V3: "Graphics", V2: "B-roll", V1: "A-roll", T1: "Overlays", A1: "Voice", A2: "Music" };

export const uid = (p = "c") => `${p}${Date.now().toString(36)}${Math.floor(Math.random() * 1296).toString(36)}`;

export function defaultTransform(): Transform { return { x: 0, y: 0, scale: 1, rotation: 0, opacity: 1 }; }
export function defaultText(content = "Title"): TextStyle {
  return { content, font: "Helvetica Neue", size: 96, weight: 700, color: "#ffffff", align: "center", x: 0.5, y: 0.5, bg: "", bgOpacity: 0.6, padding: 24, animation: "fade", shadow: true, maxWidth: 0.8 };
}
export function defaultGrade(): ColorGrade { return { lut: "", lutIntensity: 1, sCurve: 0, exposure: 0, contrast: 0, saturation: 0, temperature: 0 }; }

export function blankComposition(): Composition {
  return {
    version: 3,
    scene: { width: 1920, height: 1080, fps: 30, background: "#000000" },
    clips: [],
    graphics: stockBrightHud(),
    captions: { enabled: false, sourceAsset: "", keyWords: [], y: 0.78 },
    audio: { duck: { enabled: true, musicDb: -18, duckedDb: -30, attack: 0.02, release: 0.4 }, enhance: "auphonic", masterDb: 0, loudnorm: false },
    color: { ...defaultGrade(), sCurve: 0.35 },
    adjustmentLayer: true,
    matte: { enabled: false, sourceAsset: "", alphaAsset: "", feather: 0 },
    exports: [
      { name: "landscape", width: 1920, height: 1080, bitrate: "12M", codec: "h264", fps: 0 },
      { name: "vertical", width: 1080, height: 1920, bitrate: "10M", codec: "h264", fps: 0 },
    ],
  };
}

export function newClip(partial: Partial<Clip> & { track: TrackId; type: ClipKind }): Clip {
  return {
    id: uid(), asset: "", audioAsset: "", link: "", name: "", start: 0, end: 4, in: 0, out: 4, speed: 1, volume: 0, muted: false, hidden: false, fit: "cover",
    transform: defaultTransform(), text: defaultText(), color: defaultGrade(), audio: { fadeIn: 0, fadeOut: 0, keyframes: [] },
    behindSubject: false, transitionIn: { kind: "", duration: 0 }, transitionOut: { kind: "", duration: 0 },
    ...partial,
  };
}

const num = (v: unknown, fb: number) => (typeof v === "number" && isFinite(v) ? v : fb);
const str = (v: unknown, fb: string) => (typeof v === "string" ? v : fb);
const bool = (v: unknown, fb: boolean) => (typeof v === "boolean" ? v : fb);

/** Tolerant normalize: accepts v3 and the v0.2 `sequence[]` shape. */
export function normalize(raw: unknown, assets: Asset[] = []): Composition {
  const b = blankComposition();
  if (typeof raw !== "object" || raw === null) return b;
  const r = raw as Record<string, any>;
  let clipsRaw: any[] = Array.isArray(r.clips) ? r.clips : [];
  if (!Array.isArray(r.clips) && Array.isArray(r.sequence)) {
    const byName = new Map(assets.map((a) => [a.name, a.id]));
    clipsRaw = r.sequence.map((c: any) => ({
      id: c.id, track: c.track, type: c.track === "T1" ? "text" : String(c.track).startsWith("A") ? "audio" : "video",
      asset: byName.get(c.src) ?? "", name: c.name ?? c.src, start: c.start, end: c.end, in: c.sourceIn, out: c.sourceOut,
      volume: c.volume, muted: c.muted, hidden: c.hidden, text: { content: c.text ?? "" },
    }));
  }
  const clips: Clip[] = clipsRaw.filter((c) => c && typeof c === "object").map((c, i) => {
    const start = num(c.start, 0);
    const end = Math.max(start + 0.04, num(c.end, start + 4));
    const base = newClip({ track: str(c.track, "V1"), type: (["video", "audio", "image", "text"].includes(c.type) ? c.type : "video") as ClipKind });
    const inn = num(c.in, 0);
    return {
      ...base,
      id: str(c.id, "") || `clip-${i}`,
      asset: str(c.asset, ""), audioAsset: str(c.audioAsset, ""), link: str(c.link, ""), name: str(c.name, ""),
      start, end, in: inn, out: Math.max(inn + 0.04, num(c.out, inn + (end - start))), speed: num(c.speed, 1) || 1,
      volume: num(c.volume, 0), muted: bool(c.muted, false), hidden: bool(c.hidden, false), fit: (["cover", "contain", "none"].includes(c.fit) ? c.fit : "cover"),
      transform: { ...base.transform, ...(c.transform ?? {}) },
      text: { ...base.text, ...(c.text ?? {}) },
      color: { ...base.color, ...(c.color ?? {}) },
      audio: { fadeIn: num(c.audio?.fadeIn, 0), fadeOut: num(c.audio?.fadeOut, 0), keyframes: Array.isArray(c.audio?.keyframes) ? c.audio.keyframes : [] },
      behindSubject: bool(c.behindSubject, false),
      transitionIn: { kind: str(c.transitionIn?.kind, ""), duration: num(c.transitionIn?.duration, 0) },
      transitionOut: { kind: str(c.transitionOut?.kind, ""), duration: num(c.transitionOut?.duration, 0) },
    };
  });
  return {
    version: 3,
    scene: { width: num(r.scene?.width, 1920), height: num(r.scene?.height, 1080), fps: num(r.scene?.fps, 30) || 30, background: str(r.scene?.background, "#000000") },
    clips,
    graphics: { ...stockBrightHud(), ...(r.graphics ?? {}) },
    captions: { ...b.captions, ...(r.captions ?? {}), keyWords: Array.isArray(r.captions?.keyWords) ? r.captions.keyWords : [] },
    audio: { ...b.audio, ...(r.audio ?? {}), duck: { ...b.audio.duck, ...(r.audio?.duck ?? {}) } },
    color: { ...b.color, ...(r.color ?? {}) },
    adjustmentLayer: bool(r.adjustmentLayer, true),
    matte: { ...b.matte, ...(r.matte ?? {}) },
    exports: Array.isArray(r.exports) && r.exports.length ? r.exports.map((e: any) => ({ name: str(e.name, "export"), width: num(e.width, 1920), height: num(e.height, 1080), bitrate: str(e.bitrate, "12M"), codec: (["h264", "hevc", "prores"].includes(e.codec) ? e.codec : "h264"), fps: num(e.fps, 0) })) : b.exports,
  };
}

export const duration = (c: Composition) => c.clips.reduce((m, k) => (k.hidden ? m : Math.max(m, k.end)), 0);
export const clipDur = (c: Clip) => Math.max(0, c.end - c.start);

export function fmtTime(t: number, fps = 30): string {
  const s = Math.max(0, t);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = Math.floor(s % 60), f = Math.floor((s - Math.floor(s)) * fps);
  const mm = String(m).padStart(2, "0"), ss = String(sec).padStart(2, "0"), ff = String(f).padStart(2, "0");
  return h ? `${h}:${mm}:${ss}:${ff}` : `${mm}:${ss}:${ff}`;
}
export function fmtDur(t: number): string {
  const s = Math.max(0, t); const m = Math.floor(s / 60); const sec = s - m * 60;
  return m ? `${m}:${sec.toFixed(1).padStart(4, "0")}` : `${sec.toFixed(2)}s`;
}
export const fmtBytes = (b: number) => b > 1e9 ? `${(b / 1e9).toFixed(2)} GB` : b > 1e6 ? `${(b / 1e6).toFixed(1)} MB` : `${Math.round(b / 1e3)} KB`;

/** Media URL served by the jailed aygent-media:// scheme. */
export function mediaUrl(agentId: string, project: string, kind: "asset" | "cache" | "render" | "lut", name: string, bust?: number): string {
  const base = `aygent-media://localhost/${encodeURIComponent(agentId)}/${encodeURIComponent(project)}/${kind}/${encodeURIComponent(name.replace(/^\.cache\//, "").replace(/^renders\//, "").replace(/^luts\//, ""))}`;
  return bust ? `${base}?v=${bust}` : base;
}
