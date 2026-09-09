// AYGENT — VIDEO v0.3 render engine.
//
// composition.json  →  one ffmpeg filter graph  →  MP4 (or a single frame).
//
// The composition is the edit. UI and agent both write it; this module is the
// only thing that turns it into pixels. Everything runs through the PROVISIONED
// ffmpeg (provision.rs) with a filter script written into Video/<p>/.cache/, so
// no shell quoting and nothing the agent names gets executed.
//
// Visual stacking (bottom → top): V1 < V2 < V3 … < T1 (caption overlays play only
// when captions.enabled), then the global grade (LUT + S-curve = "adjustment layer")
// on the composite. Captions + graphics are Hyperframes transparent-overlay video
// clips — there is no ASS/drawtext caption pipeline anymore. If a matte is enabled,
// layers flagged behindSubject are composited BETWEEN the a-roll and its
// alpha-merged foreground so overlays sit behind Mason.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::broker::Broker;
use crate::video::{self, Asset, Manifest};

// ---------------------------------------------------------------------------
// Composition schema (v3)
// ---------------------------------------------------------------------------

fn d_one() -> f64 { 1.0 }
fn d_neg3() -> f64 { -3.0 }
fn d_true() -> bool { true }
fn d_half() -> f64 { 0.5 }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Scene { pub width: u32, pub height: u32, pub fps: f64, pub background: String }
impl Default for Scene { fn default() -> Self { Self { width: 1920, height: 1080, fps: 30.0, background: "#000000".into() } } }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Transform { pub x: f64, pub y: f64, #[serde(default = "d_one")] pub scale: f64, pub rotation: f64, #[serde(default = "d_one")] pub opacity: f64 }
impl Default for Transform { fn default() -> Self { Self { x: 0.0, y: 0.0, scale: 1.0, rotation: 0.0, opacity: 1.0 } } }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct TextStyle {
    pub content: String,
    pub font: String,
    pub size: f64,
    pub weight: u32,
    pub color: String,
    pub align: String,             // left | center | right
    #[serde(default = "d_half")] pub x: f64, // 0..1 of scene width (anchor center)
    #[serde(default = "d_half")] pub y: f64, // 0..1 of scene height
    pub bg: String,                // "" or #rrggbb
    pub bg_opacity: f64,
    pub padding: f64,
    pub animation: String,         // fade | none
    pub shadow: bool,
    pub max_width: f64,            // 0..1 of scene width for wrapping (0 = no wrap)
}
impl Default for TextStyle {
    fn default() -> Self { Self { content: String::new(), font: "Helvetica Neue".into(), size: 96.0, weight: 700, color: "#ffffff".into(), align: "center".into(), x: 0.5, y: 0.5, bg: String::new(), bg_opacity: 0.6, padding: 24.0, animation: "fade".into(), shadow: true, max_width: 0.8 } }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ColorGrade {
    pub lut: String,               // project-relative "luts/x.cube"
    #[serde(default = "d_one")] pub lut_intensity: f64,
    pub s_curve: f64,              // 0..1 strength
    pub exposure: f64,             // -1..1
    pub contrast: f64,             // -1..1
    pub saturation: f64,           // -1..1
    pub temperature: f64,          // -1..1 (warm +)
}
impl ColorGrade { fn is_identity(&self) -> bool { self.lut.is_empty() && self.s_curve == 0.0 && self.exposure == 0.0 && self.contrast == 0.0 && self.saturation == 0.0 && self.temperature == 0.0 } }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Keyframe { pub t: f64, pub db: f64 }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioFx { pub fade_in: f64, pub fade_out: f64, pub keyframes: Vec<Keyframe> }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Transition { pub kind: String, pub duration: f64 }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Clip {
    pub id: String,
    pub track: String,             // V1 V2 V3 T1 A1 A2
    #[serde(rename = "type")] pub kind: String, // video | audio | image | text
    pub asset: String,             // asset id
    pub audio_asset: String,       // optional replacement audio (Auphonic result)
    pub link: String,              // links a V picture clip to its A1 waveform partner ("" = unlinked)
    pub name: String,
    pub start: f64,
    pub end: f64,
    #[serde(rename = "in")] pub in_: f64,
    pub out: f64,
    #[serde(default = "d_one")] pub speed: f64,
    pub volume: f64,               // dB
    pub muted: bool,
    pub hidden: bool,
    pub fit: String,               // cover | contain | none
    pub transform: Transform,
    pub text: TextStyle,
    pub color: ColorGrade,
    pub audio: AudioFx,
    pub behind_subject: bool,
    pub transition_in: Transition,
    pub transition_out: Transition,
}
impl Default for Clip {
    fn default() -> Self { Self { id: String::new(), track: "V1".into(), kind: "video".into(), asset: String::new(), audio_asset: String::new(), link: String::new(), name: String::new(), start: 0.0, end: 0.0, in_: 0.0, out: 0.0, speed: 1.0, volume: 0.0, muted: false, hidden: false, fit: "cover".into(), transform: Transform::default(), text: TextStyle::default(), color: ColorGrade::default(), audio: AudioFx::default(), behind_subject: false, transition_in: Transition::default(), transition_out: Transition::default() } }
}
impl Clip {
    pub fn dur(&self) -> f64 { (self.end - self.start).max(0.0) }
    fn is_visual(&self) -> bool { matches!(self.kind.as_str(), "video" | "image" | "text") && !self.hidden }
    fn has_audio_role(&self) -> bool { matches!(self.kind.as_str(), "video" | "audio") && !self.muted }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct GraphicsStyle {
    pub instructions: String,      // plain-text style directions for Hyperframes overlays
    pub style_guide: String,       // extracted text of the uploaded style guide (.md/.txt/.rtf/.pdf/.docx)
    pub style_guide_name: String,  // original file name of the guide
    // ---- structured brand style (stock = Mason Bright HUD; ELI5 dark = one tap) ----
    pub theme: String,             // "bright-hud" | "eli5-dark"
    pub accent: String,            // brand accent (#00cafc Bright HUD, #00e6ff ELI5 dark)
    pub accent_ink: String,        // readable accent on light (#0090b4) / deep (#00b8d9)
    pub panel: String,             // panel surface (#ffffff / #0a0e15)
    pub ink: String,               // headline ink (#000000 / #f2f8fb)
    pub ink_soft: String,          // body ink (#3a3a3a / #eaf2f7)
    pub muted: String,             // secondary (#6b6b6b / #7f93a3)
    pub positive: String,          // up metrics (#3ddc84)
    pub negative: String,          // errors / "the gap" (#ff5470)
    pub amber: String,             // question prompts (#ffb020)
    pub font_display: String,      // "SF Pro Display"
    pub font_mono: String,         // "SF Mono"
    pub headline_weight: u32,      // 800 Bright HUD / 900 ELI5
    pub caption_size: u32,         // px at 1080x1920 reference (58)
    pub caption_weight: u32,       // 600 semibold — never heavy
    pub caption_max_words: u32,    // max 3, always one line
    pub max_lines: u32,            // 2 lines max per graphic
    pub align: String,             // "left" — same left axis, no centered mixes
    pub max_variations: u32,       // max 2 text styles per graphic
    pub motion: String,            // "subtle" (stock) | "kinetic"
    pub ease: String,              // cubic-bezier(0.16,1,0.3,1) / power3.out
    pub placement: String,         // "upper-right" | "upper-third" | "center"
    pub width_cap_pct: f64,        // content within 80% of frame width
}
impl Default for GraphicsStyle {
    fn default() -> Self { stock_bright_hud() }
}
fn stock_bright_hud() -> GraphicsStyle {
    GraphicsStyle {
        instructions: String::new(), style_guide: String::new(), style_guide_name: String::new(),
        theme: "bright-hud".into(), accent: "#00cafc".into(), accent_ink: "#0090b4".into(),
        panel: "#ffffff".into(), ink: "#000000".into(), ink_soft: "#3a3a3a".into(), muted: "#6b6b6b".into(),
        positive: "#3ddc84".into(), negative: "#ff5470".into(), amber: "#ffb020".into(),
        font_display: "SF Pro Display".into(), font_mono: "SF Mono".into(),
        headline_weight: 800, caption_size: 58, caption_weight: 600, caption_max_words: 3,
        max_lines: 2, align: "left".into(), max_variations: 2,
        motion: "subtle".into(), ease: "cubic-bezier(0.16,1,0.3,1)".into(),
        placement: "upper-right".into(), width_cap_pct: 80.0,
    }
}
/// ELI5 dark pack (one tap in the Graphics panel): the picture-locked dark system.
pub fn eli5_dark_pack() -> GraphicsStyle {
    GraphicsStyle {
        theme: "eli5-dark".into(), accent: "#00e6ff".into(), accent_ink: "#00b8d9".into(),
        panel: "#0a0e15".into(), ink: "#f2f8fb".into(), ink_soft: "#eaf2f7".into(), muted: "#7f93a3".into(),
        headline_weight: 900, ease: "power3.out".into(), placement: "upper-third".into(),
        ..stock_bright_hud()
    }
}

/// One user-edited caption line (timeline time): each word carries its own
/// timecode + editable text. Empty `lines` = auto-group from the transcript.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptionWordEdit { pub w: String, pub s: f64, pub e: f64 }
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptionLineEdit { pub s: f64, pub e: f64, pub words: Vec<CaptionWordEdit> }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Captions {
    pub enabled: bool,             // the T1 caption overlay clip plays
    pub source_asset: String,      // transcript's asset id ("" = first a-roll)
    pub key_words: Vec<String>,    // words to highlight (case-insensitive)
    pub y: f64,                    // 0..1 center from top (passed to the Hyperframes build)
    pub lines: Vec<CaptionLineEdit>, // user-edited caption sections (timeline time; empty = auto)
}
impl Default for Captions {
    fn default() -> Self { Self { enabled: false, source_asset: String::new(), key_words: vec![], y: 0.78, lines: vec![] } }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Duck { pub enabled: bool, pub music_db: f64, pub ducked_db: f64, pub attack: f64, pub release: f64 }
impl Default for Duck { fn default() -> Self { Self { enabled: false, music_db: -18.0, ducked_db: -30.0, attack: 0.02, release: 0.4 } } }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioMix { pub duck: Duck, pub enhance: String, pub master_db: f64, pub loudnorm: bool, #[serde(default)] pub track_gain: std::collections::HashMap<String, f64>, #[serde(default = "d_one")] pub clean_mix: f64 }
impl Default for AudioMix { fn default() -> Self { Self { duck: Duck::default(), enhance: "neural".into(), master_db: 0.0, loudnorm: false, track_gain: Default::default(), clean_mix: 1.0 } } }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Matte { pub enabled: bool, pub source_asset: String, pub alpha_asset: String, pub feather: f64 }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportPreset { pub name: String, pub width: u32, pub height: u32, pub bitrate: String, pub codec: String, pub fps: f64 }
impl Default for ExportPreset { fn default() -> Self { Self { name: "landscape".into(), width: 1920, height: 1080, bitrate: "12M".into(), codec: "h264".into(), fps: 0.0 } } }

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Composition {
    pub version: u32,
    pub scene: Scene,
    pub clips: Vec<Clip>,
    pub graphics: GraphicsStyle,   // Hyperframes overlay style (instructions + guide)
    pub captions: Captions,
    pub audio: AudioMix,
    pub color: ColorGrade,         // the adjustment layer (on the composite)
    #[serde(default = "d_true")] pub adjustment_layer: bool,
    pub matte: Matte,
    pub exports: Vec<ExportPreset>,
}
impl Default for Composition {
    fn default() -> Self {
        Self { version: 3, scene: Scene::default(), clips: vec![], graphics: GraphicsStyle::default(), captions: Captions::default(), audio: AudioMix::default(), color: ColorGrade { s_curve: 0.35, ..Default::default() }, adjustment_layer: true, matte: Matte::default(),
            exports: vec![ExportPreset::default(), ExportPreset { name: "vertical".into(), width: 1080, height: 1920, bitrate: "10M".into(), ..Default::default() }] }
    }
}
impl Composition {
    pub fn duration(&self) -> f64 { self.clips.iter().filter(|c| !c.hidden && c.kind != "review").map(|c| c.end).fold(0.0, f64::max) }
}

pub fn blank_composition_json() -> serde_json::Value { serde_json::to_value(Composition::default()).unwrap_or_default() }

/// Parse + upgrade a composition (v0.2 `sequence[]` with `src` → v3 `clips[]`).
pub fn parse_composition(raw: &serde_json::Value, assets: &[Asset]) -> Result<Composition, String> {
    let mut v = raw.clone();
    if v.get("clips").is_none() {
        if let Some(seq) = v.get("sequence").and_then(|s| s.as_array()).cloned() {
            let by_name: HashMap<String, String> = assets.iter().map(|a| (a.name.clone(), a.id.clone())).collect();
            let clips: Vec<serde_json::Value> = seq.iter().map(|c| {
                let track = c["track"].as_str().unwrap_or("V1");
                let src = c["src"].as_str().unwrap_or("");
                let kind = if track == "T1" { "text" } else if track.starts_with('A') { "audio" } else { "video" };
                let start = c["start"].as_f64().unwrap_or(0.0);
                let end = c["end"].as_f64().unwrap_or(start + 4.0);
                serde_json::json!({
                    "id": c["id"], "track": track, "type": kind, "asset": by_name.get(src).cloned().unwrap_or_default(),
                    "name": c["name"].as_str().unwrap_or(src), "start": start, "end": end,
                    "in": c["sourceIn"].as_f64().unwrap_or(0.0), "out": c["sourceOut"].as_f64().unwrap_or(end - start),
                    "volume": c["volume"].as_f64().unwrap_or(0.0), "muted": c["muted"].as_bool().unwrap_or(false), "hidden": c["hidden"].as_bool().unwrap_or(false),
                    "text": { "content": c["text"].as_str().unwrap_or("") }
                })
            }).collect();
            v["clips"] = serde_json::Value::Array(clips);
        }
    }
    v["version"] = serde_json::json!(3);
    let mut comp: Composition = serde_json::from_value(v).map_err(|e| format!("composition schema: {e}"))?;
    for (i, c) in comp.clips.iter_mut().enumerate() {
        if c.id.is_empty() { c.id = format!("clip-{i}"); }
        if c.end <= c.start { c.end = c.start + 4.0; }
        if c.speed <= 0.0 { c.speed = 1.0; }
        if c.out <= c.in_ { c.out = c.in_ + c.dur() * c.speed; }
    }
    if comp.scene.width < 16 || comp.scene.height < 16 { return Err("scene too small".into()); }
    if comp.scene.fps <= 0.0 { comp.scene.fps = 30.0; }
    Ok(comp)
}

// ---------------------------------------------------------------------------
// Transcript (word-level) — shared with video_tools
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Word { pub w: String, pub s: f64, pub e: f64 }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Segment { pub text: String, pub s: f64, pub e: f64 }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Transcript { pub asset: String, pub language: String, pub text: String, pub words: Vec<Word>, pub segments: Vec<Segment>, pub created: i64 }

// ---------------------------------------------------------------------------
// Filter-graph construction
// ---------------------------------------------------------------------------

/// Escape a value for use inside a filter option in a filter_complex SCRIPT.
fn fesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(c, '\\' | '\'' | ':' | ',' | ';' | '[' | ']' | '=') { o.push('\\'); }
        o.push(c);
    }
    o
}

fn hex_color(s: &str, fb: &str) -> String {
    let t = s.trim();
    if t.len() == 7 && t.starts_with('#') && t[1..].chars().all(|c| c.is_ascii_hexdigit()) { t.to_string() } else { fb.to_string() }
}
fn ff_color(s: &str, fb: &str) -> String { hex_color(s, fb).replacen('#', "0x", 1) }

fn rank(track: &str) -> i32 {
    let (p, n) = track.split_at(1);
    let n: i32 = n.parse().unwrap_or(1);
    match p { "V" => n, "T" => 100 + n, _ => 50 }
}

struct Graph { lines: Vec<String>, n: usize }
impl Graph {
    fn new() -> Self { Self { lines: vec![], n: 0 } }
    fn label(&mut self, p: &str) -> String { self.n += 1; format!("{p}{}", self.n) }
    fn push(&mut self, s: String) { self.lines.push(s); }
}

#[allow(dead_code)]
struct Ctx<'a> {
    comp: &'a Composition,
    proj: &'a Path,
    assets: &'a HashMap<String, Asset>,
    abs: &'a HashMap<String, PathBuf>, // asset id → readable abs path
    inputs: Vec<String>,                // ffmpeg -i args (flattened)
    input_idx: HashMap<String, usize>,  // key (asset id or "img:<id>:<dur>") → input index
    w: u32, h: u32, fps: f64, dur: f64,
}

impl<'a> Ctx<'a> {
    fn input_for(&mut self, asset_id: &str, image_dur: Option<f64>) -> Option<usize> {
        let key = match image_dur { Some(d) => format!("img:{asset_id}:{d:.3}"), None => asset_id.to_string() };
        if let Some(i) = self.input_idx.get(&key) { return Some(*i); }
        let abs = self.abs.get(asset_id)?;
        let idx = self.input_idx.len();
        if let Some(d) = image_dur {
            self.inputs.extend(["-loop".into(), "1".into(), "-framerate".into(), format!("{}", self.fps), "-t".into(), format!("{d:.3}"), "-i".into(), abs.to_string_lossy().to_string()]);
        } else {
            self.inputs.extend(["-i".into(), abs.to_string_lossy().to_string()]);
        }
        self.input_idx.insert(key, idx);
        Some(idx)
    }
}

fn color_chain(g: &ColorGrade, proj: &Path, graph: &mut Graph, input: String) -> String {
    if g.is_identity() { return input; }
    let mut cur = input;
    let mut f: Vec<String> = vec![];
    if g.exposure != 0.0 || g.contrast != 0.0 || g.saturation != 0.0 {
        f.push(format!("eq=contrast={:.3}:brightness={:.3}:saturation={:.3}", 1.0 + g.contrast * 0.6, g.exposure * 0.25, (1.0 + g.saturation).max(0.0)));
    }
    if g.temperature != 0.0 {
        let t = g.temperature * 0.25;
        f.push(format!("colorbalance=rs={t:.3}:bs={:.3}:rm={:.3}:bm={:.3}", -t, t * 0.6, -t * 0.6));
    }
    if g.s_curve > 0.0 {
        let k = g.s_curve.clamp(0.0, 1.0) * 0.07;
        f.push(format!("curves=all='0/0 0.25/{:.3} 0.5/0.5 0.75/{:.3} 1/1'", 0.25 - k, 0.75 + k));
    }
    if !f.is_empty() {
        let out = graph.label("g");
        graph.push(format!("[{cur}]{}[{out}]", f.join(",")));
        cur = out;
    }
    if !g.lut.is_empty() && !g.lut.contains("..") {
        let lut = proj.join(&g.lut);
        if lut.is_file() {
            let lut_s = fesc(&lut.to_string_lossy());
            let k = g.lut_intensity.clamp(0.0, 1.0);
            if k >= 0.999 {
                let out = graph.label("l");
                graph.push(format!("[{cur}]lut3d=file='{lut_s}':interp=tetrahedral[{out}]"));
                cur = out;
            } else if k > 0.001 {
                let a = graph.label("la"); let b = graph.label("lb"); let b2 = graph.label("lc"); let out = graph.label("l");
                graph.push(format!("[{cur}]split[{a}][{b}]"));
                graph.push(format!("[{b}]lut3d=file='{lut_s}':interp=tetrahedral[{b2}]"));
                graph.push(format!("[{a}][{b2}]blend=all_mode=normal:all_opacity={k:.3}[{out}]"));
                cur = out;
            }
        }
    }
    cur
}

/// Geometry chain: fit → user scale → rotation → opacity. Returns the label.
fn geom_chain(ctx: &Ctx, c: &Clip, graph: &mut Graph, input: String) -> String {
    let (w, h) = (ctx.w, ctx.h);
    let mut f: Vec<String> = vec![];
    match c.fit.as_str() {
        "contain" => f.push(format!("scale={w}:{h}:force_original_aspect_ratio=decrease:flags=bicubic")),
        "none" => {}
        _ => f.push(format!("scale={w}:{h}:force_original_aspect_ratio=increase:flags=bicubic")),
    }
    let s = c.transform.scale;
    if (s - 1.0).abs() > 0.001 && s > 0.0 { f.push(format!("scale=trunc(iw*{s:.4}/2)*2:trunc(ih*{s:.4}/2)*2:flags=bicubic")); }
    f.push("format=rgba".into());
    if c.transform.rotation.abs() > 0.01 { f.push(format!("rotate={:.5}:c=none@0:ow=rotw({:.5}):oh=roth({:.5})", c.transform.rotation.to_radians(), c.transform.rotation.to_radians(), c.transform.rotation.to_radians())); }
    let op = c.transform.opacity.clamp(0.0, 1.0);
    if op < 0.999 { f.push(format!("colorchannelmixer=aa={op:.3}")); }
    // transitions: fade in/out on alpha
    if c.transition_in.kind == "fade" && c.transition_in.duration > 0.0 { f.push(format!("fade=t=in:st=0:d={:.3}:alpha=1", c.transition_in.duration)); }
    if c.transition_out.kind == "fade" && c.transition_out.duration > 0.0 { f.push(format!("fade=t=out:st={:.3}:d={:.3}:alpha=1", (c.dur() - c.transition_out.duration).max(0.0), c.transition_out.duration)); }
    let out = graph.label("v");
    graph.push(format!("[{input}]{}[{out}]", f.join(",")));
    out
}

/// A visual clip as a positioned, timed RGBA stream (starts at c.start on the timeline).
fn visual_stream(ctx: &mut Ctx, c: &Clip, graph: &mut Graph, alpha_of: Option<&str>) -> Option<String> {
    let (start, dur) = (c.start, c.dur());
    let base = match c.kind.as_str() {
        "video" => {
            let asset_id = alpha_of.unwrap_or(&c.asset);
            let i = ctx.input_for(asset_id, None)?;
            let l = graph.label("t");
            let sp = c.speed;
            let out = c.in_ + dur * sp;
            let mut f = format!("trim=start={:.4}:end={:.4},setpts=(PTS-STARTPTS)/{sp:.5}", c.in_, out);
            f.push_str(&format!(",fps={:.4}", ctx.fps));
            graph.push(format!("[{i}:v]{f}[{l}]"));
            l
        }
        "image" => {
            let i = ctx.input_for(&c.asset, Some(dur))?;
            let l = graph.label("t");
            graph.push(format!("[{i}:v]fps={:.4},trim=duration={dur:.4},setpts=PTS-STARTPTS[{l}]", ctx.fps));
            l
        }
        _ => return None,
    };
    let g = geom_chain(ctx, c, graph, base);
    let colored = if alpha_of.is_some() { g } else { color_chain(&c.color, ctx.proj, graph, g) };
    let out = graph.label("p");
    graph.push(format!("[{colored}]setpts=PTS+{start:.4}/TB[{out}]"));
    Some(out)
}

fn overlay(ctx: &Ctx, graph: &mut Graph, bg: String, fg: String, c: &Clip) -> String {
    let out = graph.label("o");
    let (x, y) = (c.transform.x, c.transform.y);
    graph.push(format!(
        "[{bg}][{fg}]overlay=x='(W-w)/2+({x:.2})':y='(H-h)/2+({y:.2})':enable='between(t,{:.4},{:.4})':eof_action=pass:repeatlast=0:format=auto[{out}]",
        c.start, c.end - 0.0005
    ));
    let _ = ctx;
    out
}

/// drawtext for a title clip. Written to a textfile to dodge escaping.
fn text_overlay(ctx: &Ctx, graph: &mut Graph, bg: String, c: &Clip, n: usize) -> String {
    let t = &c.text;
    let dir = ctx.proj.join(".cache");
    let _ = std::fs::create_dir_all(&dir);
    let tf = dir.join(format!("text-{n}-{}.txt", c.id.chars().filter(|ch| ch.is_ascii_alphanumeric()).collect::<String>()));
    let content = wrap_text(&t.content, if t.max_width > 0.0 { ((ctx.w as f64 * t.max_width) / (t.size * 0.55)).max(4.0) as usize } else { usize::MAX });
    let _ = std::fs::write(&tf, content);
    let (s, e) = (c.start, c.end);
    let fade = if t.animation == "fade" { 0.25f64.min(c.dur() / 3.0) } else { 0.0 };
    let alpha = if fade > 0.0 {
        format!(":alpha='if(lt(t,{s:.3}+{fade:.3}),(t-{s:.3})/{fade:.3},if(gt(t,{e:.3}-{fade:.3}),({e:.3}-t)/{fade:.3},1))'")
    } else { String::new() };
    let xexpr = match t.align.as_str() { "left" => format!("{:.1}", ctx.w as f64 * t.x), "right" => format!("{:.1}-text_w", ctx.w as f64 * t.x), _ => format!("{:.1}-text_w/2", ctx.w as f64 * t.x) };
    let yexpr = format!("{:.1}-text_h/2", ctx.h as f64 * t.y);
    let mut f = format!(
        "drawtext=textfile='{}':font='{}':fontsize={:.0}:fontcolor={}:x='{xexpr}':y='{yexpr}':line_spacing={:.0}:enable='between(t,{s:.4},{e:.4})'{alpha}",
        fesc(&tf.to_string_lossy()), fesc(&t.font), t.size.max(8.0), ff_color(&t.color, "0xffffff"), t.size * 0.18
    );
    if t.weight >= 600 { f.push_str(":expansion=normal"); }
    if !t.bg.is_empty() { f.push_str(&format!(":box=1:boxcolor={}@{:.2}:boxborderw={:.0}", ff_color(&t.bg, "0x000000"), t.bg_opacity.clamp(0.0, 1.0), t.padding.max(0.0))); }
    else if t.shadow { f.push_str(&format!(":shadowcolor=black@0.6:shadowx={:.0}:shadowy={:.0}", t.size * 0.03, t.size * 0.04)); }
    let out = graph.label("x");
    graph.push(format!("[{bg}]{f}[{out}]"));
    out
}

fn wrap_text(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, para) in s.split('\n').enumerate() {
        if i > 0 { out.push('\n'); }
        let mut line = String::new();
        for w in para.split_whitespace() {
            if !line.is_empty() && line.len() + 1 + w.len() > max_chars { out.push_str(&line); out.push('\n'); line.clear(); }
            if !line.is_empty() { line.push(' '); }
            line.push_str(w);
        }
        out.push_str(&line);
    }
    out
}

// ---------------------------------------------------------------------------
// Caption TIMING (shared with video_hyperframes)
//
// The old ASS/drawtext caption renderer is gone. This section keeps only the
// transcript→timeline word mapper, which video_hyperframes uses to time the
// transparent Hyperframes caption overlay. `timeline_words` is pub for that.

/// Map transcript words (asset source time) onto the timeline through the clips
/// that play that asset. Returns timeline-time words.
pub fn timeline_words(comp: &Composition, tr: &Transcript) -> Vec<Word> {
    let mut out = vec![];
    for c in comp.clips.iter().filter(|c| c.kind == "video" && !c.hidden && c.asset == tr.asset) {
        let src_end = c.in_ + c.dur() * c.speed;
        for w in &tr.words {
            if w.s >= c.in_ && w.s < src_end {
                let s = c.start + (w.s - c.in_) / c.speed;
                let e = (c.start + (w.e - c.in_) / c.speed).min(c.end);
                if e > s + 0.02 { out.push(Word { w: w.w.clone(), s, e }); }
            }
        }
    }
    out.sort_by(|a, b| a.s.partial_cmp(&b.s).unwrap_or(std::cmp::Ordering::Equal));
    out
}

// ---------------------------------------------------------------------------
// Whole-graph build
// ---------------------------------------------------------------------------

pub struct Plan {
    pub args: Vec<String>,     // full ffmpeg args (without -y / output)
    pub dur: f64,
}

pub struct Target { pub w: u32, pub h: u32, pub fps: f64 }

pub fn build_plan(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, comp: &Composition, target: &Target, script_name: &str) -> Result<Plan, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let manifest: Manifest = video::load_manifest(broker, agent_id, project);
    let assets: HashMap<String, Asset> = manifest.assets.iter().map(|a| (a.id.clone(), a.clone())).collect();
    let mut abs: HashMap<String, PathBuf> = HashMap::new();
    for a in manifest.assets.iter() {
        if let Ok(p) = video::asset_abs(broker, agent_id, project, a) { abs.insert(a.id.clone(), p); }
    }
    let dur = comp.duration();
    if dur <= 0.0 { return Err("timeline is empty".into()); }
    let (w, h) = (target.w & !1, target.h & !1);
    let fps = if target.fps > 0.0 { target.fps } else { comp.scene.fps };
    let mut ctx = Ctx { comp, proj: &proj, assets: &assets, abs: &abs, inputs: vec![], input_idx: HashMap::new(), w, h, fps, dur };
    let mut g = Graph::new();

    // scene → target scale factor (so a 1920-authored transform lands right at 1080 export)
    let sx = w as f64 / comp.scene.width as f64;
    let sy = h as f64 / comp.scene.height as f64;
    let scaled: Vec<Clip> = comp.clips.iter().map(|c| {
        let mut c = c.clone();
        c.transform.x *= sx; c.transform.y *= sy;
        c.text.size *= sy;
        c
    }).collect();

    // base
    let mut cur = g.label("base");
    g.push(format!("color=c={}:s={w}x{h}:r={fps:.4}:d={dur:.4},format=rgba[{cur}]", ff_color(&comp.scene.background, "0x000000")));

    let mut visuals: Vec<&Clip> = scaled.iter().filter(|c| c.is_visual()).collect();
    visuals.sort_by_key(|c| (rank(&c.track), (c.start * 1000.0) as i64));

    let matte_on = comp.matte.enabled && !comp.matte.alpha_asset.is_empty() && abs.contains_key(&comp.matte.alpha_asset)
        && visuals.iter().any(|c| c.behind_subject);

    let mut missing: Vec<String> = vec![];
    let mut tn = 0usize;
    let captions_on = comp.captions.enabled;
    let mut compose = |ctx: &mut Ctx, g: &mut Graph, cur: &mut String, c: &Clip, missing: &mut Vec<String>| {
        if c.kind == "text" {
            tn += 1;
            *cur = text_overlay(ctx, g, cur.clone(), c, tn);
            return;
        }
        // T1 holds Hyperframes caption overlays: silent while captions are off.
        // (Legacy T1 text clips have no asset and always play — see above.)
        if c.track == "T1" && !c.asset.is_empty() && !captions_on { return; }
        if !ctx.abs.contains_key(&c.asset) { missing.push(format!("{} ({})", c.name, c.asset)); return; }
        if let Some(s) = visual_stream(ctx, c, g, None) { *cur = overlay(ctx, g, cur.clone(), s, c); }
    };

    if matte_on {
        // 1) a-roll + non-behind V1 stuff
        for c in visuals.iter().filter(|c| !c.behind_subject && rank(&c.track) <= 1) { compose(&mut ctx, &mut g, &mut cur, c, &mut missing); }
        // 2) behind-subject layers
        for c in visuals.iter().filter(|c| c.behind_subject) { compose(&mut ctx, &mut g, &mut cur, c, &mut missing); }
        // 3) foreground = a-roll clips of the matte source with alpha merged
        let src = if comp.matte.source_asset.is_empty() { scaled.iter().find(|c| c.track == "V1" && c.kind == "video").map(|c| c.asset.clone()).unwrap_or_default() } else { comp.matte.source_asset.clone() };
        for c in scaled.iter().filter(|c| c.kind == "video" && !c.hidden && c.asset == src) {
            let Some(rgb) = visual_stream(&mut ctx, c, &mut g, None) else { continue };
            let Some(a) = visual_stream(&mut ctx, c, &mut g, Some(&comp.matte.alpha_asset)) else { continue };
            let ag = g.label("ag"); let fg = g.label("fg");
            let feather = comp.matte.feather.max(0.0);
            let blur = if feather > 0.0 { format!(",gblur=sigma={feather:.2}") } else { String::new() };
            g.push(format!("[{a}]format=gray{blur}[{ag}]"));
            let rgbf = g.label("rf");
            g.push(format!("[{rgb}]format=rgb24[{rgbf}]"));
            g.push(format!("[{rgbf}][{ag}]alphamerge[{fg}]"));
            cur = overlay(&ctx, &mut g, cur, fg, c);
        }
        // 4) front layers
        for c in visuals.iter().filter(|c| !c.behind_subject && rank(&c.track) > 1) { compose(&mut ctx, &mut g, &mut cur, c, &mut missing); }
    } else {
        for c in visuals.iter() { compose(&mut ctx, &mut g, &mut cur, c, &mut missing); }
    }
    if !missing.is_empty() { return Err(format!("clips reference missing media: {}", missing.join(", "))); }

    // adjustment layer (global grade)
    if comp.adjustment_layer { cur = color_chain(&comp.color, &proj, &mut g, cur); }
    let vout = g.label("vout");
    g.push(format!("[{cur}]format=yuv420p[{vout}]"));

    // ---- audio ----
    // Linked V+A pairs share one voice: the picture clip's audio already plays,
    // so a linked waveform partner with a video mate is skipped (no doubling).
    let video_links: std::collections::HashSet<&str> = scaled.iter().filter(|c| c.kind == "video" && !c.link.is_empty()).map(|c| c.link.as_str()).collect();
    let mut voice: Vec<String> = vec![];
    let mut music: Vec<String> = vec![];
    // Live cleanup blend: clips carrying a cleaned replacement emit TWO branches —
    // original + cleaned — weighted by an equal-power crossfade of cleanMix, so the
    // export mix matches the preview slider sample-accurately.
    let cm = comp.audio.clean_mix.clamp(0.0, 1.0);
    let (w_orig, w_clean) = ((0.5 * std::f64::consts::FRAC_PI_2 * (1.0 - cm)).cos(), (0.5 * std::f64::consts::FRAC_PI_2 * cm).sin());
    let w_orig_db = 20.0 * w_orig.max(1e-4).log10();
    let w_clean_db = 20.0 * w_clean.max(1e-4).log10();
    for c in scaled.iter().filter(|c| c.has_audio_role() && !c.hidden || (c.kind == "audio" && !c.muted)) {
        if c.kind == "audio" && !c.link.is_empty() && video_links.contains(c.link.as_str()) { continue; }
        let has_rep = !c.audio_asset.is_empty() && abs.contains_key(&c.audio_asset) && assets.get(&c.audio_asset).map(|a| a.has_audio).unwrap_or(false);
        let solo_clean = has_rep && cm >= 0.999;
        let solo_orig = !has_rep || cm <= 0.001;
        // (asset_id, is_cleaned_branch)
        let mut branches: Vec<(&str, bool)> = vec![(c.asset.as_str(), false)];
        if has_rep && !solo_clean && !solo_orig { branches.push((c.audio_asset.as_str(), true)); }
        for (asset_id, is_clean) in branches {
        let use_id = if solo_clean { c.audio_asset.as_str() } else { asset_id };
        let Some(a) = assets.get(use_id) else { continue };
        if !a.has_audio { continue; }
        let Some(i) = ctx.input_for(use_id, None) else { continue };
        let l = g.label("a");
        let sp = c.speed;
        let out = c.in_ + c.dur() * sp;
        let mut f = format!("[{i}:a]atrim=start={:.4}:end={:.4},asetpts=PTS-STARTPTS", c.in_, out);
        if (sp - 1.0).abs() > 0.001 { let mut r = sp; while r > 2.0 { f.push_str(",atempo=2.0"); r /= 2.0; } while r < 0.5 { f.push_str(",atempo=0.5"); r *= 2.0; } f.push_str(&format!(",atempo={r:.4}")); }
        f.push_str(",aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo");
        // blend weight for this branch (solo endpoints skip the gain)
        if !solo_clean && !solo_orig {
            let wdb = if is_clean { w_clean_db } else { w_orig_db };
            if wdb < -0.05 { f.push_str(&format!(",volume={:.2}dB", wdb)); }
        }
        if c.volume != 0.0 { f.push_str(&format!(",volume={:.2}dB", c.volume)); }
        let tg = comp.audio.track_gain.get(&c.track).copied().unwrap_or(0.0);
        if tg != 0.0 { f.push_str(&format!(",volume={:.2}dB", tg)); }
        if !c.audio.keyframes.is_empty() {
            // piecewise-linear dB envelope. Keyframes store TIMELINE seconds;
            // the stream clock here is clip-local (asetpts ran above), so shift.
            let mut kf: Vec<Keyframe> = c.audio.keyframes.iter().map(|k| Keyframe { t: k.t - c.start, db: k.db }).collect();
            kf.sort_by(|x, y| x.t.partial_cmp(&y.t).unwrap_or(std::cmp::Ordering::Equal));
            let mut expr = format!("{:.2}", kf[0].db);
            for k in 1..kf.len() {
                let (a0, a1) = (&kf[k - 1], &kf[k]);
                let seg = if (a1.t - a0.t).abs() < 1e-6 { format!("{:.2}", a1.db) } else { format!("({:.2}+({:.2}-{:.2})*(t-{:.4})/({:.4}-{:.4}))", a0.db, a1.db, a0.db, a0.t, a1.t, a0.t) };
                expr = format!("if(lt(t,{:.4}),{expr},{seg})", a1.t);
            }
            f.push_str(&format!(",volume=volume='pow(10,({expr})/20)':eval=frame"));
        }
        if c.audio.fade_in > 0.0 { f.push_str(&format!(",afade=t=in:st=0:d={:.3}", c.audio.fade_in)); }
        if c.audio.fade_out > 0.0 { f.push_str(&format!(",afade=t=out:st={:.3}:d={:.3}", (c.dur() - c.audio.fade_out).max(0.0), c.audio.fade_out)); }
        let ms = (c.start * 1000.0).round().max(0.0) as i64;
        f.push_str(&format!(",adelay=delays={ms}:all=1"));
        g.push(format!("{f}[{l}]"));
        if c.track == "A2" { music.push(l) } else { voice.push(l) }
        } // branches
    }
    let mix = |g: &mut Graph, ins: &[String], name: &str| -> String {
        if ins.len() == 1 { return ins[0].clone(); }
        let out = g.label(name);
        g.push(format!("{}amix=inputs={}:duration=longest:normalize=0[{out}]", ins.iter().map(|s| format!("[{s}]")).collect::<String>(), ins.len()));
        out
    };
    let aout = g.label("aout");
    let master = if comp.audio.master_db != 0.0 { format!(",volume={:.2}dB", comp.audio.master_db) } else { String::new() };
    let norm = if comp.audio.loudnorm { ",loudnorm=I=-16:TP=-1.5:LRA=11".to_string() } else { String::new() };
    let tail = format!("apad=whole_dur={dur:.4},atrim=0:{dur:.4}{master}{norm}");
    // No automatic ducking: every audible clip (voice AND music) mixes flat.
    // Manual ducking = volume keyframes on a clip (UI keyframe lane or agent
    // update_clip {audio:{keyframes:[{t,db}]}}); per-track trim = track_gain.
    let mut all = voice;
    all.extend(music);
    let cache = proj.join(".cache");
    std::fs::create_dir_all(&cache).map_err(|e| format!("mkdir .cache: {e}"))?;
    let script = cache.join(script_name);
    // No peak-normalize pass: the cleaned master is already leveled to -16 LUFS
    // at cleanup time, and loudnorm on export stays the master switch.
    match all.len() {
        0 => g.push(format!("anullsrc=r=48000:cl=stereo,{tail}[{aout}]")),
        _ => {
            let m = mix(&mut g, &all, "mx2");
            let tail2 = format!("apad=whole_dur={dur:.4},atrim=0:{dur:.4}{master}{norm}");
            g.push(format!("[{m}]{tail2}[{aout}]"));
        }
    }

    std::fs::write(&script, g.lines.join(";\n")).map_err(|e| format!("write filter script: {e}"))?;

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into()];
    args.extend(ctx.inputs.clone());
    // ffmpeg ≥ 7.1 reads option values from a file with the `-/opt file` form and
    // removed -filter_complex_script (gone in 8/9). Detect once and pick.
    args.extend(filter_file_args(app, &script));
    args.extend(["-map".into(), format!("[{vout}]"), "-map".into(), format!("[{aout}]")]);
    let _ = app;
    let _ = script;
    Ok(Plan { args, dur })
}

/// `-/filter_complex <file>` on modern ffmpeg, `-filter_complex_script <file>` on ≤ 7.0.
fn filter_file_args(app: &tauri::AppHandle, script: &Path) -> Vec<String> {
    static MODERN: OnceLock<bool> = OnceLock::new();
    let modern = *MODERN.get_or_init(|| {
        let Ok(ff) = video::ffmpeg(app) else { return true };
        let out = Command::new(ff).arg("-version").output().ok();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        let major: u32 = text.split_whitespace().nth(2).and_then(|v| v.split(['.', '-']).next()).and_then(|m| m.trim_start_matches('n').parse().ok()).unwrap_or(9);
        major >= 8 || (major == 7 && !text.contains("7.0"))
    });
    if modern { vec!["-/filter_complex".into(), script.to_string_lossy().to_string()] } else { vec!["-filter_complex_script".into(), script.to_string_lossy().to_string()] }
}

// ---------------------------------------------------------------------------
// Running ffmpeg: render (with progress + cancel) and single frame
// ---------------------------------------------------------------------------

static RUNNING: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
fn running() -> &'static Mutex<HashMap<String, u32>> { RUNNING.get_or_init(|| Mutex::new(HashMap::new())) }

pub fn cancel(key: &str) -> bool {
    let pid = running().lock().ok().and_then(|m| m.get(key).copied());
    match pid { Some(p) => { unsafe { libc::kill(p as i32, libc::SIGTERM); } true } None => false }
}

fn encoder_args(codec: &str, bitrate: &str, fps: f64, fallback: bool) -> Vec<String> {
    let br = if bitrate.trim().is_empty() { "12M" } else { bitrate.trim() };
    let mut v: Vec<String> = vec![];
    match (codec, fallback) {
        ("prores", _) => v.extend(["-c:v".into(), "prores_ks".into(), "-profile:v".into(), "3".into(), "-pix_fmt".into(), "yuv422p10le".into()]),
        ("hevc", false) => v.extend(["-c:v".into(), "hevc_videotoolbox".into(), "-b:v".into(), br.into(), "-tag:v".into(), "hvc1".into(), "-pix_fmt".into(), "yuv420p".into()]),
        ("hevc", true) => v.extend(["-c:v".into(), "libx265".into(), "-crf".into(), "20".into(), "-tag:v".into(), "hvc1".into(), "-pix_fmt".into(), "yuv420p".into()]),
        (_, false) => v.extend(["-c:v".into(), "h264_videotoolbox".into(), "-b:v".into(), br.into(), "-maxrate".into(), br.into(), "-profile:v".into(), "high".into(), "-pix_fmt".into(), "yuv420p".into()]),
        (_, true) => v.extend(["-c:v".into(), "libx264".into(), "-preset".into(), "medium".into(), "-crf".into(), "18".into(), "-pix_fmt".into(), "yuv420p".into()]),
    }
    v.extend(["-r".into(), format!("{fps:.4}"), "-c:a".into(), "aac".into(), "-b:a".into(), "192k".into(), "-movflags".into(), "+faststart".into()]);
    v
}

/// Render a composition to renders/<name>.mp4 (or .mov for prores). Blocking.
pub fn render(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, comp: &Composition, preset: &ExportPreset, out_name: Option<&str>) -> Result<serde_json::Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let key = format!("{agent_id}/{project}");
    if running().lock().map(|m| m.contains_key(&key)).unwrap_or(false) { return Err("a render is already running for this project".into()); }
    // h264/hevc yuv420p need even dimensions — coerce rather than fail at encode time
    let target = Target { w: preset.width.max(16) / 2 * 2, h: preset.height.max(16) / 2 * 2, fps: preset.fps };
    let plan = build_plan(app, broker, agent_id, project, comp, &target, "render.filters")?;
    let renders = proj.join("renders");
    std::fs::create_dir_all(&renders).map_err(|e| format!("mkdir renders: {e}"))?;
    let ext = if preset.codec == "prores" { "mov" } else { "mp4" };
    let stem: String = out_name.map(|s| s.to_string()).unwrap_or_else(|| {
        let ts = chrono_like();
        format!("{project}-{}-{}x{}-{ts}", if preset.name.is_empty() { "export" } else { &preset.name }, target.w, target.h)
    }).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let out = renders.join(format!("{stem}.{ext}"));
    let fps = if preset.fps > 0.0 { preset.fps } else { comp.scene.fps };

    let mut last_err = String::new();
    for fallback in [false, true] {
        if preset.codec == "prores" && fallback { break; }
        let mut args = plan.args.clone();
        args.extend(encoder_args(&preset.codec, &preset.bitrate, fps, fallback));
        args.extend(["-t".into(), format!("{:.4}", plan.dur), "-progress".into(), "pipe:1".into(), "-nostats".into(), "-y".into(), out.to_string_lossy().to_string()]);
        match run_ffmpeg_progress(app, &key, &args, plan.dur, project) {
            Ok(()) => {
                let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                let _ = app.emit("video-render-progress", serde_json::json!({ "project": project, "pct": 100, "done": true, "path": format!("renders/{stem}.{ext}") }));
                return Ok(serde_json::json!({ "ok": true, "path": format!("renders/{stem}.{ext}"), "abs": out.to_string_lossy(), "bytes": size, "duration": plan.dur, "width": target.w, "height": target.h, "encoder": if fallback { "software" } else { "videotoolbox" } }));
            }
            Err(e) => {
                last_err = e;
                if last_err.contains("cancelled") { break; }
            }
        }
    }
    let _ = app.emit("video-render-progress", serde_json::json!({ "project": project, "error": last_err, "done": true }));
    Err(last_err)
}

fn chrono_like() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // yymmdd-hhmm without pulling a date crate
    let days = t / 86400; let rem = t % 86400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{:02}{:02}{:02}-{:02}{:02}", y % 100, m, d, rem / 3600, (rem % 3600) / 60)
}
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468; let era = z.div_euclid(146097); let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400; let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153; let d = (doy - (153 * mp + 2) / 5 + 1) as u32; let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn run_ffmpeg_progress(app: &tauri::AppHandle, key: &str, args: &[String], dur: f64, project: &str) -> Result<(), String> {
    let ff = video::ffmpeg(app)?;
    let mut child = Command::new(&ff).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null()).spawn().map_err(|e| format!("spawn ffmpeg: {e}"))?;
    let pid = child.id();
    if let Ok(mut m) = running().lock() { m.insert(key.to_string(), pid); }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let err_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let eb = err_buf.clone();
    let et = std::thread::spawn(move || {
        if let Some(e) = stderr {
            for line in std::io::BufReader::new(e).lines().map_while(Result::ok) {
                if let Ok(mut b) = eb.lock() { if b.len() < 20_000 { b.push_str(&line); b.push('\n'); } }
            }
        }
    });
    if let Some(o) = stdout {
        let app2 = app.clone(); let project = project.to_string();
        let mut last_pct = -1i64;
        for line in std::io::BufReader::new(o).lines().map_while(Result::ok) {
            if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
                if let Ok(us) = v.trim().parse::<i64>() {
                    let pct = ((us as f64 / 1e6) / dur * 100.0).clamp(0.0, 99.0) as i64;
                    if pct != last_pct { last_pct = pct; let _ = app2.emit("video-render-progress", serde_json::json!({ "project": project, "pct": pct })); }
                }
            }
        }
    }
    let status = child.wait().map_err(|e| format!("ffmpeg wait: {e}"))?;
    let _ = et.join();
    if let Ok(mut m) = running().lock() { m.remove(key); }
    if status.success() { return Ok(()); }
    let err = err_buf.lock().map(|b| b.clone()).unwrap_or_default();
    if status.code().is_none() { return Err("render cancelled".into()); }
    let tail: String = err.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    Err(format!("ffmpeg exited {}: {tail}", status.code().unwrap_or(-1)))
}

/// Render ONE frame of the full composite (grade + captions) at time t → .cache/frame.jpg
pub fn frame(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, comp: &Composition, t: f64, max_w: u32) -> Result<String, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let scale = if comp.scene.width > max_w { max_w as f64 / comp.scene.width as f64 } else { 1.0 };
    let target = Target { w: ((comp.scene.width as f64 * scale) as u32).max(16) / 2 * 2, h: ((comp.scene.height as f64 * scale) as u32).max(16) / 2 * 2, fps: comp.scene.fps };
    let plan = build_plan(app, broker, agent_id, project, comp, &target, "frame.filters")?;
    let name = format!("frame-{}.jpg", (t * 1000.0) as i64);
    let out = proj.join(".cache").join(&name);
    let mut args = plan.args.clone();
    // drop the audio map (last two entries) for a still
    args.truncate(args.len() - 2);
    let t = t.clamp(0.0, (plan.dur - 0.001).max(0.0));
    args.extend(["-ss".into(), format!("{t:.4}"), "-frames:v".into(), "1".into(), "-q:v".into(), "3".into(), "-y".into(), out.to_string_lossy().to_string()]);
    let ff = video::ffmpeg(app)?;
    let o = Command::new(&ff).arg("-hide_banner").args(&args).output().map_err(|e| format!("ffmpeg: {e}"))?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(&o.stderr);
        let tail: String = e.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(format!("frame render failed: {tail}"));
    }
    Ok(format!(".cache/{name}"))
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

fn comp_from_project(broker: &Broker, agent_id: &str, project: &str) -> Result<Composition, String> {
    let raw = video::read_json(broker, agent_id, &video::rel(project, "composition.json")).ok_or_else(|| format!("no such project: {project}"))?;
    let assets = video::load_manifest(broker, agent_id, project).assets;
    parse_composition(&raw, &assets)
}

#[tauri::command]
pub async fn video_render(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, preset: serde_json::Value, out_name: Option<String>) -> Result<serde_json::Value, String> {
    let broker = broker.inner().clone();
    tokio::task::spawn_blocking(move || {
        let comp = comp_from_project(&broker, &agent_id, &project)?;
        let p: ExportPreset = serde_json::from_value(preset).map_err(|e| format!("preset: {e}"))?;
        render(&app, &broker, &agent_id, &project, &comp, &p, out_name.as_deref())
    }).await.map_err(|e| format!("render task: {e}"))?
}

#[tauri::command]
pub fn video_render_cancel(agent_id: String, project: String) -> bool { cancel(&format!("{agent_id}/{project}")) }

#[tauri::command]
pub async fn video_frame(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, time: f64, composition: Option<serde_json::Value>) -> Result<String, String> {
    let broker = broker.inner().clone();
    tokio::task::spawn_blocking(move || {
        let comp = match composition {
            Some(c) => { let assets = video::load_manifest(&broker, &agent_id, &project).assets; parse_composition(&c, &assets)? }
            None => comp_from_project(&broker, &agent_id, &project)?,
        };
        frame(&app, &broker, &agent_id, &project, &comp, time, 1280)
    }).await.map_err(|e| format!("frame task: {e}"))?
}

/// Validate + normalize a composition (UI calls this to show schema errors early).
#[tauri::command]
pub fn video_validate(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, composition: serde_json::Value) -> Result<serde_json::Value, String> {
    let assets = video::load_manifest(&broker, &agent_id, &project).assets;
    let comp = parse_composition(&composition, &assets)?;
    let ids: std::collections::HashSet<&str> = assets.iter().map(|a| a.id.as_str()).collect();
    let missing: Vec<String> = comp.clips.iter().filter(|c| c.kind != "text" && !c.asset.is_empty() && !ids.contains(c.asset.as_str())).map(|c| c.id.clone()).collect();
    Ok(serde_json::json!({ "duration": comp.duration(), "clips": comp.clips.len(), "missingAssets": missing, "composition": comp }))
}

#[tauri::command]
pub fn video_list_renders(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String) -> Result<Vec<serde_json::Value>, String> {
    let proj = video::project_dir(&broker, &agent_id, &project)?;
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(proj.join("renders")) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() { continue; }
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with('.') { continue; }
            let md = std::fs::metadata(&p).ok();
            out.push(serde_json::json!({ "path": format!("renders/{n}"), "bytes": md.as_ref().map(|m| m.len()).unwrap_or(0), "modified": md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0) }));
        }
    }
    out.sort_by_key(|v| -(v["modified"].as_i64().unwrap_or(0)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upgrades_v02_sequence() {
        let assets = vec![Asset { id: "a1".into(), name: "clip.mp4".into(), kind: "video".into(), duration: 20.0, has_video: true, has_audio: true, ..Default::default() }];
        let raw = serde_json::json!({ "scene": { "width": 1920, "height": 1080, "fps": 30 }, "sequence": [ { "id": "x", "track": "V1", "src": "clip.mp4", "start": 0, "end": 5, "sourceIn": 2, "sourceOut": 7 }, { "id": "t", "track": "T1", "src": "", "start": 1, "end": 3, "text": "Hello" } ] });
        let c = parse_composition(&raw, &assets).unwrap();
        assert_eq!(c.clips.len(), 2);
        assert_eq!(c.clips[0].asset, "a1"); assert_eq!(c.clips[0].in_, 2.0); assert_eq!(c.clips[0].out, 7.0);
        assert_eq!(c.clips[1].kind, "text"); assert_eq!(c.clips[1].text.content, "Hello");
        assert!((c.duration() - 5.0).abs() < 1e-9);
    }
    #[test]
    fn words_map_through_cuts() {
        let mut comp = Composition::default();
        comp.clips.push(Clip { id: "c1".into(), track: "V1".into(), kind: "video".into(), asset: "a".into(), start: 0.0, end: 2.0, in_: 10.0, out: 12.0, ..Default::default() });
        let tr = Transcript { asset: "a".into(), words: vec![Word { w: "hello".into(), s: 10.2, e: 10.5 }, Word { w: "world".into(), s: 10.6, e: 10.9 }, Word { w: "skipped".into(), s: 13.0, e: 13.4 }], ..Default::default() };
        let words = timeline_words(&comp, &tr);
        assert_eq!(words.len(), 2);
        assert!((words[0].s - 0.2).abs() < 1e-9);
    }
    #[test]
    fn fesc_escapes_filter_chars() { assert_eq!(fesc("a:b,c'd"), "a\\:b\\,c\\'d"); }
    #[test]
    fn wrap_text_breaks_lines() { assert_eq!(wrap_text("one two three four", 9), "one two\nthree\nfour"); }
}
