// AYGENT — VIDEO v0.3 agent tools ("code helpers").
//
// Precision edits happen here, not by the model hand-editing JSON: the agent
// calls video_* tools with frame-accurate numbers, and each tool validates +
// saves composition.json through the jail. The UI calls the same core via thin
// Tauri commands, so a button press and an agent prompt do literally the same
// thing. Nothing here shells out to anything the model names — only the
// provisioned ffmpeg/ffprobe/uv binaries with fixed argument shapes.
//
// Tools:
//   video_project        overview of a project (or list projects)
//   video_edit           batched ops on the composition (set/add/update/remove/split/ripple/cutlist)
//   video_transcribe     word-level transcript → transcript.json (Whisper verbose_json, chunked)
//   video_silences       silent ranges of an asset (ffmpeg silencedetect)
//   video_takes          repeated-take groups from the transcript (keep last/longest)
//   video_auto_cut       silences + takes → tight V1 selects in one call
//   video_frame          render one composite frame (grade + overlays) for QA
//   video_render         export with a preset (progress streams to the UI)
//   video_audio_enhance  local normalize + denoise → enhanced audio asset
//   video_matte          RobustVideoMatting alpha (uv + torch) → matte asset (experimental)
//   video_build_captions Hyperframes caption overlay → transparent T1 clip
//   video_render_overlay one approved Hyperframes graphic → transparent V3 clip

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};

use crate::broker::Broker;
use crate::video::{self, Asset};
use crate::video_render::{self as vr, Clip, Composition, Segment, Transcript, Word};

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
pub fn install_app(app: tauri::AppHandle) { let _ = APP.set(app); }
fn app() -> Result<tauri::AppHandle, String> { APP.get().cloned().ok_or_else(|| "video tools not initialized".to_string()) }
/// AppHandle for the Hyperframes overlay module (same install_app source).
pub fn app_handle() -> Option<tauri::AppHandle> { APP.get().cloned() }

const NAMES: &[&str] = &["video_project", "video_edit", "video_transcribe", "video_silences", "video_takes", "video_auto_cut", "video_frame", "video_look", "video_render", "video_audio_enhance", "video_audio_audition", "video_matte", "video_build_captions", "video_render_overlay"];
pub fn is_video_tool(name: &str) -> bool { NAMES.contains(&name) || crate::video_hyperframes::is_hyperframes_tool(name) }

pub fn tool_schemas() -> Vec<Value> {
    let proj = json!({ "type": "string", "description": "project name (folder under Video/)" });
    let mut v = vec![
        json!({ "name": "video_project", "description": "Overview of a video project: scene, duration, clips per track, assets (ids, durations, fps, audio), transcript/captions/graphics-style/LUT state, renders. Omit project to list all projects. Call this FIRST before editing.",
            "input_schema": { "type": "object", "properties": { "project": proj } } }),
        json!({ "name": "video_edit", "description": "Apply precise edits to Video/<project>/composition.json and save. ops run in order. Ops: {op:'set', path:'captions.enabled', value:true} (dot path into the composition, e.g. scene.width, color.lut, color.sCurve, audio.duck.enabled, audio.cleanEnabled, captions.keyWords, graphics.instructions) · {op:'add_clip', clip:{track,type:'video'|'audio'|'image'|'text'|'review',asset,start,end,in,out,name,text:{content,size,y,...},transform:{x,y,scale,opacity},fit,behindSubject,volume}} (video with audio on V1/V2 auto-lays a linked A1 waveform partner) · {op:'update_clip', id, patch:{...}} · {op:'remove_clip', id, ripple?:true} (linked partners go together) · {op:'split', id, at:<timeline seconds>} (linked partners split together) · {op:'move', id, start} · {op:'cutlist', asset, keep:[{in,out}], track?:'V1', pad?:0.03} (replaces that asset's clips on the track with contiguous selects + linked A1 partners). Times are seconds; start/end = timeline placement, in/out = source range. Returns the saved summary.",
            "input_schema": { "type": "object", "properties": { "project": proj, "ops": { "type": "array", "items": { "type": "object" } } }, "required": ["project", "ops"] } }),
        json!({ "name": "video_transcribe", "description": "Word-level transcript of an asset (OpenAI Whisper; audio is extracted + chunked automatically, any length). Saves Video/<project>/transcript.json used by captions, takes and auto_cut. Returns the segments with timestamps.",
            "input_schema": { "type": "object", "properties": { "project": proj, "asset": { "type": "string", "description": "asset id (default: the first V1 video clip's asset, else the first video asset)" }, "language": { "type": "string", "description": "ISO code hint, e.g. en" } }, "required": ["project"] } }),
        json!({ "name": "video_silences", "description": "Detect silent ranges in an asset's audio (source time, seconds).",
            "input_schema": { "type": "object", "properties": { "project": proj, "asset": { "type": "string" }, "threshold_db": { "type": "number", "description": "default -35" }, "min_duration": { "type": "number", "description": "seconds, default 0.5" } }, "required": ["project", "asset"] } }),
        json!({ "name": "video_takes", "description": "Find repeated takes in the transcript (the speaker re-saying a line). Returns groups of similar segments with the recommended keep (last take by default, or the longest coherent one). Requires video_transcribe first.",
            "input_schema": { "type": "object", "properties": { "project": proj, "asset": { "type": "string" }, "keep": { "type": "string", "enum": ["last", "longest"] }, "similarity": { "type": "number", "description": "0..1, default 0.6" } }, "required": ["project"] } }),
        json!({ "name": "video_auto_cut", "description": "One shot rough cut: drop silences/dead air and duplicate takes from an A-roll asset, then lay the kept ranges as tight contiguous V1 selects (+ linked A1 waveform partners). Transcribes first if needed. Returns the cutlist + what was dropped so the user can review.",
            "input_schema": { "type": "object", "properties": { "project": proj, "asset": { "type": "string" }, "threshold_db": { "type": "number", "description": "default -35" }, "min_silence": { "type": "number", "description": "seconds, default 0.6" }, "pad": { "type": "number", "description": "seconds kept around speech, default 0.08" }, "keep": { "type": "string", "enum": ["last", "longest"] }, "drop_takes": { "type": "boolean", "description": "default true" } }, "required": ["project"] } }),
        json!({ "name": "video_frame", "description": "Render ONE composite frame (grade + overlays) at a timeline time to .cache/. Use to sanity-check a moment; the user sees it in the Video tab preview.",
            "input_schema": { "type": "object", "properties": { "project": proj, "time": { "type": "number" } }, "required": ["project", "time"] } }),
        json!({ "name": "video_look", "description": "LOOK at the canvas: render the full composite (grade + LUT + overlays) at timeline time(s) and SEE the frame(s) as images in your next message. Your eyes in the editor — any timecode, any moment. The first frame also appears in the user's Video tab preview. Delete frames you no longer need with delete_file (Video/<project>/.cache/frame-*.jpg).",
            "input_schema": { "type": "object", "properties": { "project": proj, "time": { "type": "number", "description": "timeline seconds to look at" }, "times": { "type": "array", "items": { "type": "number" }, "description": "up to 4 times to see side by side" } }, "required": ["project"] } }),
        json!({ "name": "video_render", "description": "Export the project with a preset from composition.exports (by name) or an explicit {width,height,bitrate,codec:'h264'|'hevc'|'prores'}. Blocking; progress streams to the UI. Output lands in Video/<project>/renders/.",
            "input_schema": { "type": "object", "properties": { "project": proj, "preset": { "type": "string", "description": "preset name, e.g. landscape | vertical" }, "width": { "type": "integer" }, "height": { "type": "integer" }, "bitrate": { "type": "string" }, "codec": { "type": "string" }, "name": { "type": "string", "description": "output file stem" } }, "required": ["project"] } }),
        json!({ "name": "video_audio_enhance", "description": "LOCAL dialogue cleanup (ffmpeg only, nothing leaves the machine): normalize peaks to normalizeDb (default -3 dBFS) + background-noise reduction at denoise 0..1 (0 = off). Video sources produce a full-length .cleaned.mov proxy (picture stream-copied, cleaned audio padded to the video duration) so preview plays picture+sound as one element through cuts; audio-only sources produce a .cleaned.wav. Points the source clips' audioAsset at it, and sets audio.cleanEnabled=true (toggle it off to A/B the original). (TEMPORARY compat: engine/preset args are ignored if passed.)",
            "input_schema": { "type": "object", "properties": { "project": proj, "asset": { "type": "string" }, "normalize_db": { "type": "number", "description": "peak target dBFS, default -3" }, "denoise": { "type": "number", "description": "0..1 noise-reduction strength, default 0" } }, "required": ["project", "asset"] } }),
        json!({ "name": "video_audio_audition", "description": "Preview a denoise strength WITHOUT touching the timeline: renders a short sample (at/len seconds) with that denoise amount to .cache/ for listening in the Video tab.",
            "input_schema": { "type": "object", "properties": { "project": proj, "asset": { "type": "string" }, "denoise": { "type": "number", "description": "0..1 strength" }, "at": { "type": "number", "description": "sample start (source seconds, default 0)" }, "len": { "type": "number", "description": "sample length seconds, default 8" } }, "required": ["project", "asset", "denoise"] } }),
        json!({ "name": "video_matte", "description": "EXPERIMENTAL: generate a subject alpha matte for an asset with RobustVideoMatting (downloads torch via uv on first run; slow). Registers the matte asset and enables composition.matte so behindSubject overlay layers render behind the person.",
            "input_schema": { "type": "object", "properties": { "project": proj, "asset": { "type": "string" }, "engine": { "type": "string", "description": "ignored (local only)" } }, "required": ["project", "asset"] } }),
    ];
    v.extend(crate::video_hyperframes::tool_schemas());
    v
}

pub const INSTRUCTIONS: &str = "\n\nVIDEO EDITOR: the Video tab is an agentic NLE. A project is Video/<name>/ with composition.json (the edit), assets.json (imported media — HARDLINKS, never copy media), transcript.json, chat.json. Use the video_* tools for every edit (frame-accurate numbers, validated + saved); never hand-write composition.json unless a tool cannot express the change. Workflow the user follows: video_project → video_auto_cut (silences + keep the LAST take; lays linked V1+A1 pairs) → review → video_transcribe (if not done) → graphics/captions as Hyperframes overlays (see below) → audio via video_audio_enhance {normalizeDb, denoise} (local, peaks default -3 dBFS) + per-track audio.trackGain + audio.cleanEnabled bypass + manual ducking with clip volume keyframes {audio:{keyframes:[{t,db}]}} (video_audio_audition previews a denoise strength first); color via set color.lut 'luts/<file>.cube' + color.sCurve → video_audio_enhance → video_render {preset:'landscape'|'vertical'}. Report clip ids + times in one line; the UI refreshes automatically after each tool. Video clips with audio carry a LINKED A1 waveform partner (clip.link shared) — split/move/remove keep pairs together, and the mix plays the pair once (no double audio). REVIEW: the user leaves timestamped feedback as review clips on R1 (video_project exposes reviewNotes/reviewNotesResolved) — read them first, act on each, mark done via update_clip {hidden:true}. VISION: video_look renders canvas frame(s) you SEE as images; delete spent frames with delete_file.";

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Agent-tool exec (sync — runs on the caller's thread like shell_run).
pub fn exec(broker: &Arc<Broker>, agent_id: &str, name: &str, input: &Value) -> (String, bool) {
    let Ok(app) = app() else { return ("video tools not initialized".into(), true) };
    match run(&app, broker, agent_id, name, input) {
        Ok(v) => (bounded(&v), false),
        Err(e) => (e, true),
    }
}

/// Frames a tool just rendered for the model to SEE (currently only video_look).
/// `rel` is jail-relative (Video/<project>/.cache/frame-*.jpg); the agent loop
/// reads the bytes through the jail and inlines them as provider image blocks.
/// Delete with the normal delete_file tool when no longer needed.
#[derive(Clone, Debug, Default)]
pub struct VisionFrame { pub rel: String, pub label: String }

thread_local! { static PENDING_VISION: std::cell::RefCell<Vec<VisionFrame>> = std::cell::RefCell::new(Vec::new()); }
fn stash_vision(v: Vec<VisionFrame>) { PENDING_VISION.with(|p| *p.borrow_mut() = v); }
fn take_vision() -> Vec<VisionFrame> { PENDING_VISION.with(|p| std::mem::take(&mut *p.borrow_mut())) }

#[allow(dead_code)]
pub struct ExecOut { pub text: String, pub is_err: bool, pub vision: Vec<VisionFrame> }

/// exec + any vision frames the tool produced (drains stale frames first so a
/// reused worker thread can never leak one turn's frames into the next).
/// Frames stay in the thread-local stash for take_pending_vision (called by the
/// agent loop right after it pushes the text result); the returned clone lets
/// unit tests assert without touching thread state.
/// Local-model loop keeps using exec (text only); cloud loops use exec_full.
pub fn exec_full(broker: &Arc<Broker>, agent_id: &str, name: &str, input: &Value) -> ExecOut {
    let _ = take_vision();
    let (text, is_err) = exec(broker, agent_id, name, input);
    let v = take_vision();
    stash_vision(v.clone());
    ExecOut { text, is_err, vision: v }
}

/// Drain frames stashed by the last exec_full on this thread.
pub fn take_pending_vision() -> Vec<VisionFrame> { take_vision() }

fn bounded(v: &Value) -> String {
    let s = serde_json::to_string_pretty(v).unwrap_or_default();
    if s.len() > 24_000 { format!("{}\n… [truncated {} chars]", &s[..24_000], s.len() - 24_000) } else { s }
}

/// UI entry: same core, async wrapper.
#[tauri::command]
pub async fn video_tool(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, name: String, input: Value) -> Result<Value, String> {
    let broker = broker.inner().clone();
    tokio::task::spawn_blocking(move || run(&app, &broker, &agent_id, &name, &input)).await.map_err(|e| format!("tool task: {e}"))?
}

pub fn run(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, name: &str, input: &Value) -> Result<Value, String> {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let f = |k: &str, d: f64| input.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let project = s("project");
    // Hyperframes overlay tools live in video_hyperframes (same jail, same emit).
    if crate::video_hyperframes::is_hyperframes_tool(name) {
        let out = crate::video_hyperframes::exec(broker, agent_id, name, input)?;
        return Ok(out);
    }
    let out = match name {
        "video_project" => match project { Some(p) => overview(broker, agent_id, &p)?, None => list_projects(broker, agent_id)? },
        "video_edit" => {
            let p = project.ok_or("project is required")?;
            let ops = input.get("ops").and_then(|o| o.as_array()).cloned().ok_or("ops must be an array")?;
            edit(broker, agent_id, &p, &ops)?
        }
        "video_transcribe" => {
            let p = project.ok_or("project is required")?;
            let (a, tr) = transcribe(app, broker, agent_id, &p, s("asset").as_deref(), s("language").as_deref())?;
            json!({ "asset": a.id, "name": a.name, "language": tr.language, "words": tr.words.len(), "segments": tr.segments.iter().map(|g| json!({ "s": round2(g.s), "e": round2(g.e), "text": g.text })).collect::<Vec<_>>(), "saved": "transcript.json" })
        }
        "video_silences" => {
            let p = project.ok_or("project is required")?;
            let a = find_asset(broker, agent_id, &p, s("asset").as_deref(), true)?;
            let abs = video::asset_abs(broker, agent_id, &p, &a)?;
            let sil = silences(app, &abs, f("threshold_db", -35.0), f("min_duration", 0.5))?;
            json!({ "asset": a.id, "duration": a.duration, "silences": sil.iter().map(|(s, e)| json!({ "s": round2(*s), "e": round2(*e), "dur": round2(e - s) })).collect::<Vec<_>>() })
        }
        "video_takes" => {
            let p = project.ok_or("project is required")?;
            let proj = video::project_dir(broker, agent_id, &p)?;
            let tr = load_transcript(&proj).ok_or("no transcript yet — run video_transcribe first")?;
            let groups = takes(&tr, s("keep").as_deref().unwrap_or("last"), f("similarity", 0.6));
            json!({ "asset": tr.asset, "groups": groups })
        }
        "video_auto_cut" => {
            let p = project.ok_or("project is required")?;
            auto_cut(app, broker, agent_id, &p, s("asset").as_deref(), f("threshold_db", -35.0), f("min_silence", 0.6), f("pad", 0.08), s("keep").as_deref().unwrap_or("last"), input.get("drop_takes").and_then(|v| v.as_bool()).unwrap_or(true))?
        }
        "video_frame" => {
            let p = project.ok_or("project is required")?;
            let comp = load_comp(broker, agent_id, &p)?;
            let rel = vr::frame(app, broker, agent_id, &p, &comp, f("time", 0.0), 1280)?;
            let _ = tauri::Emitter::emit(app, "video-frame-ready", json!({ "project": p, "path": rel, "time": f("time", 0.0) }));
            json!({ "frame": rel, "note": "rendered to Video/<project>/.cache — the Video tab shows it" })
        }
        "video_look" => {
            let p = project.ok_or("project is required")?;
            let mut times: Vec<f64> = vec![];
            if let Some(t) = input.get("time").and_then(|v| v.as_f64()) { times.push(t); }
            if let Some(arr) = input.get("times").and_then(|v| v.as_array()) {
                for v in arr.iter().filter_map(|v| v.as_f64()).take(4) { times.push(v); }
            }
            if times.is_empty() { times.push(0.0); }
            times.truncate(4);
            let comp = load_comp(broker, agent_id, &p)?;
            let dur = comp.duration().max(0.04);
            let mut frames: Vec<Value> = vec![];
            let mut vision: Vec<VisionFrame> = vec![];
            for (i, t) in times.iter().enumerate() {
                let t = t.clamp(0.0, (dur - 0.001).max(0.0));
                let rel = vr::frame(app, broker, agent_id, &p, &comp, t, 854)?;
                if i == 0 { let _ = tauri::Emitter::emit(app, "video-frame-ready", json!({ "project": p, "path": rel, "time": t })); }
                frames.push(json!({ "time": round3(t), "frame": rel }));
                vision.push(VisionFrame { rel: video::rel(&p, &rel), label: format!("{t:.2}s") });
            }
            stash_vision(vision);
            json!({ "frames": frames, "note": "you SEE these frame(s) as images alongside this result. Delete frames you no longer need with delete_file." })
        }
        "video_render" => {
            let p = project.ok_or("project is required")?;
            let comp = load_comp(broker, agent_id, &p)?;
            let mut preset = match s("preset") { Some(n) => comp.exports.iter().find(|e| e.name.eq_ignore_ascii_case(&n)).cloned().ok_or_else(|| format!("no export preset named {n}; have: {}", comp.exports.iter().map(|e| e.name.clone()).collect::<Vec<_>>().join(", ")))?, None => comp.exports.first().cloned().unwrap_or_default() };
            if let Some(w) = input.get("width").and_then(|v| v.as_u64()) { preset.width = w as u32; }
            if let Some(h) = input.get("height").and_then(|v| v.as_u64()) { preset.height = h as u32; }
            if let Some(b) = s("bitrate") { preset.bitrate = b; }
            if let Some(c) = s("codec") { preset.codec = c; }
            vr::render(app, broker, agent_id, &p, &comp, &preset, s("name").as_deref())?
        }
        "video_audio_enhance" => {
            let p = project.ok_or("project is required")?;
            enhance(app, broker, agent_id, &p, &s("asset").ok_or("asset is required")?, f("normalize_db", -3.0), f("denoise", 0.0))?
        }
        "video_audio_audition" => {
            let p = project.ok_or("project is required")?;
            audition(app, broker, agent_id, &p, &s("asset").ok_or("asset is required")?, f("denoise", 0.5), f("at", 0.0), f("len", 8.0))?
        }
        "video_matte" => {
            let p = project.ok_or("project is required")?;
            matte(app, broker, agent_id, &p, &s("asset").ok_or("asset is required")?)?
        }
        other => return Err(format!("unknown video tool: {other}")),
    };
    if let Some(p) = s("project") { let _ = tauri::Emitter::emit(app, "video-project-changed", json!({ "project": p, "tool": name })); }
    Ok(out)
}

fn round2(x: f64) -> f64 { (x * 100.0).round() / 100.0 }
fn round3(x: f64) -> f64 { (x * 1000.0).round() / 1000.0 }

// ---------------------------------------------------------------------------
// Composition IO
// ---------------------------------------------------------------------------

fn load_comp(broker: &Broker, agent_id: &str, project: &str) -> Result<Composition, String> {
    let raw = video::read_json(broker, agent_id, &video::rel(project, "composition.json")).ok_or_else(|| format!("no such project: {project}"))?;
    let assets = video::load_manifest(broker, agent_id, project).assets;
    vr::parse_composition(&raw, &assets)
}
fn save_comp(broker: &Broker, agent_id: &str, project: &str, comp: &Composition) -> Result<(), String> {
    video::write_json(broker, agent_id, &video::rel(project, "composition.json"), &serde_json::to_value(comp).map_err(|e| e.to_string())?).map(|_| ())
}
fn load_transcript(proj: &Path) -> Option<Transcript> {
    serde_json::from_str(&std::fs::read_to_string(proj.join("transcript.json")).ok()?).ok()
}

fn find_asset(broker: &Broker, agent_id: &str, project: &str, id: Option<&str>, need_audio: bool) -> Result<Asset, String> {
    let m = video::load_manifest(broker, agent_id, project);
    if let Some(id) = id {
        return m.assets.iter().find(|a| a.id == id || a.name == id).cloned().ok_or_else(|| format!("no asset {id}; have: {}", m.assets.iter().map(|a| format!("{} ({})", a.id, a.name)).collect::<Vec<_>>().join(", ")));
    }
    // default: the first V1 video clip's asset, else first video asset
    if let Ok(comp) = load_comp(broker, agent_id, project) {
        if let Some(c) = comp.clips.iter().find(|c| c.track == "V1" && c.kind == "video") {
            if let Some(a) = m.assets.iter().find(|a| a.id == c.asset) { return Ok(a.clone()); }
        }
    }
    m.assets.iter().find(|a| a.kind == "video" && (!need_audio || a.has_audio)).or_else(|| m.assets.iter().find(|a| a.has_audio)).cloned().ok_or_else(|| "no media imported yet — import A-roll first".to_string())
}

fn list_projects(broker: &Broker, agent_id: &str) -> Result<Value, String> {
    let dir = broker.resolve(agent_id, video::VIDEO_ROOT, crate::broker::Mode::Read).map_err(|e| format!("{e:?}"))?;
    let mut names = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) { for e in rd.flatten() { let n = e.file_name().to_string_lossy().to_string(); if e.path().join("composition.json").is_file() && video::slug_ok(&n) { names.push(n); } } }
    names.sort();
    Ok(json!({ "projects": names, "hint": "call video_project with a project name for details" }))
}

fn overview(broker: &Broker, agent_id: &str, project: &str) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let comp = load_comp(broker, agent_id, project)?;
    let assets = video::load_manifest(broker, agent_id, project).assets;
    let tr = load_transcript(&proj);
    let mut tracks: HashMap<String, Vec<Value>> = HashMap::new();
    for c in &comp.clips {
        tracks.entry(c.track.clone()).or_default().push(json!({ "id": c.id, "type": c.kind, "asset": c.asset, "name": c.name, "start": round3(c.start), "end": round3(c.end), "in": round3(c.in_), "out": round3(c.out), "hidden": c.hidden, "muted": c.muted, "link": c.link, "text": if c.kind == "text" || c.kind == "review" { Some(&c.text.content) } else { None }, "behindSubject": c.behind_subject }));
    }
    let luts: Vec<String> = std::fs::read_dir(proj.join("luts")).map(|rd| rd.flatten().map(|e| format!("luts/{}", e.file_name().to_string_lossy())).collect()).unwrap_or_default();
    let renders: Vec<String> = std::fs::read_dir(proj.join("renders")).map(|rd| rd.flatten().map(|e| format!("renders/{}", e.file_name().to_string_lossy())).collect()).unwrap_or_default();
    let overlays: Vec<String> = assets.iter().filter(|a| a.name.contains("(overlay HF)") || a.name.contains("(captions HF)")).map(|a| format!("{} ({})", a.id, a.name)).collect();
    let offline: Vec<Value> = assets.iter().filter(|a| !video::asset_abs(broker, agent_id, project, a).map(|p| p.is_file()).unwrap_or(false)).map(|a| json!({ "id": a.id, "name": a.name, "path": a.path })).collect();
    let mut notes: Vec<Value> = vec![];
    let mut notes_resolved: Vec<Value> = vec![];
    for c in comp.clips.iter().filter(|c| c.kind == "review") {
        let v = json!({ "id": c.id, "start": round3(c.start), "end": round3(c.end), "text": c.text.content });
        if c.hidden { notes_resolved.push(v); } else { notes.push(v); }
    }
    Ok(json!({
        "project": project,
        "reviewNotes": notes,
        "reviewNotesResolved": notes_resolved,
        "offlineMedia": offline,
        "warning": if offline.is_empty() { Value::Null } else { json!("Some media is OFFLINE (drive unplugged or file moved) — tell the user which files/paths and don't run transcribe/cut/render on them until relinked.") },
        "scene": comp.scene,
        "duration": round3(comp.duration()),
        "tracks": tracks,
        "assets": assets.iter().map(|a| json!({ "id": a.id, "name": a.name, "kind": a.kind, "duration": round2(a.duration), "fps": round2(a.fps), "size": format!("{}x{}", a.width, a.height), "hasAudio": a.has_audio, "linked": a.linked, "online": video::asset_abs(broker, agent_id, project, a).map(|p| p.is_file()).unwrap_or(false), "path": a.path })).collect::<Vec<_>>(),
        "transcript": tr.as_ref().map(|t| json!({ "asset": t.asset, "words": t.words.len(), "segments": t.segments.len() })),
        "captions": comp.captions,
        "graphics": comp.graphics,
        "overlays": overlays,
        "color": comp.color,
        "audio": comp.audio,
        "matte": comp.matte,
        "exports": comp.exports,
        "luts": luts,
        "renders": renders,
    }))
}

// ---------------------------------------------------------------------------
// video_edit — linked V+A pairs stay together through every op
// ---------------------------------------------------------------------------

fn set_path(v: &mut Value, path: &str, val: Value) -> Result<(), String> {
    let parts: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { return Err("empty path".into()); }
    if parts[0] == "clips" { return Err("use add_clip/update_clip/remove_clip for clips".into()); }
    let mut cur = v;
    for (i, p) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            match cur { Value::Object(m) => { m.insert((*p).to_string(), val); return Ok(()); } _ => return Err(format!("{path}: parent is not an object")) }
        }
        cur = match cur {
            Value::Object(m) => m.entry((*p).to_string()).or_insert_with(|| json!({})),
            Value::Array(a) => { let idx: usize = p.parse().map_err(|_| format!("{path}: bad index {p}"))?; a.get_mut(idx).ok_or_else(|| format!("{path}: index out of range"))? }
            _ => return Err(format!("{path}: cannot descend into scalar")),
        };
    }
    Ok(())
}

fn merge(dst: &mut Value, patch: &Value) {
    match (dst, patch) {
        (Value::Object(d), Value::Object(p)) => { for (k, v) in p { if v.is_object() && d.get(k).map(|x| x.is_object()).unwrap_or(false) { merge(d.get_mut(k).unwrap(), v); } else { d.insert(k.clone(), v.clone()); } } }
        (d, p) => *d = p.clone(),
    }
}

/// Ids of clips sharing a link with any of `ids` (V+A pairs act as one).
fn linked_ids(arr: &[Value], ids: &HashSet<String>) -> HashSet<String> {
    let mut links: HashSet<String> = HashSet::new();
    for c in arr {
        if c.get("id").and_then(|x| x.as_str()).map(|x| ids.contains(x)).unwrap_or(false) {
            if let Some(l) = c.get("link").and_then(|x| x.as_str()).filter(|x| !x.is_empty()) { links.insert(l.to_string()); }
        }
    }
    if links.is_empty() { return ids.clone(); }
    let mut out = ids.clone();
    for c in arr {
        let same = c.get("link").and_then(|x| x.as_str()).map(|x| links.contains(x)).unwrap_or(false);
        if same { if let Some(id) = c.get("id").and_then(|x| x.as_str()) { out.insert(id.to_string()); } }
    }
    out
}

pub fn edit(broker: &Broker, agent_id: &str, project: &str, ops: &[Value]) -> Result<Value, String> {
    let assets = video::load_manifest(broker, agent_id, project).assets;
    let comp = load_comp(broker, agent_id, project)?;
    let mut v = serde_json::to_value(&comp).map_err(|e| e.to_string())?;
    let mut log: Vec<String> = vec![];
    for (i, op) in ops.iter().enumerate() {
        let kind = op.get("op").and_then(|o| o.as_str()).ok_or_else(|| format!("op #{i} missing 'op'"))?;
        let id = op.get("id").and_then(|o| o.as_str()).unwrap_or("").to_string();
        match kind {
            "set" => {
                let path = op.get("path").and_then(|p| p.as_str()).ok_or("set needs path")?;
                set_path(&mut v, path, op.get("value").cloned().unwrap_or(Value::Null))?;
                log.push(format!("set {path}"));
            }
            "add_clip" => {
                let mut c = op.get("clip").cloned().ok_or("add_clip needs clip")?;
                if !c.is_object() { return Err("clip must be an object".into()); }
                let cid = c.get("id").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| video::new_id("c"));
                c["id"] = json!(cid);
                // sensible defaults from the asset
                let kind = c.get("type").and_then(|x| x.as_str()).unwrap_or("video").to_string();
                if kind == "review" {
                    // Feedback notes: no media, pinned to the R1 review lane.
                    // hidden = resolved (render skips them; UI greys them out).
                    if c.get("track").is_none() { c["track"] = json!("R1"); }
                    let start = c.get("start").and_then(|x| x.as_f64()).unwrap_or(0.0).max(0.0);
                    c["start"] = json!(start);
                    if c.get("end").is_none() { c["end"] = json!(start + 2.0); }
                    c["in"] = json!(0.0);
                    c["out"] = json!(c.get("end").and_then(|x| x.as_f64()).unwrap_or(start + 2.0) - start);
                    if c.get("name").is_none() { c["name"] = json!("Note"); }
                    v["clips"].as_array_mut().ok_or("clips")?.push(c);
                    log.push(format!("add_clip {cid} (review note)"));
                } else if kind != "text" {
                    let aid = c.get("asset").and_then(|x| x.as_str()).unwrap_or("");
                    let a = assets.iter().find(|a| a.id == aid || a.name == aid).ok_or_else(|| format!("add_clip: unknown asset {aid}"))?;
                    c["asset"] = json!(a.id);
                    if c.get("name").is_none() { c["name"] = json!(a.name); }
                    let in_ = c.get("in").and_then(|x| x.as_f64()).unwrap_or(0.0);
                    let out = c.get("out").and_then(|x| x.as_f64()).unwrap_or(if a.kind == "image" { in_ + 5.0 } else { a.duration });
                    let start = c.get("start").and_then(|x| x.as_f64()).unwrap_or_else(|| serde_json::from_value::<Composition>(v.clone()).map(|k| k.duration()).unwrap_or(0.0));
                    c["in"] = json!(in_); c["out"] = json!(out); c["start"] = json!(start);
                    if c.get("end").is_none() { c["end"] = json!(start + (out - in_).max(0.04)); }
                    if c.get("track").is_none() { c["track"] = json!(if a.kind == "audio" { "A1" } else { "V1" }); }
                    let track = c.get("track").and_then(|x| x.as_str()).unwrap_or("V1").to_string();
                    let arr = v["clips"].as_array_mut().ok_or("clips")?;
                    arr.push(c);
                    // Video with audio on a picture track lays a linked A1
                    // waveform partner (same timing, shared link id).
                    if a.has_audio && (track == "V1" || track == "V2") {
                        let link = video::new_id("l");
                        if let Some(last) = arr.last_mut() { last["link"] = json!(link); }
                        let vc = arr.last().cloned().unwrap_or(json!({}));
                        let mut ac = vc.clone();
                        ac["id"] = json!(video::new_id("c"));
                        ac["track"] = json!("A1"); ac["type"] = json!("audio");
                        ac["name"] = json!(format!("{} · audio", a.name));
                        ac["link"] = json!(link);
                        arr.push(ac);
                        log.push(format!("add_clip {cid} + linked A1 partner"));
                    } else {
                        log.push(format!("add_clip {cid}"));
                    }
                } else {
                    if c.get("track").is_none() { c["track"] = json!("T1"); }
                    let start = c.get("start").and_then(|x| x.as_f64()).unwrap_or(0.0);
                    if c.get("end").is_none() { c["end"] = json!(start + 3.0); }
                    if c.get("name").is_none() { c["name"] = json!(c.get("text").and_then(|t| t.get("content")).and_then(|x| x.as_str()).unwrap_or("Title")); }
                    v["clips"].as_array_mut().ok_or("clips")?.push(c);
                    log.push(format!("add_clip {cid}"));
                }
            }
            "update_clip" => {
                let patch = op.get("patch").cloned().ok_or("update_clip needs patch")?;
                let arr = v["clips"].as_array_mut().ok_or("clips")?;
                let c = arr.iter_mut().find(|c| c["id"] == id).ok_or_else(|| format!("update_clip: no clip {id}"))?;
                merge(c, &patch);
                c["id"] = json!(id);
                log.push(format!("update_clip {id}"));
            }
            "remove_clip" => {
                let ripple = op.get("ripple").and_then(|r| r.as_bool()).unwrap_or(false);
                let arr = v["clips"].as_array_mut().ok_or("clips")?;
                let targets = linked_ids(arr, &HashSet::from([id.clone()]));
                let mut removed: Vec<Value> = vec![];
                arr.retain(|c| {
                    let hit = c.get("id").and_then(|x| x.as_str()).map(|x| targets.contains(x)).unwrap_or(false);
                    if hit { removed.push(c.clone()); }
                    !hit
                });
                if removed.is_empty() { return Err(format!("remove_clip: no clip {id}")); }
                if ripple {
                    let mut removed = removed;
                    removed.sort_by(|a, b| b["start"].as_f64().unwrap_or(0.0).partial_cmp(&a["start"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
                    for r in removed.iter() {
                        let (s, e, tr) = (r["start"].as_f64().unwrap_or(0.0), r["end"].as_f64().unwrap_or(0.0), r["track"].as_str().unwrap_or("").to_string());
                        let d = e - s;
                        for c in arr.iter_mut().filter(|c| c["track"] == tr && c["start"].as_f64().unwrap_or(0.0) >= e - 1e-6) {
                            c["start"] = json!(c["start"].as_f64().unwrap_or(0.0) - d); c["end"] = json!(c["end"].as_f64().unwrap_or(0.0) - d);
                        }
                    }
                }
                log.push(format!("remove_clip {id}{}", if ripple { " (ripple)" } else { "" }));
            }
            "move" => {
                let start = op.get("start").and_then(|x| x.as_f64()).ok_or("move needs start")?;
                let arr = v["clips"].as_array_mut().ok_or("clips")?;
                let idx = arr.iter().position(|c| c["id"] == id).ok_or_else(|| format!("move: no clip {id}"))?;
                let old_start = arr[idx]["start"].as_f64().unwrap_or(0.0);
                let delta = start.max(0.0) - old_start;
                let targets = linked_ids(arr, &HashSet::from([id.clone()]));
                for c in arr.iter_mut().filter(|c| c.get("id").and_then(|x| x.as_str()).map(|x| targets.contains(x)).unwrap_or(false)) {
                    let d = c["end"].as_f64().unwrap_or(0.0) - c["start"].as_f64().unwrap_or(0.0);
                    let ns = (c["start"].as_f64().unwrap_or(0.0) + delta).max(0.0);
                    c["start"] = json!(ns); c["end"] = json!(ns + d);
                }
                if let Some(t) = op.get("track").and_then(|t| t.as_str()) {
                    if let Some(c) = arr.iter_mut().find(|c| c["id"] == id) { c["track"] = json!(t); }
                }
                log.push(format!("move {id} → {start:.3}"));
            }
            "split" => {
                let at = op.get("at").and_then(|x| x.as_f64()).ok_or("split needs at")?;
                let arr = v["clips"].as_array_mut().ok_or("clips")?;
                if !arr.iter().any(|c| c["id"] == id) { return Err(format!("split: no clip {id}")); }
                // Split the target AND its linked partners at the same timeline
                // time; left halves share one fresh link, right halves another.
                // (Two passes: collect first, mutate second — one borrow at a time.)
                let targets = linked_ids(arr, &HashSet::from([id.clone()]));
                let mut fresh: HashMap<String, String> = HashMap::new();
                let mut made: Vec<String> = vec![];
                let mut jobs: Vec<(usize, Clip, f64, String)> = vec![];
                for (idx, c) in arr.iter().enumerate() {
                    let cid = c.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if !targets.contains(&cid) { continue; }
                    let (cs, ce) = (c["start"].as_f64().unwrap_or(0.0), c["end"].as_f64().unwrap_or(0.0));
                    if at <= cs + 0.02 || at >= ce - 0.02 { continue; }
                    let cc: Clip = serde_json::from_value(c.clone()).map_err(|e| e.to_string())?;
                    let src_at = cc.in_ + (at - cc.start) * cc.speed;
                    let key = if cc.link.is_empty() { format!("solo:{cid}") } else { cc.link.clone() };
                    let nl = fresh.entry(key).or_insert_with(|| video::new_id("l")).clone();
                    jobs.push((idx, cc, src_at, nl));
                }
                let mut inserts: Vec<(usize, Value)> = vec![];
                for (idx, cc, src_at, nl) in jobs {
                    let mut b = cc.clone(); b.id = video::new_id("c"); b.start = at; b.in_ = src_at; b.link = nl.clone();
                    b.transition_in = Default::default();
                    made.push(b.id.clone());
                    inserts.push((idx, serde_json::to_value(&b).map_err(|e| e.to_string())?));
                    // shrink the left half in place
                    if let Some(a) = arr.get_mut(idx) {
                        a["end"] = json!(at); a["out"] = json!(src_at);
                        a["transitionOut"] = json!({});
                        if !cc.link.is_empty() { a["link"] = json!(nl); }
                    }
                }
                if made.is_empty() {
                    let c = arr.iter().find(|c| c["id"] == id).cloned().unwrap_or(json!({}));
                    return Err(format!("split: {at} is not inside clip {id} ({}..{})", c["start"].as_f64().unwrap_or(0.0), c["end"].as_f64().unwrap_or(0.0)));
                }
                inserts.sort_by_key(|(idx, _)| *idx);
                for (n, (idx, b)) in inserts.into_iter().enumerate() { arr.insert(idx + 1 + n, b); }
                log.push(format!("split {id} at {at:.3} → {}", made.join(", ")));
            }
            "cutlist" => {
                let aid = op.get("asset").and_then(|x| x.as_str()).unwrap_or("");
                let a = assets.iter().find(|a| a.id == aid || a.name == aid).ok_or_else(|| format!("cutlist: unknown asset {aid}"))?;
                let track = op.get("track").and_then(|t| t.as_str()).unwrap_or("V1").to_string();
                let pad = op.get("pad").and_then(|p| p.as_f64()).unwrap_or(0.0).max(0.0);
                let keep: Vec<(f64, f64)> = op.get("keep").and_then(|k| k.as_array()).ok_or("cutlist needs keep[]")?.iter()
                    .filter_map(|r| Some(((r.get("in")?.as_f64()? - pad).max(0.0), (r.get("out")?.as_f64()? + pad).min(a.duration.max(0.04)))))
                    .filter(|(i, o)| o > i).collect();
                let n = apply_cutlist(&mut v, a, &track, &keep)?;
                log.push(format!("cutlist {} → {n} clips on {track}", a.id));
            }
            other => return Err(format!("op #{i}: unknown op {other}")),
        }
    }
    let comp = vr::parse_composition(&v, &assets)?;
    save_comp(broker, agent_id, project, &comp)?;
    Ok(json!({ "ok": true, "applied": log, "duration": round3(comp.duration()), "clips": comp.clips.len() }))
}

/// Replace `asset`'s clips on `track` with contiguous selects for `keep` (source ranges).
/// Video assets with audio also lay matching linked A1 partners (the timeline's
/// picture+waveform pairs) sharing a fresh link id per select.
fn apply_cutlist(v: &mut Value, a: &Asset, track: &str, keep: &[(f64, f64)]) -> Result<usize, String> {
    let arr = v["clips"].as_array_mut().ok_or("clips")?;
    // where the existing selects started on the timeline (keep that origin)
    let origin = arr.iter().filter(|c| c["track"] == track && c["asset"] == a.id.as_str()).filter_map(|c| c["start"].as_f64()).fold(f64::MAX, f64::min);
    let origin = if origin == f64::MAX { 0.0 } else { origin };
    let template = arr.iter().find(|c| c["track"] == track && c["asset"] == a.id.as_str()).cloned();
    arr.retain(|c| !(c["track"] == track && c["asset"] == a.id.as_str()));
    // linked A1 partners of the old selects go too (they're rebuilt below)
    arr.retain(|c| !(c["track"] == "A1" && c["asset"] == a.id.as_str() && c.get("link").and_then(|x| x.as_str()).map(|x| !x.is_empty()).unwrap_or(false) && (track == "V1" || track == "V2")));
    let mut t = origin;
    let mut sorted = keep.to_vec();
    sorted.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut n = 0;
    for (i, (in_, out)) in sorted.iter().enumerate() {
        let d = out - in_;
        let mut c = template.clone().unwrap_or_else(|| json!({ "type": if a.kind == "audio" { "audio" } else { "video" }, "fit": "cover" }));
        c["id"] = json!(video::new_id("c")); c["track"] = json!(track); c["asset"] = json!(a.id);
        c["name"] = json!(format!("{} · {}", a.name, i + 1));
        c["start"] = json!(round3(t)); c["end"] = json!(round3(t + d)); c["in"] = json!(round3(*in_)); c["out"] = json!(round3(*out));
        c["transitionIn"] = json!({}); c["transitionOut"] = json!({});
        if a.has_audio && (track == "V1" || track == "V2") {
            let link = video::new_id("l");
            c["link"] = json!(link);
            let mut ac = c.clone();
            ac["id"] = json!(video::new_id("c"));
            ac["track"] = json!("A1"); ac["type"] = json!("audio");
            ac["name"] = json!(format!("{} · audio {}", a.name, i + 1));
            ac["link"] = json!(link);
            arr.push(c);
            arr.push(ac);
        } else {
            arr.push(c);
        }
        t += d; n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Transcription (Whisper verbose_json, chunked)
// ---------------------------------------------------------------------------

fn run_thread<T: Send + 'static>(f: impl std::future::Future<Output = T> + Send + 'static) -> Result<T, String> {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        Ok::<T, String>(rt.block_on(f))
    }).join().map_err(|_| "worker thread panicked".to_string())?
}

const CHUNK_SECS: f64 = 20.0 * 60.0;

pub fn transcribe(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, asset: Option<&str>, language: Option<&str>) -> Result<(Asset, Transcript), String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let a = find_asset(broker, agent_id, project, asset, true)?;
    if !a.has_audio { return Err(format!("asset {} has no audio", a.name)); }
    let key = crate::keychain::get_key("openai").map_err(|_| "no OpenAI key set — add one in Settings to transcribe".to_string())?;
    let abs = video::asset_abs(broker, agent_id, project, &a)?;
    let ff = video::ffmpeg(app)?;
    let cache = proj.join(".cache"); std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let n = (a.duration / CHUNK_SECS).ceil().max(1.0) as usize;
    let mut words: Vec<Word> = vec![]; let mut segs: Vec<Segment> = vec![]; let mut text = String::new(); let mut lang = String::new();
    for i in 0..n {
        let off = i as f64 * CHUNK_SECS;
        let chunk = cache.join(format!("{}.tx{i}.mp3", a.id));
        let o = Command::new(&ff).args(["-v", "error", "-y", "-ss", &format!("{off:.3}"), "-t", &format!("{CHUNK_SECS:.3}"), "-i"]).arg(&abs)
            .args(["-vn", "-ac", "1", "-ar", "16000", "-b:a", "48k"]).arg(&chunk).output().map_err(|e| format!("ffmpeg: {e}"))?;
        if !o.status.success() { return Err(format!("audio extract failed: {}", String::from_utf8_lossy(&o.stderr).chars().take(300).collect::<String>())); }
        let bytes = std::fs::read(&chunk).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&chunk);
        if bytes.len() < 1000 { continue; }
        let key2 = key.clone(); let lang_hint = language.map(String::from);
        let v: Value = run_thread(async move {
            let mut form = reqwest::multipart::Form::new().text("model", "whisper-1").text("response_format", "verbose_json").text("timestamp_granularities[]", "word").text("timestamp_granularities[]", "segment")
                .part("file", reqwest::multipart::Part::bytes(bytes).file_name("audio.mp3").mime_str("audio/mpeg").map_err(|e| e.to_string())?);
            if let Some(l) = lang_hint { form = form.text("language", l); }
            let resp = reqwest::Client::new().post("https://api.openai.com/v1/audio/transcriptions").bearer_auth(key2).multipart(form).send().await.map_err(|e| format!("whisper: {e}"))?;
            if !resp.status().is_success() { let st = resp.status(); let b = resp.text().await.unwrap_or_default(); return Err(format!("whisper {st}: {}", b.chars().take(300).collect::<String>())); }
            resp.json::<Value>().await.map_err(|e| format!("whisper parse: {e}"))
        })??;
        if lang.is_empty() { lang = v["language"].as_str().unwrap_or("").to_string(); }
        if let Some(t) = v["text"].as_str() { if !text.is_empty() { text.push(' '); } text.push_str(t.trim()); }
        for w in v["words"].as_array().cloned().unwrap_or_default() {
            let (Some(s), Some(e)) = (w["start"].as_f64(), w["end"].as_f64()) else { continue };
            words.push(Word { w: w["word"].as_str().unwrap_or("").trim().to_string(), s: round3(s + off), e: round3(e + off) });
        }
        for g in v["segments"].as_array().cloned().unwrap_or_default() {
            let (Some(s), Some(e)) = (g["start"].as_f64(), g["end"].as_f64()) else { continue };
            segs.push(Segment { text: g["text"].as_str().unwrap_or("").trim().to_string(), s: round3(s + off), e: round3(e + off) });
        }
    }
    if segs.is_empty() && !words.is_empty() { segs = sentences_from_words(&words); }
    let tr = Transcript { asset: a.id.clone(), language: lang, text, words, segments: segs, created: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) };
    video::write_json(broker, agent_id, &video::rel(project, "transcript.json"), &serde_json::to_value(&tr).map_err(|e| e.to_string())?)?;
    Ok((a, tr))
}

fn sentences_from_words(words: &[Word]) -> Vec<Segment> {
    let mut out = vec![]; let mut cur: Vec<&Word> = vec![];
    for w in words {
        cur.push(w);
        if w.w.ends_with(['.', '?', '!']) || cur.len() >= 18 { out.push(Segment { text: cur.iter().map(|x| x.w.as_str()).collect::<Vec<_>>().join(" "), s: cur[0].s, e: w.e }); cur.clear(); }
    }
    if !cur.is_empty() { out.push(Segment { text: cur.iter().map(|x| x.w.as_str()).collect::<Vec<_>>().join(" "), s: cur[0].s, e: cur.last().unwrap().e }); }
    out
}

// ---------------------------------------------------------------------------
// Silence + takes + auto cut
// ---------------------------------------------------------------------------

pub fn silences(app: &tauri::AppHandle, abs: &Path, threshold_db: f64, min_dur: f64) -> Result<Vec<(f64, f64)>, String> {
    let ff = video::ffmpeg(app)?;
    let o = Command::new(&ff).args(["-hide_banner", "-nostats", "-i"]).arg(abs)
        .args(["-vn", "-af", &format!("silencedetect=noise={threshold_db:.1}dB:d={:.3}", min_dur.max(0.05)), "-f", "null", "-"]).output().map_err(|e| format!("ffmpeg: {e}"))?;
    let err = String::from_utf8_lossy(&o.stderr);
    let mut out = vec![]; let mut start: Option<f64> = None;
    for line in err.lines() {
        if let Some(i) = line.find("silence_start:") { start = line[i + 14..].trim().split_whitespace().next().and_then(|x| x.parse().ok()); }
        else if let Some(i) = line.find("silence_end:") {
            let e: Option<f64> = line[i + 12..].trim().split_whitespace().next().and_then(|x| x.parse().ok());
            if let (Some(s), Some(e)) = (start.take(), e) { out.push((s, e)); }
        }
    }
    if let Some(s) = start { out.push((s, f64::MAX)); } // trailing silence to EOF
    Ok(out)
}

fn norm_tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|t| t.len() > 1 && !matches!(*t, "um" | "uh" | "like" | "so" | "the" | "and" | "a" | "to" | "of" | "is" | "it" | "that" | "in")).map(String::from).collect()
}
fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() { return 0.0; }
    let sa: std::collections::HashSet<&String> = a.iter().collect(); let sb: std::collections::HashSet<&String> = b.iter().collect();
    let inter = sa.intersection(&sb).count() as f64; let uni = sa.union(&sb).count() as f64;
    if uni == 0.0 { 0.0 } else { inter / uni }
}
/// Prefix-overlap: a false start ("So today we— So today we're going to") shares a leading run.
fn prefix_sim(a: &[String], b: &[String]) -> f64 {
    let n = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    if a.is_empty() { 0.0 } else { n as f64 / a.len().min(b.len()).max(1) as f64 }
}

pub fn takes(tr: &Transcript, keep: &str, sim: f64) -> Vec<Value> {
    let segs = &tr.segments;
    let toks: Vec<Vec<String>> = segs.iter().map(|s| norm_tokens(&s.text)).collect();
    let mut used = vec![false; segs.len()];
    let mut groups = vec![];
    for i in 0..segs.len() {
        if used[i] { continue; }
        let mut g = vec![i];
        let mut j = i + 1;
        while j < segs.len() && j <= i + 5 && segs[j].s - segs[g[g.len() - 1]].e < 45.0 {
            let last = g[g.len() - 1];
            let s1 = jaccard(&toks[last], &toks[j]); let s2 = prefix_sim(&toks[last], &toks[j]);
            let short_false_start = toks[last].len() <= 6 && s2 >= 0.5;
            if s1 >= sim || s2 >= 0.75 || short_false_start { g.push(j); }
            j += 1;
        }
        if g.len() > 1 {
            for &k in &g { used[k] = true; }
            let keep_idx = if keep == "longest" { *g.iter().max_by(|a, b| toks[**a].len().cmp(&toks[**b].len()).then((segs[**a].e - segs[**a].s).partial_cmp(&(segs[**b].e - segs[**b].s)).unwrap_or(std::cmp::Ordering::Equal))).unwrap() } else { *g.last().unwrap() };
            groups.push(json!({ "keep": keep_idx, "takes": g.iter().map(|&k| json!({ "index": k, "s": round2(segs[k].s), "e": round2(segs[k].e), "text": segs[k].text, "keep": k == keep_idx })).collect::<Vec<_>>() }));
        }
    }
    groups
}

fn subtract(keep: Vec<(f64, f64)>, cut: &(f64, f64)) -> Vec<(f64, f64)> {
    let mut out = vec![];
    for (s, e) in keep {
        if cut.1 <= s || cut.0 >= e { out.push((s, e)); continue; }
        if cut.0 > s { out.push((s, cut.0)); }
        if cut.1 < e { out.push((cut.1, e)); }
    }
    out
}

pub fn auto_cut(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, asset: Option<&str>, threshold_db: f64, min_silence: f64, pad: f64, keep: &str, drop_takes: bool) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let a = find_asset(broker, agent_id, project, asset, true)?;
    let abs = video::asset_abs(broker, agent_id, project, &a)?;
    let dur = a.duration.max(0.04);
    let sil = silences(app, &abs, threshold_db, min_silence)?;
    let mut keep_ranges = vec![(0.0, dur)];
    let mut dropped_sil = 0.0;
    for (s, e) in &sil {
        let e = e.min(dur);
        let (cs, ce) = ((s + pad).min(e), (e - pad).max(*s));
        if ce - cs < 0.05 { continue; }
        dropped_sil += ce - cs;
        keep_ranges = subtract(keep_ranges, &(cs, ce));
    }
    let mut dropped_takes: Vec<Value> = vec![];
    let mut groups: Vec<Value> = vec![];
    if drop_takes {
        let tr = match load_transcript(&proj).filter(|t| t.asset == a.id) { Some(t) => t, None => transcribe(app, broker, agent_id, project, Some(&a.id), None)?.1 };
        groups = takes(&tr, keep, 0.6);
        for g in &groups {
            for t in g["takes"].as_array().cloned().unwrap_or_default() {
                if t["keep"].as_bool().unwrap_or(false) { continue; }
                let (s, e) = (t["s"].as_f64().unwrap_or(0.0), t["e"].as_f64().unwrap_or(0.0));
                // expand to the silence boundaries around the take so no half-word survives
                let s2 = sil.iter().filter(|(_, se)| *se <= s + 0.15).map(|(ss, _)| *ss).fold(s, f64::max).min(s);
                let e2 = sil.iter().filter(|(ss, _)| *ss >= e - 0.15).map(|(_, se)| *se).fold(e, f64::min).max(e);
                keep_ranges = subtract(keep_ranges, &(s2.max(0.0), e2.min(dur)));
                dropped_takes.push(json!({ "s": round2(s2), "e": round2(e2), "text": t["text"] }));
            }
        }
    }
    // tidy: merge gaps < 0.12s, drop keeps < 0.25s
    keep_ranges.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f64, f64)> = vec![];
    for r in keep_ranges { if let Some(l) = merged.last_mut() { if r.0 - l.1 < 0.12 { l.1 = r.1; continue; } } merged.push(r); }
    merged.retain(|(s, e)| e - s >= 0.25);
    if merged.is_empty() { return Err("auto cut removed everything — raise threshold_db (e.g. -45) or min_silence".into()); }
    let ops = vec![json!({ "op": "cutlist", "asset": a.id, "track": "V1", "keep": merged.iter().map(|(s, e)| json!({ "in": round3(*s), "out": round3(*e) })).collect::<Vec<_>>() })];
    let r = edit(broker, agent_id, project, &ops)?;
    let kept: f64 = merged.iter().map(|(s, e)| e - s).sum();
    Ok(json!({ "asset": a.id, "source_duration": round2(dur), "kept_duration": round2(kept), "removed_silence": round2(dropped_sil), "clips": r["clips"], "keep": merged.iter().map(|(s, e)| json!({ "in": round2(*s), "out": round2(*e) })).collect::<Vec<_>>(), "dropped_takes": dropped_takes, "take_groups": groups.len(), "note": "review the V1+A1 selects; use video_edit update_clip/split to fine-tune, or re-run with a different threshold" }))
}

// ---------------------------------------------------------------------------
// Audio enhance (Auphonic / local)
// ---------------------------------------------------------------------------

fn register_generated(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, file: &Path, name: &str, kind: &str) -> Result<Asset, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let mut m = video::load_manifest(broker, agent_id, project);
    let rel = format!("media/{}", file.file_name().and_then(|n| n.to_str()).unwrap_or("out"));
    m.assets.retain(|a| a.rel != rel);
    let mut a = Asset { id: video::new_id("a"), name: name.to_string(), kind: kind.to_string(), rel: rel.clone(), path: file.to_string_lossy().to_string(), linked: true, size: std::fs::metadata(file).map(|x| x.len()).unwrap_or(0), imported: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0), ..Default::default() };
    video::probe_into(app, file, &mut a)?;
    a.thumbs = video::build_thumbs(app, &proj, file, &a).ok();
    m.assets.push(a.clone());
    video::save_manifest(broker, agent_id, project, &m)?;
    Ok(a)
}

pub fn enhance(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, asset: &str, normalize_db: f64, denoise: f64) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let a = find_asset(broker, agent_id, project, Some(asset), true)?;
    if !a.has_audio { return Err("asset has no audio".into()); }
    let abs = video::asset_abs(broker, agent_id, project, &a)?;
    let ff = video::ffmpeg(app)?;
    let media = proj.join("media"); std::fs::create_dir_all(&media).map_err(|e| e.to_string())?;
    let stem: String = Path::new(&a.name).file_stem().and_then(|s| s.to_str()).unwrap_or("audio").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let out = media.join(format!("{stem}.cleaned.wav"));
    // 1) extract a clean WAV
    let wav = proj.join(".cache").join(format!("{}.src.wav", a.id));
    std::fs::create_dir_all(wav.parent().unwrap()).map_err(|e| e.to_string())?;
    let o = Command::new(&ff).args(["-v", "error", "-y", "-i"]).arg(&abs).args(["-vn", "-ac", "2", "-ar", "48000", "-c:a", "pcm_s16le"]).arg(&wav).output().map_err(|e| format!("ffmpeg: {e}"))?;
    if !o.status.success() { return Err(format!("extract failed: {}", String::from_utf8_lossy(&o.stderr).chars().take(300).collect::<String>())); }
    // LOCAL ONLY: highpass -> afftdn (strength-mapped) -> gentle leveling.
    // Peak normalize to normalizeDb + optional loudnorm stay in the TIMELINE
    // mix (audio.normalizeDb on export), so this file is denoise + leveling only.
    let dn = denoise.clamp(0.0, 1.0);
    let nr = (dn * 18.0).round() as i64; // 0..18 dB reduction (stable floor, no noise-tracking pump)
    let af = if dn <= 0.001 {
        "highpass=f=70,acompressor=threshold=-20dB:ratio=2:attack=10:release=150:makeup=2".to_string()
    } else {
        format!("highpass=f=70,afftdn=nr={nr}:nf=-30,acompressor=threshold=-20dB:ratio=2:attack=10:release=150:makeup=2")
    };
    let o = Command::new(&ff).args(["-v", "error", "-y", "-i"]).arg(&wav).args(["-af", &af, "-ar", "48000", "-c:a", "pcm_s16le"]).arg(&out).output().map_err(|e| format!("ffmpeg: {e}"))?;
    if !o.status.success() { return Err(format!("local enhance failed: {}", String::from_utf8_lossy(&o.stderr).chars().take(400).collect::<String>())); }
    // Full-length proxy: video sources get a .cleaned.mov (picture stream-copied,
    // cleaned audio padded to the full video duration + AAC for browser playback)
    // so preview plays picture+sound as ONE element — no dual-element drift.
    // Audio-only sources keep the cleaned .wav directly.
    let dur_full = a.duration.max(0.04);
    let (rep_path, rep_kind): (std::path::PathBuf, &str) = if a.has_video {
        let padded = proj.join(".cache").join(format!("{}.pad.wav", a.id));
        let apad = format!("apad=whole_dur={:.3}", dur_full);
        let mov = media.join(format!("{stem}.cleaned.mov"));
        let pad_ok = Command::new(&ff).args(["-v", "error", "-y", "-i"]).arg(&out).args(["-af", apad.as_str(), "-ar", "48000", "-c:a", "pcm_s16le"]).arg(&padded).output().map(|o| o.status.success()).unwrap_or(false);
        let mux_ok = if pad_ok {
            Command::new(&ff).args(["-v", "error", "-y", "-i"]).arg(&abs).args(["-i"]).arg(&padded)
                .args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-ac", "2",
                       "-movflags", "+faststart", "-t", &format!("{:.3}", dur_full)]).arg(&mov)
                .output().map(|o| o.status.success() && mov.is_file()).unwrap_or(false)
        } else { false };
        let _ = std::fs::remove_file(&padded);
        if mux_ok {
            // intermediate wav is baked into the proxy; don't leave an orphan
            let _ = std::fs::remove_file(&out);
            (mov, "video")
        } else { (out.clone(), "audio") }
    } else { (out.clone(), "audio") };
    let _ = std::fs::remove_file(&wav);
    // Drop superseded cleaned replacements for this source (wav<->mov on
    // re-clean) so Generated doesn't pile up stale files.
    {
        let cleaned_name = format!("{} (cleaned)", a.name);
        let rep_rel = rep_path.strip_prefix(&proj).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let mut m = video::load_manifest(broker, agent_id, project);
        let before = m.assets.len();
        let mut gone: Vec<String> = vec![];
        m.assets.retain(|x| {
            let hit = x.name == cleaned_name && x.id != a.id;
            // never delete the file we just wrote (same rel on re-clean)
            if hit && x.rel != rep_rel { gone.push(x.rel.clone()); }
            !hit
        });
        if m.assets.len() != before {
            for rel in &gone {
                if rel.starts_with("media/") || rel.starts_with(".cache/") {
                    let _ = std::fs::remove_file(proj.join(rel));
                }
            }
            video::save_manifest(broker, agent_id, project, &m)?;
        }
    }
    let na = register_generated(app, broker, agent_id, project, &rep_path, &format!("{} (cleaned)", a.name), rep_kind)?;
    // point every clip of the source asset at the cleaned replacement
    let mut comp = load_comp(broker, agent_id, project)?;
    let mut n = 0;
    for c in comp.clips.iter_mut().filter(|c| c.asset == a.id && c.kind == "video") { c.audio_asset = na.id.clone(); n += 1; }
    comp.audio.enhance = "local".into();
    comp.audio.normalize_db = normalize_db.clamp(-24.0, 0.0);
    comp.audio.denoise = dn;
    comp.audio.clean_enabled = true;
    save_comp(broker, agent_id, project, &comp)?;
    Ok(json!({ "ok": true, "engine": "local", "asset": na.id, "file": na.rel, "clipsUpdated": n, "normalizeDb": comp.audio.normalize_db, "denoise": dn }))
}

/// Render a short denoise audition sample to .cache/ WITHOUT touching the
/// timeline. Same chain as enhance (highpass + afftdn at strength + leveling),
/// so what you hear is what the cleanup produces.
pub fn audition(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, asset: &str, denoise: f64, at: f64, len: f64) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let a = find_asset(broker, agent_id, project, Some(asset), true)?;
    if !a.has_audio { return Err("asset has no audio".into()); }
    let abs = video::asset_abs(broker, agent_id, project, &a)?;
    let ff = video::ffmpeg(app)?;
    let at = at.max(0.0).min(a.duration.max(0.0));
    let len = len.clamp(1.0, 30.0).min((a.duration - at).max(1.0));
    let dn = denoise.clamp(0.0, 1.0);
    let nr = (dn * 18.0).round() as i64; // 0..18 dB (matches enhance)
    let af = if dn <= 0.001 {
        "highpass=f=70,acompressor=threshold=-20dB:ratio=2:attack=10:release=150:makeup=2".to_string()
    } else {
        format!("highpass=f=70,afftdn=nr={nr}:nf=-30,acompressor=threshold=-20dB:ratio=2:attack=10:release=150:makeup=2")
    };
    let cache = proj.join(".cache");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let name = format!("audition-{}-dn{:02}.wav", a.id, (dn * 100.0).round() as i64);
    let out = cache.join(&name);
    let _ = tauri::Emitter::emit(app, "video-tool-progress", json!({ "project": project, "tool": "video_audio_audition", "msg": "rendering audition…" }));
    let o = Command::new(&ff).args(["-v", "error", "-y", "-ss", &format!("{at:.3}"), "-t", &format!("{len:.3}"), "-i"]).arg(&abs)
        .args(["-vn", "-af", &af, "-ar", "48000", "-c:a", "pcm_s16le"]).arg(&out).output().map_err(|e| format!("ffmpeg: {e}"))?;
    if !o.status.success() { return Err(format!("audition failed: {}", String::from_utf8_lossy(&o.stderr).chars().take(300).collect::<String>())); }
    Ok(json!({ "ok": true, "sample": format!(".cache/{name}"), "denoise": dn, "at": at, "len": len, "note": "listen in the Video tab (Media > Generated, or the preview below), then run video_audio_enhance with the denoise you like" }))
}

// ---------------------------------------------------------------------------
// Matte (RobustVideoMatting via uv) — experimental
// ---------------------------------------------------------------------------

const RVM_SCRIPT: &str = r#"
import sys, torch, av, numpy as np
src, dst = sys.argv[1], sys.argv[2]
dev = "mps" if torch.backends.mps.is_available() else ("cuda" if torch.cuda.is_available() else "cpu")
model = torch.hub.load("PeterL1n/RobustVideoMatting", "mobilenetv3").eval().to(dev)
inp = av.open(src); vs = inp.streams.video[0]; vs.thread_type = "AUTO"
out = av.open(dst, "w"); os_ = out.add_stream("libx264", rate=vs.average_rate); os_.width = vs.codec_context.width; os_.height = vs.codec_context.height; os_.pix_fmt = "yuv420p"; os_.options = {"crf": "16", "preset": "fast"}
rec = [None] * 4
ratio = 0.25 if vs.codec_context.width >= 1920 else 0.375
n = 0
with torch.no_grad():
    for frame in inp.decode(vs):
        img = torch.from_numpy(frame.to_ndarray(format="rgb24")).to(dev).permute(2, 0, 1).float().div(255).unsqueeze(0)
        fgr, pha, *rec = model(img, *rec, downsample_ratio=ratio)
        a = (pha[0, 0].clamp(0, 1) * 255).byte().cpu().numpy()
        gray = np.repeat(a[:, :, None], 3, axis=2)
        vf = av.VideoFrame.from_ndarray(gray, format="rgb24")
        for p in os_.encode(vf): out.mux(p)
        n += 1
        if n % 30 == 0: print(f"frames={n}", flush=True)
for p in os_.encode(): out.mux(p)
out.close(); inp.close()
print(f"done frames={n}")
"#;

pub fn matte(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, asset: &str) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let a = find_asset(broker, agent_id, project, Some(asset), false)?;
    if !a.has_video { return Err("asset has no video".into()); }
    let abs = video::asset_abs(broker, agent_id, project, &a)?;
    let uv = crate::provision::uv_bin(app).ok_or("uv is not provisioned — enable a Python MCP server once in MCP Connections (it installs uv), then retry")?;
    let cache = proj.join(".cache"); std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let script = cache.join("rvm_matte.py");
    std::fs::write(&script, RVM_SCRIPT).map_err(|e| e.to_string())?;
    let stem: String = Path::new(&a.name).file_stem().and_then(|s| s.to_str()).unwrap_or("clip").chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let out = proj.join("media").join(format!("{stem}.alpha.mp4"));
    let _ = tauri::Emitter::emit(app, "video-tool-progress", json!({ "project": project, "tool": "video_matte", "msg": "running RobustVideoMatting (first run downloads torch + weights)" }));
    let mut cmd = Command::new(&uv);
    cmd.args(["run", "--python", "3.11", "--with", "torch", "--with", "torchvision", "--with", "av", "--with", "numpy"]).arg(&script).arg(&abs).arg(&out);
    for (k, v) in crate::provision::uv_env(app) { cmd.env(k, v); }
    cmd.env("PYTORCH_ENABLE_MPS_FALLBACK", "1");
    let o = cmd.output().map_err(|e| format!("uv: {e}"))?;
    if !o.status.success() || !out.is_file() {
        let e = String::from_utf8_lossy(&o.stderr);
        return Err(format!("matte failed: {}", e.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")));
    }
    let na = register_generated(app, broker, agent_id, project, &out, &format!("{} (alpha)", a.name), "video")?;
    let mut comp = load_comp(broker, agent_id, project)?;
    comp.matte.enabled = true; comp.matte.source_asset = a.id.clone(); comp.matte.alpha_asset = na.id.clone();
    save_comp(broker, agent_id, project, &comp)?;
    Ok(json!({ "ok": true, "alphaAsset": na.id, "file": na.rel, "note": "matte.enabled = true; set behindSubject on overlay clips to place them behind the person" }))
}
