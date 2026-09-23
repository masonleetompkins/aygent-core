// AYGENT — VIDEO v0.3: project store + media import + probe/thumbs + chat.
//
// A video project lives in the agent's jailed folder:
//   Video/<project>/composition.json   the edit (agent + UI both read/write)
//   Video/<project>/assets.json        imported media manifest (import writes)
//   Video/<project>/transcript.json    word-level transcript (video_transcribe)
//   Video/<project>/chat.json          the agent dock conversation
//   Video/<project>/media/<file>       HARDLINKS to the source footage (never copies)
//   Video/<project>/.cache/            filmstrips, waveforms, captions.ass, frames
//   Video/<project>/renders/           exports
//   Video/<project>/.gitignore         media/ .cache/ renders/ — keeps Save Points lean
//
// Every path resolves through the broker (per-agent scope, fail-closed). Reads of
// hardlinked media are admitted (rule 7 only refuses WRITES to nlink>1 files);
// sources on another volume can't be hardlinked (EXDEV) and are stored as an
// allow-listed absolute reference in assets.json instead — served ONLY via the
// aygent-media protocol (video_media.rs) after that manifest lookup.
//
// ffmpeg/ffprobe are the provisioned binaries (provision.rs); this module never
// spawns anything the agent names.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::broker::{self, Broker};

pub const VIDEO_ROOT: &str = "Video";

pub fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_missing(e: &broker::BrokerError) -> bool {
    let s = format!("{e:?}");
    s.contains("NotFound") || s.contains("No such file") || s.contains("errno Some(2)")
}

pub fn read_json(broker: &Broker, agent_id: &str, rel: &str) -> Option<serde_json::Value> {
    let abs = broker.resolve(agent_id, rel, broker::Mode::Read).ok()?;
    let text = std::fs::read_to_string(&abs).ok()?;
    if text.trim().is_empty() { return None; }
    serde_json::from_str(&text).ok()
}

pub fn write_json(broker: &Broker, agent_id: &str, rel: &str, v: &serde_json::Value) -> Result<PathBuf, String> {
    let text = serde_json::to_string_pretty(v).map_err(|e| format!("serialize: {e}"))?;
    if text.len() > 8 * 1024 * 1024 { return Err("file too large (>8MB)".into()); }
    let abs = broker.resolve(agent_id, rel, broker::Mode::Write).map_err(|e| format!("refused by jail: {e:?}"))?;
    if let Some(parent) = abs.parent() { std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?; }
    std::fs::write(&abs, text.as_bytes()).map_err(|e| format!("write: {e}"))?;
    Ok(abs)
}

/// Absolute project dir (must exist). Read-mode resolve = "is it inside the jail".
pub fn project_dir(broker: &Broker, agent_id: &str, project: &str) -> Result<PathBuf, String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(project) { return Err("invalid project name — use letters, digits, - _".into()); }
    let rel = format!("{VIDEO_ROOT}/{project}");
    let abs = broker.resolve(agent_id, &rel, broker::Mode::Read).map_err(|e| format!("refused by jail: {e:?}"))?;
    if !abs.is_dir() { return Err(format!("no such project: {project}")); }
    Ok(abs)
}

pub fn rel(project: &str, tail: &str) -> String { format!("{VIDEO_ROOT}/{project}/{tail}") }

/// Sequence file for an edit. The default sequence is the legacy
/// `composition.json` (so old projects open untouched); named sequences live
/// at `sequences/<name>.json`. Names are slug-checked at every entry point.
pub fn seq_rel(project: &str, sequence: &str) -> String {
    if sequence.is_empty() || sequence == "main" { rel(project, "composition.json") }
    else { rel(project, &format!("sequences/{sequence}.json")) }
}

pub fn seq_ok(s: &str) -> bool { s.is_empty() || s == "main" || slug_ok(s) }

/// List sequences in a project: the default (`main`, when composition.json
/// exists) plus every `sequences/*.json`. Returns (name, modified) sorted
/// with main first, then by recency.
pub fn list_sequences(broker: &Broker, agent_id: &str, project: &str) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    let Ok(dir) = project_dir(broker, agent_id, project) else { return out; };
    let stamp = |p: &std::path::Path| std::fs::metadata(p).ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    if dir.join("composition.json").is_file() {
        out.push(serde_json::json!({ "name": "main", "modified": stamp(&dir.join("composition.json")) }));
    }
    if let Ok(rd) = std::fs::read_dir(dir.join("sequences")) {
        let mut rest: Vec<serde_json::Value> = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() || p.extension().and_then(|x| x.to_str()) != Some("json") { continue; }
            let name = e.file_name().to_string_lossy().to_string();
            let name = name.strip_suffix(".json").unwrap_or(&name).to_string();
            if !slug_ok(&name) || name == "main" { continue; }
            rest.push(serde_json::json!({ "name": name, "modified": stamp(&p) }));
        }
        rest.sort_by_key(|v| -v["modified"].as_i64().unwrap_or(0));
        out.extend(rest);
    }
    out
}

// ---------------------------------------------------------------------------
// Assets manifest
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Thumbs {
    pub strip: String,       // rel to project: .cache/<id>.strip.jpg
    pub strip_frames: u32,   // frames tiled horizontally
    pub strip_step: f64,     // seconds per frame
    pub frame_w: u32,
    pub frame_h: u32,
    pub wave: String,        // rel to project: .cache/<id>.wave.png (empty if no audio)
    pub wave_w: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Asset {
    pub id: String,
    pub name: String,
    pub kind: String,            // video | audio | image
    pub rel: String,             // "media/<file>" when hardlinked inside the project
    pub path: String,            // absolute source path (always kept; used when rel is empty = reference)
    pub linked: bool,            // true = hardlink inside the jail; false = cross-volume reference
    pub size: u64,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub audio_channels: u32,
    pub codec: String,
    pub thumbs: Option<Thumbs>,
    pub imported: i64,
    #[serde(default)] pub folder: String, // media bin id ("" = unfiled)
}

/// A media bin (Premiere-style folder) in the Media panel. Pure organization:
/// clips reference assets by id, so moving assets between bins never breaks
/// the edit. Bins live in assets.json next to the assets they group.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct MediaFolder { pub id: String, pub name: String, pub created: i64, #[serde(default)] pub parent: String }

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Manifest { pub assets: Vec<Asset>, #[serde(default)] pub folders: Vec<MediaFolder> }

pub fn load_manifest(broker: &Broker, agent_id: &str, project: &str) -> Manifest {
    read_json(broker, agent_id, &rel(project, "assets.json"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub fn save_manifest(broker: &Broker, agent_id: &str, project: &str, m: &Manifest) -> Result<(), String> {
    write_json(broker, agent_id, &rel(project, "assets.json"), &serde_json::to_value(m).unwrap_or_default()).map(|_| ())
}

/// The absolute, readable path of an asset's media (hardlink inside the jail, or
/// the allow-listed reference). Fail-closed: a linked asset MUST resolve via broker.
pub fn asset_abs(broker: &Broker, agent_id: &str, project: &str, a: &Asset) -> Result<PathBuf, String> {
    if a.linked && !a.rel.is_empty() {
        broker.resolve(agent_id, &rel(project, &a.rel), broker::Mode::Read).map_err(|e| format!("refused by jail: {e:?}"))
    } else if !a.path.is_empty() {
        let p = PathBuf::from(&a.path);
        if p.is_file() { Ok(p) } else { Err(format!("referenced media missing: {}", a.path)) }
    } else {
        Err(format!("asset {} has no media path", a.id))
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("{prefix}{}{}", radix36(t as u64), radix36(n as u64 + 36))
}

fn radix36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 { return "0".into(); }
    let mut s = Vec::new();
    while n > 0 { s.push(D[(n % 36) as usize]); n /= 36; }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

fn kind_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "mts" | "m2ts" | "mxf") => "video",
        Some("mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" | "aif" | "aiff") => "audio",
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "heic" | "tif" | "tiff") => "image",
        _ => "video",
    }
}

pub fn media_ext_ok(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "mts" | "m2ts" | "mxf"
            | "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" | "aif" | "aiff"
            | "png" | "jpg" | "jpeg" | "webp" | "gif" | "heic" | "tif" | "tiff" | "cube"))
}

/// Sanitize a file name for the media/ dir: keep [A-Za-z0-9._-], collapse the rest.
fn safe_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { out.push(c); dash = false; }
        else if !dash { out.push('-'); dash = true; }
    }
    let t = out.trim_matches('-').to_string();
    if t.is_empty() || t.starts_with('.') { format!("media-{t}") } else { t }
}

// ---------------------------------------------------------------------------
// ffprobe / ffmpeg helpers
// ---------------------------------------------------------------------------

pub fn ffmpeg(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    crate::provision::ffmpeg_bin(app).ok_or_else(|| "ffmpeg is not provisioned — install HyperFrames in Tools (it ships ffmpeg)".to_string())
}
pub fn ffprobe(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    crate::provision::ffprobe_bin(app).ok_or_else(|| "ffprobe is not provisioned — install HyperFrames in Tools (it ships ffmpeg)".to_string())
}

fn parse_rate(s: &str) -> f64 {
    let mut it = s.split('/');
    let a: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
    let b: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(1.0);
    if b == 0.0 { a } else { a / b }
}

/// Probe a media file into the Asset fields (duration, streams, fps, size).
pub fn probe_into(app: &tauri::AppHandle, abs: &Path, a: &mut Asset) -> Result<(), String> {
    let out = Command::new(ffprobe(app)?)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(abs)
        .output().map_err(|e| format!("ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr).chars().take(300).collect::<String>()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe parse: {e}"))?;
    a.duration = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    a.size = v["format"]["size"].as_str().and_then(|d| d.parse().ok()).unwrap_or(a.size);
    a.has_video = false; a.has_audio = false;
    for s in v["streams"].as_array().cloned().unwrap_or_default() {
        match s["codec_type"].as_str() {
            Some("video") => {
                // attached pictures (album art) are not video
                if s["disposition"]["attached_pic"].as_i64() == Some(1) { continue; }
                a.has_video = true;
                a.width = s["width"].as_u64().unwrap_or(0) as u32;
                a.height = s["height"].as_u64().unwrap_or(0) as u32;
                // rotation metadata: swap dims for 90/270 so the UI fits correctly
                let rot = s["side_data_list"].as_array().and_then(|l| l.iter().find_map(|d| d["rotation"].as_f64())).unwrap_or(0.0);
                if (rot.abs() - 90.0).abs() < 1.0 || (rot.abs() - 270.0).abs() < 1.0 { std::mem::swap(&mut a.width, &mut a.height); }
                a.fps = s["avg_frame_rate"].as_str().map(parse_rate).filter(|f| *f > 0.0)
                    .or_else(|| s["r_frame_rate"].as_str().map(parse_rate)).unwrap_or(0.0);
                a.codec = s["codec_name"].as_str().unwrap_or("").to_string();
                if a.duration == 0.0 { a.duration = s["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0); }
            }
            Some("audio") => {
                a.has_audio = true;
                a.audio_channels = s["channels"].as_u64().unwrap_or(0) as u32;
                if a.duration == 0.0 { a.duration = s["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0); }
            }
            _ => {}
        }
    }
    if a.kind == "image" { a.has_video = true; a.has_audio = false; if a.duration == 0.0 { a.duration = 5.0; } }
    if a.kind == "video" && !a.has_video && a.has_audio { a.kind = "audio".into(); }
    Ok(())
}

/// Build filmstrip + waveform into .cache. Idempotent (skips existing files).
pub fn build_thumbs(app: &tauri::AppHandle, proj: &Path, abs: &Path, a: &Asset) -> Result<Thumbs, String> {
    let cache = proj.join(".cache");
    std::fs::create_dir_all(&cache).map_err(|e| format!("mkdir .cache: {e}"))?;
    let ff = ffmpeg(app)?;
    let mut t = Thumbs::default();
    let dur = a.duration.max(0.04);

    if a.has_video {
        // ≤ 90 frames per strip, 64px tall. Step grows with duration.
        let frames = (dur / 2.0).ceil().clamp(1.0, 90.0) as u32;
        let step = dur / frames as f64;
        let fh: u32 = 64;
        let fw: u32 = if a.width > 0 && a.height > 0 { ((a.width as f64 / a.height as f64) * fh as f64).round().max(16.0) as u32 } else { 114 };
        let fw = fw + (fw % 2); // even
        let strip = cache.join(format!("{}.strip.jpg", a.id));
        if !strip.is_file() {
            let vf = if a.kind == "image" {
                format!("scale={fw}:{fh}")
            } else {
                format!("fps=1/{step:.6},scale={fw}:{fh}:force_original_aspect_ratio=increase,crop={fw}:{fh},tile={frames}x1")
            };
            let mut cmd = Command::new(&ff);
            cmd.args(["-v", "error", "-y", "-i"]).arg(abs)
                .args(["-vf", &vf, "-frames:v", "1", "-q:v", "6", "-an"]).arg(&strip);
            let out = cmd.output().map_err(|e| format!("ffmpeg strip: {e}"))?;
            if !out.status.success() {
                return Err(format!("filmstrip failed: {}", String::from_utf8_lossy(&out.stderr).chars().take(300).collect::<String>()));
            }
        }
        t.strip = format!(".cache/{}.strip.jpg", a.id);
        t.strip_frames = if a.kind == "image" { 1 } else { frames };
        t.strip_step = if a.kind == "image" { dur } else { step };
        t.frame_w = fw; t.frame_h = fh;
    }
    if a.has_audio {
        let w = ((dur * 40.0).round() as u32).clamp(64, 8000);
        let wave = cache.join(format!("{}.wave.png", a.id));
        if !wave.is_file() {
            let fc = format!("[0:a]aformat=channel_layouts=mono,showwavespic=s={w}x64:colors=white:scale=sqrt[w]");
            let out = Command::new(&ff)
                .args(["-v", "error", "-y", "-i"]).arg(abs)
                .args(["-filter_complex", &fc, "-map", "[w]", "-frames:v", "1"]).arg(&wave)
                .output().map_err(|e| format!("ffmpeg wave: {e}"))?;
            if !out.status.success() {
                return Err(format!("waveform failed: {}", String::from_utf8_lossy(&out.stderr).chars().take(300).collect::<String>()));
            }
        }
        t.wave = format!(".cache/{}.wave.png", a.id);
        t.wave_w = w;
    }
    Ok(t)
}

// ---------------------------------------------------------------------------
// Import (hardlink only)
// ---------------------------------------------------------------------------

/// Import one source file into a project: hardlink into media/ (same volume) or
/// record a reference (other volume). Probes + builds thumbs. Returns the asset.
pub fn import_one(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, src: &Path) -> Result<Asset, String> {
    import_one_in(app, broker, agent_id, project, src, "")
}

/// import_one + bin assignment (folder import parks each file in its bin).
pub fn import_one_in(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, src: &Path, folder: &str) -> Result<Asset, String> {
    let proj = project_dir(broker, agent_id, project)?;
    let src = std::fs::canonicalize(src).map_err(|e| format!("source: {e}"))?;
    if !src.is_file() { return Err(format!("not a file: {}", src.display())); }
    if !media_ext_ok(&src) { return Err(format!("unsupported media type: {}", src.display())); }

    // Already imported (same absolute source)? Return it.
    let mut m = load_manifest(broker, agent_id, project);
    let src_s = src.to_string_lossy().to_string();
    if let Some(a) = m.assets.iter().find(|a| a.path == src_s) { return Ok(a.clone()); }

    let media_dir = proj.join("media");
    std::fs::create_dir_all(&media_dir).map_err(|e| format!("mkdir media: {e}"))?;
    ensure_gitignore(&proj);

    let base = safe_name(src.file_name().and_then(|n| n.to_str()).unwrap_or("media"));
    let mut name = base.clone();
    let mut n = 1;
    while media_dir.join(&name).exists() {
        n += 1;
        let (stem, ext) = match base.rfind('.') { Some(i) => (&base[..i], &base[i..]), None => (base.as_str(), "") };
        name = format!("{stem}-{n}{ext}");
    }
    let dst = media_dir.join(&name);
    // Refuse to link a file that's already inside the jail root? No — a file
    // inside the agent folder is fine to link too (same volume by definition).
    let linked = match std::fs::hard_link(&src, &dst) {
        Ok(()) => true,
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => false,
        Err(e) => return Err(format!("hardlink failed: {e}")),
    };

    let kind = kind_for(&src).to_string();
    let mut a = Asset {
        id: new_id("a"),
        name: src.file_name().and_then(|n| n.to_str()).unwrap_or(&name).to_string(),
        kind,
        rel: if linked { format!("media/{name}") } else { String::new() },
        path: src_s,
        linked,
        size: std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0),
        imported: now(),
        folder: folder.to_string(),
        ..Default::default()
    };
    let abs = if linked { dst.clone() } else { src.clone() };
    probe_into(app, &abs, &mut a)?;
    a.thumbs = build_thumbs(app, &proj, &abs, &a).ok();
    m.assets.push(a.clone());
    save_manifest(broker, agent_id, project, &m)?;
    Ok(a)
}

fn ensure_gitignore(proj: &Path) {
    let gi = proj.join(".gitignore");
    if !gi.exists() { let _ = std::fs::write(gi, "media/\n.cache/\nrenders/\n"); }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn video_status(app: tauri::AppHandle, folder: Option<String>) -> Result<serde_json::Value, String> {
    let ffmpeg = crate::provision::ffmpeg_bin(&app).map(|p| p.to_string_lossy().to_string());
    let hyperframes = crate::provision::hyperframes_installed(&app);
    let whisper = crate::keychain::has_key("openai");
    let uv = crate::provision::uv_bin(&app).is_some();
    let pro = folder.as_deref().map(|f| crate::allow_shell_access_enabled_pub(&app, f)).unwrap_or(false);
    Ok(serde_json::json!({ "ffmpeg": ffmpeg, "hyperframes": hyperframes, "whisper": whisper, "uv": uv, "allowShellAccess": pro }))
}

#[tauri::command]
pub fn video_projects(broker: tauri::State<Arc<Broker>>, agent_id: String) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    let dir = match broker.resolve(&agent_id, VIDEO_ROOT, broker::Mode::Read) {
        Ok(p) => p,
        Err(e) if is_missing(&e) => return Ok(serde_json::json!([])),
        Err(e) => return Err(format!("refused by jail: {e:?}")),
    };
    let mut out: Vec<serde_json::Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() { continue; }
            let name = e.file_name().to_string_lossy().to_string();
            if !slug_ok(&name) || !p.join("composition.json").is_file() { continue; }
            let modified = std::fs::metadata(p.join("composition.json")).ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            let assets = load_manifest(&broker, &agent_id, &name).assets.len();
            out.push(serde_json::json!({ "name": name, "modified": modified, "assets": assets }));
        }
    }
    out.sort_by_key(|a| -a["modified"].as_i64().unwrap_or(0));
    Ok(serde_json::json!(out))
}

#[tauri::command]
pub fn video_create(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, composition: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(&project) { return Err("invalid project name — use letters, digits, - _".into()); }
    let comp = composition.filter(|c| c.is_object()).unwrap_or_else(crate::video_render::blank_composition_json);
    let abs = write_json(&broker, &agent_id, &rel(&project, "composition.json"), &comp)?;
    if let Some(p) = abs.parent() { ensure_gitignore(p); let _ = std::fs::create_dir_all(p.join("media")); let _ = std::fs::create_dir_all(p.join("sequences")); }
    Ok(serde_json::json!({ "project": project, "composition": comp }))
}

#[tauri::command]
pub fn video_load(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, sequence: Option<String>) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(&project) { return Err("invalid project name".into()); }
    let seq = sequence.unwrap_or_default();
    if !seq_ok(&seq) { return Err("invalid sequence name — use letters, digits, - _".into()); }
    let composition = read_json(&broker, &agent_id, &seq_rel(&project, &seq))
        .ok_or_else(|| if seq.is_empty() || seq == "main" { format!("no such project: {project}") } else { format!("no such sequence: {seq}") })?;
    let assets = assets_with_status(&broker, &agent_id, &project);
    let transcript = read_json(&broker, &agent_id, &rel(&project, "transcript.json")).unwrap_or(serde_json::Value::Null);
    let chat = read_json(&broker, &agent_id, &rel(&project, "chat.json")).unwrap_or(serde_json::Value::Null);
    let folders = serde_json::to_value(load_manifest(&broker, &agent_id, &project).folders).unwrap_or_default();
    let sequences = list_sequences(&broker, &agent_id, &project);
    let seq_name = if seq.is_empty() { "main".to_string() } else { seq };
    Ok(serde_json::json!({ "project": project, "sequence": seq_name, "sequences": sequences, "composition": composition, "assets": assets, "transcript": transcript, "chat": chat, "folders": folders }))
}

#[tauri::command]
pub fn video_save(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, composition: serde_json::Value, sequence: Option<String>) -> Result<(), String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(&project) { return Err("invalid project name — use letters, digits, - _".into()); }
    let seq = sequence.unwrap_or_default();
    if !seq_ok(&seq) { return Err("invalid sequence name".into()); }
    if !composition.is_object() { return Err("composition must be a JSON object".into()); }
    let abs = write_json(&broker, &agent_id, &seq_rel(&project, &seq), &composition)?;
    if let Some(p) = abs.parent() { ensure_gitignore(&project_root_fallback(p)); }
    Ok(())
}

/// Sequences share one project dir: media, transcript, bins, chat.
/// `parent` of a written sequences/<name>.json is sequences/ — gitignore + dir
/// setup belong on the PROJECT root, not the sequences subdir.
fn project_root_fallback(p: &std::path::Path) -> std::path::PathBuf {
    if p.file_name().and_then(|n| n.to_str()) == Some("sequences") {
        p.parent().map(|x| x.to_path_buf()).unwrap_or_else(|| p.to_path_buf())
    } else { p.to_path_buf() }
}

/// Sequence CRUD: create (from blank or duplicated), rename, delete.
/// Media, transcript, bins, chat are project-level and shared — only the
/// composition file is per-sequence. Deleting the last sequence is refused
/// (a project always has at least `main`).
#[tauri::command]
pub fn video_create_sequence(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, name: String, from: Option<String>) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(&project) { return Err("invalid project name".into()); }
    let name = name.trim().to_string();
    if name == "main" || !slug_ok(&name) { return Err("invalid sequence name — use letters, digits, - _ (not 'main')".into()); }
    let from = from.unwrap_or_default();
    if !seq_ok(&from) { return Err("invalid source sequence".into()); }
    project_dir(&broker, &agent_id, &project)?;
    if read_json(&broker, &agent_id, &seq_rel(&project, &name)).is_some() { return Err(format!("sequence '{name}' already exists")); }
    let comp = if from.is_empty() { crate::video_render::blank_composition_json() }
    else { read_json(&broker, &agent_id, &seq_rel(&project, &from)).ok_or_else(|| format!("no such sequence: {from}"))? };
    if !comp.is_object() { return Err("source sequence is corrupt".into()); }
    write_json(&broker, &agent_id, &seq_rel(&project, &name), &comp)?;
    Ok(serde_json::json!({ "name": name, "sequences": list_sequences(&broker, &agent_id, &project) }))
}

#[tauri::command]
pub fn video_rename_sequence(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, from: String, to: String) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(&project) { return Err("invalid project name".into()); }
    if from == "main" || from.is_empty() { return Err("the main sequence cannot be renamed".into()); }
    if to == "main" || !slug_ok(&to) { return Err("invalid sequence name".into()); }
    project_dir(&broker, &agent_id, &project)?;
    let src = seq_rel(&project, &from);
    let comp = read_json(&broker, &agent_id, &src).ok_or_else(|| format!("no such sequence: {from}"))?;
    if read_json(&broker, &agent_id, &seq_rel(&project, &to)).is_some() { return Err(format!("sequence '{to}' already exists")); }
    write_json(&broker, &agent_id, &seq_rel(&project, &to), &comp)?;
    // delete the old file directly (it is jail-relative + slug-checked)
    let abs = broker.resolve(&agent_id, &src, broker::Mode::Write).map_err(|e| format!("refused by jail: {e:?}"))?;
    let _ = std::fs::remove_file(abs);
    Ok(serde_json::json!({ "name": to, "sequences": list_sequences(&broker, &agent_id, &project) }))
}

#[tauri::command]
pub fn video_delete_sequence(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, name: String) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(&project) { return Err("invalid project name".into()); }
    if name == "main" || name.is_empty() { return Err("the main sequence cannot be deleted".into()); }
    if !slug_ok(&name) { return Err("invalid sequence name".into()); }
    project_dir(&broker, &agent_id, &project)?;
    let target = seq_rel(&project, &name);
    if read_json(&broker, &agent_id, &target).is_none() { return Err(format!("no such sequence: {name}")); }
    let abs = broker.resolve(&agent_id, &target, broker::Mode::Write).map_err(|e| format!("refused by jail: {e:?}"))?;
    std::fs::remove_file(&abs).map_err(|e| format!("delete failed: {e}"))?;
    Ok(serde_json::json!({ "sequences": list_sequences(&broker, &agent_id, &project) }))
}

#[tauri::command]
pub fn video_chat_save(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, chat: serde_json::Value) -> Result<(), String> {
    if !chat.is_object() { return Err("chat must be an object".into()); }
    project_dir(&broker, &agent_id, &project)?;
    write_json(&broker, &agent_id, &rel(&project, "chat.json"), &chat).map(|_| ())
}

/// Native multi-file picker → hardlink import. Async + oneshot like pick_agent_folder
/// (a blocking pick on the main thread deadlocks).
#[tauri::command]
pub async fn video_pick_media(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String) -> Result<serde_json::Value, String> {
    use tauri_plugin_dialog::DialogExt;
    project_dir(&broker, &agent_id, &project)?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<Vec<tauri_plugin_dialog::FilePath>>>();
    app.dialog().file()
        .add_filter("Media", &["mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", "m2ts", "mxf", "mp3", "wav", "m4a", "aac", "flac", "ogg", "aif", "aiff", "png", "jpg", "jpeg", "webp", "gif", "heic", "tif", "tiff", "cube"])
        .set_title("Import media (hardlinked, not copied)")
        .pick_files(move |chosen| { let _ = tx.send(chosen); });
    let chosen = rx.await.map_err(|_| "picker closed".to_string())?;
    let Some(files) = chosen else { return Ok(serde_json::json!({ "assets": [], "errors": [] })) };
    let paths: Vec<String> = files.into_iter().filter_map(|f| f.into_path().ok()).map(|p| p.to_string_lossy().to_string()).collect();
    import_paths_impl(app, broker.inner().clone(), agent_id, project, paths, String::new()).await
}

/// Import absolute paths (drag-drop from Finder lands here). `folder` parks
/// loose files in that bin (drop-onto-bin); directories always become their
/// own nested bin tree regardless.
#[tauri::command]
pub async fn video_import_paths(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, paths: Vec<String>, folder: Option<String>) -> Result<serde_json::Value, String> {
    import_paths_impl(app, broker.inner().clone(), agent_id, project, paths, folder.unwrap_or_default()).await
}

/// Native folder picker → import the whole folder as a bin, subfolders as
/// nested bins (Premiere-style). `parent` nests the new tree under that bin.
#[tauri::command]
pub async fn video_pick_folder(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, parent: Option<String>) -> Result<serde_json::Value, String> {
    use tauri_plugin_dialog::DialogExt;
    project_dir(&broker, &agent_id, &project)?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog().file()
        .set_title("Import folder as bins (subfolders nest)")
        .pick_folder(move |chosen| { let _ = tx.send(chosen); });
    let chosen = rx.await.map_err(|_| "picker closed".to_string())?;
    let Some(dir) = chosen else { return Ok(serde_json::json!({ "assets": [], "errors": [] })) };
    let path = dir.into_path().map_err(|e| e.to_string())?.to_string_lossy().to_string();
    import_paths_impl(app, broker.inner().clone(), agent_id, project, vec![path], parent.unwrap_or_default()).await
}

/// One enumerated import item: a file plus the bin it belongs in ("" = unfiled).
struct ImportItem { path: PathBuf, display: String, folder: String }

/// Walk a directory into nested bins mirroring its structure. Returns the
/// collected files. Hidden entries skipped, symlink cycles guarded, depth
/// capped — a media folder, not a filesystem browser.
fn walk_dir(dir: &Path, bin_id: &str, folders: &mut Vec<MediaFolder>, out: &mut Vec<ImportItem>, depth: u8, seen: &mut Vec<PathBuf>, truncated: &mut bool) {
    if depth > 8 || out.len() >= 200 { if out.len() >= 200 { *truncated = true; } return; }
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if seen.contains(&canon) { return; }
    seen.push(canon);
    let mut entries: Vec<_> = std::fs::read_dir(dir).map(|r| r.flatten().collect()).unwrap_or_default();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') { continue; }
        let p = e.path();
        if out.len() >= 200 { *truncated = true; return; }
        if p.is_dir() {
            let sub = MediaFolder { id: new_id("f"), name: name.chars().take(80).collect(), created: now(), parent: bin_id.to_string() };
            let sid = sub.id.clone();
            folders.push(sub);
            walk_dir(&p, &sid, folders, out, depth + 1, seen, truncated);
        } else if p.is_file() {
            out.push(ImportItem { path: p.clone(), display: p.to_string_lossy().to_string(), folder: bin_id.to_string() });
        }
    }
}

fn emit_progress(app: &tauri::AppHandle, project: &str, done: usize, total: usize, name: &str) {
    let _ = app.emit("video-import-progress", serde_json::json!({ "project": project, "done": done, "total": total, "name": name }));
}

async fn import_paths_impl(app: tauri::AppHandle, broker: Arc<Broker>, agent_id: String, project: String, paths: Vec<String>, folder: String) -> Result<serde_json::Value, String> {
    if paths.len() > 200 { return Err("too many files at once (max 200)".into()); }
    project_dir(&broker, &agent_id, &project)?;
    // Validate the target bin once, up front.
    if !folder.is_empty() {
        let m = load_manifest(&broker, &agent_id, &project);
        if !m.folders.iter().any(|f| f.id == folder) { return Err("no such folder".into()); }
    }
    tokio::task::spawn_blocking(move || {
        // Phase 1 (fast): enumerate everything, building nested bins for dirs.
        let mut m = load_manifest(&broker, &agent_id, &project);
        let mut items: Vec<ImportItem> = Vec::new();
        let mut errors: Vec<serde_json::Value> = Vec::new();
        let mut truncated = false;
        let mut seen: Vec<PathBuf> = Vec::new();
        for p in &paths {
            let path = PathBuf::from(p);
            if path.is_dir() {
                let bin_name: String = path.file_name().and_then(|n| n.to_str()).unwrap_or("Folder").chars().take(80).collect();
                let bin = MediaFolder { id: new_id("f"), name: if bin_name.is_empty() { "Folder".into() } else { bin_name }, created: now(), parent: folder.clone() };
                let bid = bin.id.clone();
                m.folders.push(bin);
                walk_dir(&path, &bid, &mut m.folders, &mut items, 0, &mut seen, &mut truncated);
                if items.is_empty() && !truncated { errors.push(serde_json::json!({ "path": p, "error": "folder has no importable files" })); }
            } else {
                items.push(ImportItem { path: path.clone(), display: p.clone(), folder: folder.clone() });
            }
            if items.len() >= 200 { truncated = true; break; }
        }
        if truncated { errors.push(serde_json::json!({ "path": "", "error": "capped at 200 files — import the rest separately" })); }
        if !items.is_empty() || m.folders.iter().any(|_| true) { save_manifest(&broker, &agent_id, &project, &m)?; }
        let total = items.len();
        // Phase 2 (slow): probe + thumbs, one progress event per file so the
        // UI never looks hung on a 20-clip import.
        let mut assets = Vec::new();
        for (i, it) in items.into_iter().enumerate() {
            let short = it.path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
            emit_progress(&app, &project, i, total, &short);
            if it.path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("cube")).unwrap_or(false) {
                match import_lut(&broker, &agent_id, &project, &it.path) {
                    Ok(rel) => assets.push(serde_json::json!({ "lut": rel })),
                    Err(e) => errors.push(serde_json::json!({ "path": it.display, "error": e })),
                }
                continue;
            }
            match import_one_in(&app, &broker, &agent_id, &project, &it.path, &it.folder) {
                Ok(a) => assets.push(serde_json::to_value(a).unwrap_or_default()),
                Err(e) => errors.push(serde_json::json!({ "path": it.display, "error": e })),
            }
        }
        emit_progress(&app, &project, total, total, "");
        let _ = app.emit("video-assets-changed", serde_json::json!({ "project": project }));
        Ok(serde_json::json!({ "assets": assets, "errors": errors, "folders": serde_json::to_value(load_manifest(&broker, &agent_id, &project).folders).unwrap_or_default() }))
    }).await.map_err(|e| format!("import task: {e}"))?
}

/// Hardlink a .cube LUT into Video/<project>/luts/ and return its project-relative path.
pub fn import_lut(broker: &Broker, agent_id: &str, project: &str, src: &Path) -> Result<String, String> {
    let proj = project_dir(broker, agent_id, project)?;
    let src = std::fs::canonicalize(src).map_err(|e| format!("lut: {e}"))?;
    let dir = proj.join("luts");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir luts: {e}"))?;
    let name = safe_name(src.file_name().and_then(|n| n.to_str()).unwrap_or("lut.cube"));
    let dst = dir.join(&name);
    if !dst.exists() {
        if let Err(e) = std::fs::hard_link(&src, &dst) {
            if e.raw_os_error() == Some(libc::EXDEV) { std::fs::copy(&src, &dst).map_err(|e| format!("lut copy: {e}"))?; } // LUTs are tiny; a copy is fine here
            else { return Err(format!("lut link: {e}")); }
        }
    }
    Ok(format!("luts/{name}"))
}

/// Manifest rows + a computed `online` flag (is the media readable right now?).
/// A cross-volume reference goes offline whenever its drive is unmounted; the
/// UI shows the stored `path` so the user knows which drive to plug in.
pub fn assets_with_status(broker: &Broker, agent_id: &str, project: &str) -> serde_json::Value {
    let m = load_manifest(broker, agent_id, project);
    serde_json::Value::Array(m.assets.iter().map(|a| {
        let mut v = serde_json::to_value(a).unwrap_or_default();
        let online = asset_abs(broker, agent_id, project, a).map(|p| p.is_file()).unwrap_or(false);
        if let Some(o) = v.as_object_mut() { o.insert("online".into(), serde_json::Value::Bool(online)); }
        v
    }).collect())
}

/// Relink an (offline) asset to a file the user picks. Same volume → hardlink into
/// media/ (linked=true); other volume → reference (linked=false). Asset id, clips
/// and cached thumbs are untouched, so the edit keeps working.
#[tauri::command]
pub async fn video_relink_asset(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, asset_id: String) -> Result<serde_json::Value, String> {
    use tauri_plugin_dialog::DialogExt;
    let proj = project_dir(&broker, &agent_id, &project)?;
    let mut m = load_manifest(&broker, &agent_id, &project);
    let Some(i) = m.assets.iter().position(|a| a.id == asset_id) else { return Err("no such asset".into()) };
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog().file().set_title(format!("Relink {}", m.assets[i].name)).pick_file(move |chosen| { let _ = tx.send(chosen); });
    let Some(chosen) = rx.await.map_err(|_| "picker closed".to_string())? else { return Ok(serde_json::json!({ "relinked": false })) };
    let src = chosen.into_path().map_err(|e| e.to_string())?;
    if !src.is_file() { return Err("not a file".into()); }
    let a = &mut m.assets[i];
    // drop the old hardlink (if any) before linking the new file under the same media/ name
    if a.linked && !a.rel.is_empty() { if let Ok(p) = broker.resolve(&agent_id, &rel(&project, &a.rel), broker::Mode::Read) { let _ = std::fs::remove_file(p); } }
    let media = proj.join("media"); std::fs::create_dir_all(&media).map_err(|e| format!("mkdir media: {e}"))?;
    let fname = format!("{}-{}", a.id, src.file_name().and_then(|n| n.to_str()).unwrap_or("media"));
    let dst = media.join(&fname);
    let _ = std::fs::remove_file(&dst);
    let linked = match std::fs::hard_link(&src, &dst) { Ok(()) => true, Err(e) if e.raw_os_error() == Some(libc::EXDEV) => false, Err(e) => return Err(format!("hardlink failed: {e}")) };
    a.path = src.to_string_lossy().to_string();
    a.linked = linked;
    a.rel = if linked { format!("media/{fname}") } else { String::new() };
    a.name = src.file_name().and_then(|n| n.to_str()).unwrap_or(&a.name).to_string();
    a.size = std::fs::metadata(&src).map(|md| md.len()).unwrap_or(a.size);
    save_manifest(&broker, &agent_id, &project, &m)?;
    let _ = app.emit("video-assets-changed", serde_json::json!({ "project": project }));
    Ok(serde_json::json!({ "relinked": true, "linked": linked, "path": src.to_string_lossy() }))
}

/// Remove an asset from the manifest (+ its hardlink and cache). Clips that
/// reference it are left to the UI/agent to clean.
#[tauri::command]
pub fn video_remove_asset(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, asset_id: String) -> Result<(), String> {
    let proj = project_dir(&broker, &agent_id, &project)?;
    let mut m = load_manifest(&broker, &agent_id, &project);
    let Some(i) = m.assets.iter().position(|a| a.id == asset_id) else { return Err("no such asset".into()) };
    let a = m.assets.remove(i);
    if a.linked && !a.rel.is_empty() {
        if let Ok(p) = broker.resolve(&agent_id, &rel(&project, &a.rel), broker::Mode::Read) { let _ = std::fs::remove_file(p); }
    }
    let _ = std::fs::remove_file(proj.join(format!(".cache/{}.strip.jpg", a.id)));
    let _ = std::fs::remove_file(proj.join(format!(".cache/{}.wave.png", a.id)));
    save_manifest(&broker, &agent_id, &project, &m)
}

/// ---- MEDIA BINS (Premiere-style folders) ------------------------------------
/// Pure organization over assets.json: create/rename/delete bins, move assets
/// between them. Clips reference assets by id, so bins never break the edit.
/// Deleting a bin just unfiles its assets (folder = "").

fn folder_ok(name: &str) -> Result<String, String> {
    let n = name.trim().to_string();
    if n.is_empty() { return Err("folder name is empty".into()); }
    if n.len() > 80 { return Err("folder name too long (max 80)".into()); }
    Ok(n)
}

#[tauri::command]
pub fn video_create_media_folder(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, name: String, parent: Option<String>) -> Result<serde_json::Value, String> {
    project_dir(&broker, &agent_id, &project)?;
    let mut m = load_manifest(&broker, &agent_id, &project);
    let parent = parent.unwrap_or_default();
    if !parent.is_empty() && !m.folders.iter().any(|f| f.id == parent) { return Err("no such parent bin".into()); }
    let f = MediaFolder { id: new_id("f"), name: folder_ok(&name)?, created: now(), parent: parent.clone() };
    m.folders.push(f.clone());
    save_manifest(&broker, &agent_id, &project, &m)?;
    Ok(serde_json::json!({ "id": f.id, "name": f.name, "parent": f.parent }))
}

#[tauri::command]
pub fn video_rename_media_folder(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, folder_id: String, name: String) -> Result<(), String> {
    project_dir(&broker, &agent_id, &project)?;
    let mut m = load_manifest(&broker, &agent_id, &project);
    let Some(f) = m.folders.iter_mut().find(|f| f.id == folder_id) else { return Err("no such folder".into()) };
    f.name = folder_ok(&name)?;
    save_manifest(&broker, &agent_id, &project, &m)
}

#[tauri::command]
pub fn video_delete_media_folder(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, folder_id: String) -> Result<(), String> {
    project_dir(&broker, &agent_id, &project)?;
    let mut m = load_manifest(&broker, &agent_id, &project);
    let Some(dead) = m.folders.iter().find(|f| f.id == folder_id).cloned() else { return Err("no such folder".into()); };
    m.folders.retain(|f| f.id != folder_id);
    for f in m.folders.iter_mut().filter(|f| f.parent == folder_id) { f.parent = dead.parent.clone(); }
    for a in m.assets.iter_mut().filter(|a| a.folder == folder_id) { a.folder = dead.parent.clone(); }
    save_manifest(&broker, &agent_id, &project, &m)
}

/// Move a bin under another bin (Premiere-style nesting). "" = top level.
/// Refuses cycles (a bin can never become its own descendant).
#[tauri::command]
pub fn video_move_media_folder(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, folder_id: String, parent: String) -> Result<(), String> {
    project_dir(&broker, &agent_id, &project)?;
    let mut m = load_manifest(&broker, &agent_id, &project);
    if !m.folders.iter().any(|f| f.id == folder_id) { return Err("no such folder".into()); }
    if !parent.is_empty() && !m.folders.iter().any(|f| f.id == parent) { return Err("no such parent bin".into()); }
    if parent == folder_id { return Err("a bin cannot contain itself".into()); }
    // cycle guard: walk up from the proposed parent; folder_id must not appear
    let mut cursor = parent.clone();
    while !cursor.is_empty() {
        if cursor == folder_id { return Err("a bin cannot move inside its own sub-bin".into()); }
        cursor = m.folders.iter().find(|f| f.id == cursor).map(|f| f.parent.clone()).unwrap_or_default();
    }
    if let Some(f) = m.folders.iter_mut().find(|f| f.id == folder_id) { f.parent = parent; }
    save_manifest(&broker, &agent_id, &project, &m)
}

#[tauri::command]
pub fn video_move_media_assets(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, asset_ids: Vec<String>, folder_id: String) -> Result<(), String> {
    project_dir(&broker, &agent_id, &project)?;
    let mut m = load_manifest(&broker, &agent_id, &project);
    if !folder_id.is_empty() && !m.folders.iter().any(|f| f.id == folder_id) { return Err("no such folder".into()); }
    if asset_ids.is_empty() { return Err("no assets".into()); }
    let mut n = 0;
    for a in m.assets.iter_mut().filter(|a| asset_ids.contains(&a.id)) { a.folder = folder_id.clone(); n += 1; }
    if n == 0 { return Err("no such assets".into()); }
    save_manifest(&broker, &agent_id, &project, &m)
}

/// (Re)build thumbs for assets missing them — e.g. after a crash mid-import.
#[tauri::command]
pub async fn video_refresh_thumbs(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String) -> Result<serde_json::Value, String> {
    let broker = broker.inner().clone();
    tokio::task::spawn_blocking(move || {
        let proj = project_dir(&broker, &agent_id, &project)?;
        let mut m = load_manifest(&broker, &agent_id, &project);
        let mut changed = false;
        for a in m.assets.iter_mut() {
            let need = match &a.thumbs { None => true, Some(t) => (a.has_video && !proj.join(&t.strip).is_file()) || (a.has_audio && !proj.join(&t.wave).is_file()) };
            if !need { continue; }
            if let Ok(abs) = asset_abs(&broker, &agent_id, &project, a) {
                if a.duration == 0.0 { let _ = probe_into(&app, &abs, a); }
                if let Ok(t) = build_thumbs(&app, &proj, &abs, a) { a.thumbs = Some(t); changed = true; }
            }
        }
        if changed { save_manifest(&broker, &agent_id, &project, &m)?; }
        Ok(serde_json::to_value(m.assets).unwrap_or_default())
    }).await.map_err(|e| format!("thumbs task: {e}"))?
}

/// List .cube LUTs available to a project (Video/<project>/luts + agent-root luts/).
#[tauri::command]
pub fn video_list_luts(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String) -> Result<Vec<String>, String> {
    let proj = project_dir(&broker, &agent_id, &project)?;
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(proj.join("luts")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.to_ascii_lowercase().ends_with(".cube") { out.push(format!("luts/{n}")); }
        }
    }
    out.sort();
    Ok(out)
}

/// Pick a .cube from anywhere and link it into the project.
#[tauri::command]
pub async fn video_pick_lut(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    project_dir(&broker, &agent_id, &project)?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog().file().add_filter("LUT", &["cube"]).set_title("Pick a .cube LUT").pick_file(move |c| { let _ = tx.send(c); });
    let Some(f) = rx.await.map_err(|_| "picker closed".to_string())? else { return Ok(None) };
    let p = f.into_path().map_err(|e| e.to_string())?;
    import_lut(&broker, &agent_id, &project, &p).map(Some)
}

/// Delete a whole project (Video/<project>/). The media inside is hardlinks +
/// renders + cache, so removing the folder frees project disk without touching
/// the original footage. Refuses when the name is invalid or missing.
#[tauri::command]
pub fn video_delete_project(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String) -> Result<(), String> {
    if agent_id.trim().is_empty() { return Err("select an agent first".into()); }
    if !slug_ok(&project) { return Err("invalid project name".into()); }
    let dir = project_dir(&broker, &agent_id, &project)?;
    // Paranoia: only delete a real project dir (has composition.json).
    if !dir.join("composition.json").is_file() { return Err(format!("not a video project: {project}")); }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("delete failed: {e}"))?;
    Ok(())
}

/// Reveal a project file/folder in Finder (render outputs, the project dir).
#[tauri::command]
pub fn video_reveal(broker: tauri::State<Arc<Broker>>, agent_id: String, project: String, tail: Option<String>) -> Result<(), String> {
    let proj = project_dir(&broker, &agent_id, &project)?;
    let p = match tail { Some(t) if !t.is_empty() && !t.contains("..") => proj.join(t), _ => proj };
    if !p.exists() { return Err("path does not exist".into()); }
    #[cfg(target_os = "macos")]
    { Command::new("open").arg("-R").arg(&p).spawn().map_err(|e| e.to_string())?; }
    #[cfg(not(target_os = "macos"))]
    { let _ = p; }
    Ok(())
}
