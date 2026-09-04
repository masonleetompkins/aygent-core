// AYGENT — VIDEO: Hyperframes overlay builds (graphics + captions).
//
// Graphics and captions are TRANSPARENT Hyperframes compositions, rendered with
// the provisioned hyperframes CLI (provision.rs) and registered back into the
// project as ordinary video assets + timeline clips:
//   captions → one T1 overlay clip (silent while captions.enabled is false)
//   graphics → V3 overlay clips (one per approved graphic)
// The ffmpeg graph composites them like any other video layer — there is no
// ASS/drawtext caption path anymore.
//
// Toolchain: the SAME provisioned runtime the HyperFrames skill uses:
//   <app_data>/runtime/{node/bin, ffmpeg, hyperframes/node_modules/.bin/hyperframes}
// Nothing the agent names is executed: only these three binaries with fixed args.
// The composition HTML comes from templates/hyperframes/*.html baked into the
// binary (include_str!), with {{SLOTS}} the agent's style text never touches as
// code — it is JSON-embedded into the page body + HTML-comment annotation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::broker::Broker;
use crate::video::{self, Asset};
use crate::video_render::{timeline_words, Clip as RClip, Composition, Transcript, Word};

const HYP_COMP: &str = include_str!("../templates/hyperframes/captions.html");

/// Provisioned toolchain: node/bin + ffmpeg on PATH, hyperframes CLI path.
fn toolchain(app: &tauri::AppHandle) -> Result<(String, PathBuf), String> {
    let node_bin = crate::provision::node_bin(app).ok_or("Hyperframes isn't enabled — turn it on in Tools first")?;
    let node_dir = node_bin.parent().map(Path::to_path_buf).unwrap_or_default();
    let ff_dir = crate::provision::ffmpeg_bin(app).ok_or("ffmpeg is not provisioned — enable HyperFrames in Tools")?
        .parent().map(Path::to_path_buf).unwrap_or_default();
    let (path_env, hf_bin) = crate::provision::hyperframes_invocation(app)
        .ok_or("Hyperframes CLI not found — re-enable Hyperframes in Tools")?;
    let _ = (node_dir, ff_dir);
    Ok((path_env, PathBuf::from(hf_bin)))
}

fn emit(app: &tauri::AppHandle, project: &str, tool: &str, msg: &str) {
    let _ = app.emit("video-tool-progress", json!({ "project": project, "tool": tool, "msg": msg }));
}

fn hf_run(path_env: &str, hf_bin: &Path, args: &[&str], workdir: &Path) -> Result<String, String> {
    let sh = format!(
        "export PATH='{path_env}':$PATH && '{}' {}",
        hf_bin.to_string_lossy().replace('\'', "'\\''"),
        args.iter().map(|a| format!("'{a}'")).collect::<Vec<_>>().join(" ")
    );
    let o = Command::new("bash").arg("-lc").arg(&sh).current_dir(workdir)
        .env("PATH", format!("{path_env}:/usr/bin:/bin"))
        .env("HYPERFRAMES_SKIP_SKILLS", "1")
        .output().map_err(|e| format!("hyperframes spawn: {e}"))?;
    let out = String::from_utf8_lossy(&o.stdout).to_string();
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        let tail: String = format!("{out}\n{err}").lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(format!("hyperframes failed: {tail}"));
    }
    Ok(out)
}

fn lint_comp(path_env: &str, hf_bin: &Path, comp: &Path, workdir: &Path) -> Result<(), String> {
    // `hyperframes lint/render [DIR]` take the project dir positionally and
    // always read index.html — so each overlay is staged as its OWN dir with
    // the composition written as index.html (see stage_comp below).
    let name = comp.file_name().and_then(|n| n.to_str()).unwrap_or("index.html");
    match hf_run(path_env, hf_bin, &["lint", "."], workdir) {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("overlay lint failed ({name}): {e}")),
    }
}

fn load_comp(broker: &Broker, agent_id: &str, project: &str) -> Result<Composition, String> {
    let raw = video::read_json(broker, agent_id, &video::rel(project, "composition.json")).ok_or_else(|| format!("no such project: {project}"))?;
    let assets = video::load_manifest(broker, agent_id, project).assets;
    crate::video_render::parse_composition(&raw, &assets)
}
fn save_comp(broker: &Broker, agent_id: &str, project: &str, comp: &Composition) -> Result<(), String> {
    video::write_json(broker, agent_id, &video::rel(project, "composition.json"), &serde_json::to_value(comp).map_err(|e| e.to_string())?).map(|_| ())
}
fn load_transcript(proj: &Path) -> Option<Transcript> {
    serde_json::from_str(&std::fs::read_to_string(proj.join("transcript.json")).ok()?).ok()
}

fn slug(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let t = t.split('-').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("-");
    t.chars().take(60).collect()
}

#[derive(Serialize, Deserialize, Clone)]
struct CapLine { s: f64, e: f64, words: Vec<CapWord> }
#[derive(Serialize, Deserialize, Clone)]
struct CapWord { w: String, key: bool, t: f64, i: usize }

/// 4 words/line, ≤ 24 chars; sentence/pause breaks. Same grouping the old ASS
/// path used — the look changed, the timing logic didn't.
fn caption_lines(words: &[Word], key_words: &[String]) -> Vec<CapLine> {
    let mut lines: Vec<Vec<Word>> = vec![];
    let mut cur: Vec<Word> = vec![];
    let mut chars = 0usize;
    for w in words {
        let gap = cur.last().map(|l| w.s - l.e).unwrap_or(0.0);
        let ends = cur.last().map(|l| l.w.ends_with(['.', '?', '!'])).unwrap_or(false);
        if !cur.is_empty() && (cur.len() >= 4 || chars + w.w.len() + 1 > 24 || gap > 0.7 || ends) {
            lines.push(std::mem::take(&mut cur)); chars = 0;
        }
        chars += w.w.len() + 1;
        cur.push(w.clone());
    }
    if !cur.is_empty() { lines.push(cur); }
    lines.iter().enumerate().map(|(idx, l)| {
        let s = l[0].s;
        let raw_e = l.last().map(|x| x.e).unwrap_or(s) + 0.6;
        // Consecutive captions share one screen slot: an end past the next
        // start co-shows two lines. Clamp to the next start (the floor keeps
        // the 0.5s word stagger readable on dense lines).
        let next_s = lines.get(idx + 1).map(|n| n[0].s).unwrap_or(f64::INFINITY);
        let e = raw_e.min(next_s - 0.04).max(s + 0.35);
        let stagger = if l.len() > 1 { 0.5 / (l.len() - 1) as f64 } else { 0.0 };
        CapLine {
            s, e,
            words: l.iter().enumerate().map(|(i, w)| {
                let clean: String = w.w.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                CapWord { w: w.w.clone(), key: key_words.iter().any(|k| k == &clean), t: (i as f64 * stagger * 1000.0).round() / 1000.0, i }
            }).collect(),
        }
    }).collect()
}

fn style_note(comp: &Composition) -> String {
    let mut parts: Vec<String> = vec![];
    if !comp.graphics.instructions.trim().is_empty() { parts.push(comp.graphics.instructions.trim().to_string()); }
    if !comp.graphics.style_guide.trim().is_empty() {
        let name = if comp.graphics.style_guide_name.is_empty() { "style guide".to_string() } else { comp.graphics.style_guide_name.clone() };
        parts.push(format!("[{name}]\n{}", comp.graphics.style_guide.trim()));
    }
    // One line, safe inside an HTML comment.
    parts.join(" / ").replace("--", "—").chars().take(500).collect()
}

/// Stage one overlay as its own Hyperframes project dir (index.html + a minimal
/// hyperframes.json) so `lint .` / `render .` resolve it. Returns the dir.
fn stage_comp(dir: &Path, name: &str, html: &str) -> Result<PathBuf, String> {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or("overlay");
    let proj = dir.join(format!("hf-{stem}"));
    std::fs::create_dir_all(&proj).map_err(|e| format!("mkdir overlay project: {e}"))?;
    std::fs::write(proj.join("index.html"), html).map_err(|e| format!("write overlay comp: {e}"))?;
    let meta = proj.join("hyperframes.json");
    if !meta.is_file() {
        std::fs::write(&meta, r#"{"paths":{"blocks":"."}}"#).map_err(|e| format!("write hyperframes.json: {e}"))?;
    }
    Ok(proj)
}

fn render_comp(app: &tauri::AppHandle, project: &str, dir: &Path, comp_file: &str, out_mov: &Path, _w: u32, _h: u32, fps: f64) -> Result<(), String> {
    let (path_env, hf_bin) = toolchain(app)?;
    emit(app, project, "hyperframes", "checking overlay…");
    lint_comp(&path_env, &hf_bin, &dir.join(comp_file), dir)?;
    emit(app, project, "hyperframes", "rendering overlay (transparent)…");
    // ProRes 4444 MOV carries a real alpha channel (verified: ~99% of pixels
    // transparent on a text comp). Rendered at COMPOSITION resolution — the
    // alpha path refuses --resolution upscaling, and the overlay is authored
    // at scene size anyway, so native res is exactly right. The ffmpeg graph
    // composites it with full alpha (no chroma keying).
    hf_run(&path_env, &hf_bin, &["render", ".", "-o", &out_mov.to_string_lossy(), "-f", &format!("{fps:.0}"), "--format", "mov", "-q", "high", "--quiet"], dir)?;
    if !out_mov.is_file() { return Err("hyperframes render produced no file".into()); }
    Ok(())
}

/// Register a rendered overlay mp4 as a project asset (media/ + manifest +
/// probes + thumbs). Returns the new asset.
fn register_overlay(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, file: &Path, name: &str) -> Result<Asset, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let mut m = video::load_manifest(broker, agent_id, project);
    let rel = format!("media/{}", file.file_name().and_then(|n| n.to_str()).unwrap_or("overlay.mp4"));
    m.assets.retain(|a| a.rel != rel);
    let mut a = Asset { id: video::new_id("a"), name: name.to_string(), kind: "video".to_string(), rel: rel.clone(), path: file.to_string_lossy().to_string(), linked: true, size: std::fs::metadata(file).map(|x| x.len()).unwrap_or(0), imported: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0), ..Default::default() };
    video::probe_into(app, file, &mut a)?;
    a.thumbs = video::build_thumbs(app, &proj, file, &a).ok();
    m.assets.push(a.clone());
    video::save_manifest(broker, agent_id, project, &m)?;
    Ok(a)
}

fn overlay_clip(asset: &Asset, track: &str, start: f64, end: f64, link_to: Option<&str>) -> RClip {
    let mut c = RClip::default();
    c.id = video::new_id("c"); c.track = track.into(); c.kind = "video".into();
    c.asset = asset.id.clone(); c.name = asset.name.clone();
    c.start = start; c.end = end; c.in_ = 0.0; c.out = (end - start).max(0.04);
    c.fit = "cover".into();
    if let Some(l) = link_to { c.link = l.into(); }
    c
}

// ---------------------------------------------------------------------------
// video_build_captions — transcript → transparent T1 overlay (Hyperframes)
// ---------------------------------------------------------------------------

pub fn build_captions(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, key_words: Vec<String>) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    toolchain(app)?; // fail fast before touching anything
    let mut comp = load_comp(broker, agent_id, project)?;
    let mut tr = load_transcript(&proj).ok_or("no transcript yet — run video_transcribe first")?;
    if tr.asset.is_empty() { if let Some(c) = comp.clips.iter().find(|c| c.track == "V1" && c.kind == "video") { tr.asset = c.asset.clone(); } }
    if !comp.captions.source_asset.is_empty() { tr.asset = comp.captions.source_asset.clone(); }

    let words = timeline_words(&comp, &tr);
    if words.is_empty() { return Err("no transcript words land on the timeline — check the A-roll selects cover the transcribed asset".into()); }
    let lines = caption_lines(&words, &key_words);
    let dur = comp.duration().max(lines.last().map(|l| l.e).unwrap_or(0.0) + 0.5);
    let (w, h) = (comp.scene.width.max(16) & !1, comp.scene.height.max(16) & !1);
    let fps = if comp.scene.fps > 0.0 { comp.scene.fps } else { 30.0 };

    let dir = proj.join(".cache").join("hf");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir hf cache: {e}"))?;
    let cid = format!("caps-{}", slug(project));
    let font_px = ((h as f64 * 0.030).round() as u32).clamp(24, 96);
    let html = HYP_COMP
        .replace("{{CID}}", &cid)
        .replace("{{DUR}}", &format!("{dur:.3}"))
        .replace("{{W}}", &w.to_string())
        .replace("{{H}}", &h.to_string())
        .replace("{{FPS}}", &format!("{fps:.0}"))
        .replace("{{Y_PX}}", &((h as f64 * comp.captions.y.clamp(0.02, 0.98)).round().to_string()))
        .replace("{{FONT_PX}}", &font_px.to_string())
        .replace("{{LINES_JSON}}", &serde_json::to_string(&lines).map_err(|e| e.to_string())?)
        .replace("{{STYLE_NOTE}}", &style_note(&comp));
    let cap_dir = stage_comp(&dir, "captions", &html)?;

    let out = proj.join("media").join(format!("{}.captions.hf.mov", slug(project)));
    render_comp(app, project, &cap_dir, "index.html", &out, w, h, fps)?;
    let asset = register_overlay(app, broker, agent_id, project, &out, &format!("{} (captions HF)", project))?;

    // Replace any previous caption overlay; lay the new one across the timeline.
    comp.clips.retain(|c| !(c.track == "T1" && !c.asset.is_empty()));
    let clip = overlay_clip(&asset, "T1", 0.0, dur, None);
    let clip_id = clip.id.clone();
    comp.clips.push(clip);
    comp.captions.enabled = true;
    comp.captions.key_words = key_words.clone();
    if comp.captions.source_asset.is_empty() { comp.captions.source_asset = tr.asset.clone(); }
    save_comp(broker, agent_id, project, &comp)?;
    let _ = tauri::Emitter::emit(app, "video-project-changed", json!({ "project": project, "tool": "video_build_captions" }));
    Ok(json!({ "ok": true, "asset": asset.id, "clip": clip_id, "lines": lines.len(), "duration": (dur * 100.0).round() / 100.0, "note": "caption overlay on T1 — toggle Captions → On to show/hide it in preview + export" }))
}

// ---------------------------------------------------------------------------
// video_render_overlay — one approved graphic → transparent V3 clip
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn render_overlay(app: &tauri::AppHandle, broker: &Broker, agent_id: &str, project: &str, name: &str, start: f64, end: f64, comp_html: &str, behind_subject: bool) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    toolchain(app)?;
    if end <= start + 0.05 { return Err("overlay needs start < end (≥ 0.05s apart)".into()); }
    if !(comp_html.contains("data-composition-id") && comp_html.contains("class=\"clip\"") && comp_html.contains("window.__timelines")) {
        return Err("comp_html must be a Hyperframes composition (data-composition-id, class=\"clip\" elements, window.__timelines)".into());
    }
    let mut comp = load_comp(broker, agent_id, project)?;
    let dur = comp.duration();
    if start < 0.0 || start >= dur.max(end) { return Err(format!("start {start} is outside the timeline (duration {dur:.2}s)")); }

    let dir = proj.join(".cache").join("hf");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir hf cache: {e}"))?;
    let gfx_dir = stage_comp(&dir, &format!("gfx-{}", slug(name)), comp_html)?;
    let (w, h) = (comp.scene.width.max(16) & !1, comp.scene.height.max(16) & !1);
    let fps = if comp.scene.fps > 0.0 { comp.scene.fps } else { 30.0 };
    let out = proj.join("media").join(format!("{}-{}.hf.mov", slug(project), slug(name)));
    render_comp(app, project, &gfx_dir, "index.html", &out, w, h, fps)?;
    let asset = register_overlay(app, broker, agent_id, project, &out, &format!("{} (overlay HF)", name))?;

    let mut clip = overlay_clip(&asset, "V3", start, end.min(dur + 5.0), None);
    clip.behind_subject = behind_subject;
    let clip_id = clip.id.clone();
    comp.clips.push(clip);
    save_comp(broker, agent_id, project, &comp)?;
    let _ = tauri::Emitter::emit(app, "video-project-changed", json!({ "project": project, "tool": "video_render_overlay" }));
    Ok(json!({ "ok": true, "asset": asset.id, "clip": clip_id, "track": "V3", "start": start, "end": end }))
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn video_build_captions(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, key_words: Option<Vec<String>>) -> Result<Value, String> {
    let broker = broker.inner().clone();
    tokio::task::spawn_blocking(move || build_captions(&app, &broker, &agent_id, &project, key_words.unwrap_or_default())).await.map_err(|e| format!("captions task: {e}"))?
}

#[tauri::command]
pub async fn video_render_overlay_cmd(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, name: String, start: f64, end: f64, comp_html: String, behind_subject: Option<bool>) -> Result<Value, String> {
    let broker = broker.inner().clone();
    tokio::task::spawn_blocking(move || render_overlay(&app, &broker, &agent_id, &project, &name, start, end, &comp_html, behind_subject.unwrap_or(false))).await.map_err(|e| format!("overlay task: {e}"))?
}

/// Style-guide picker: text extraction for .md/.txt/.rtf/.pdf/.docx → saved on
/// composition.graphics (so the agent always builds in-style). Audio/video
/// uploads are refused — a style guide is TEXT.
#[tauri::command]
pub async fn video_pick_style_guide(app: tauri::AppHandle, broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;
    video::project_dir(&broker, &agent_id, &project)?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog().file()
        .add_filter("Style guide", &["md", "txt", "rtf", "pdf", "docx"])
        .set_title("Pick a graphics style guide")
        .pick_file(move |c| { let _ = tx.send(c); });
    let Some(f) = rx.await.map_err(|_| "picker closed".to_string())? else { return Err("no file picked".into()) };
    let path = f.into_path().map_err(|e| e.to_string())?;
    if !path.is_file() { return Err("not a file".into()); }
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    if !["md", "txt", "rtf", "pdf", "docx"].contains(&ext.as_str()) { return Err("style guides are text: .md .txt .rtf .pdf .docx".into()); }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("guide").to_string();
    let bytes = std::fs::read(&path).map_err(|e| format!("read guide: {e}"))?;
    if bytes.len() > 4 * 1024 * 1024 { return Err("style guide is too large (max 4MB)".into()); }
    let text = extract_guide_text(&ext, &bytes)?;
    let text = text.trim().chars().take(24_000).collect::<String>();
    if text.trim().is_empty() { return Err("no readable text in that file".into()); }

    let mut comp = load_comp(&broker, &agent_id, &project)?;
    comp.graphics.style_guide = text.clone();
    comp.graphics.style_guide_name = name.clone();
    save_comp(&broker, &agent_id, &project, &comp)?;
    let _ = tauri::Emitter::emit(&app, "video-project-changed", json!({ "project": project, "tool": "video_pick_style_guide" }));
    Ok(json!({ "ok": true, "name": name, "chars": text.len() }))
}

fn extract_guide_text(ext: &str, bytes: &[u8]) -> Result<String, String> {
    match ext {
        "md" | "txt" => String::from_utf8(bytes.to_vec()).map_err(|_| "that file isn't UTF-8 text".into()),
        "rtf" => Ok(extract_rtf_text(bytes)),
        "pdf" => extract_pdf_text(bytes),
        "docx" => extract_docx_text(bytes),
        _ => Err("unsupported guide type".into()),
    }
}

/// Minimal RTF → text: skips groups/controls, keeps literal runs. Enough for
/// style guides (we want the words, not the formatting).
fn extract_rtf_text(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(s.len() / 2);
    let mut chars = s.chars().peekable();
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '\\' => {
                // control word / escaped char / hex escape
                let mut word = String::new();
                while let Some(&n) = chars.peek() { if n.is_ascii_alphanumeric() || n == '*' { word.push(n); chars.next(); } else { break; } }
                if word == "par" || word == "line" { out.push('\n'); }
                else if word == "tab" { out.push('\t'); }
                else if word.starts_with('\'') || word.is_empty() {
                    if word.is_empty() {
                        if let Some(&n) = chars.peek() {
                            if n == '\'' {
                                chars.next();
                                let h: String = chars.by_ref().take(2).collect();
                                if let Ok(b) = u8::from_str_radix(&h, 16) { out.push(b as char); }
                            } else if n == '\\' || n == '{' || n == '}' { out.push(n); chars.next(); }
                        }
                    }
                }
                // skip a single trailing space after a control word
                if chars.peek() == Some(&' ') { chars.next(); }
            }
            '\n' | '\r' => { out.push('\n'); }
            _ => { if depth > 0 { out.push(c); } }
        }
    }
    // collapse 3+ newlines
    let mut clean = String::with_capacity(out.len());
    let mut nl = 0;
    for c in out.chars() {
        if c == '\n' { nl += 1; if nl <= 2 { clean.push(c); } } else { nl = 0; clean.push(c); }
    }
    clean
}

/// Minimal PDF → text: pulls literal (…) and hex <…> strings from content
/// streams. Handles plain + FlateDecode streams; ignores the rest. Enough for
/// text-based guides (scanned-image PDFs have no text to extract).
fn extract_pdf_text(bytes: &[u8]) -> Result<String, String> {
    // Find stream…endstream spans and inflate FlateDecode ones.
    let mut texts: Vec<String> = vec![];
    let mut i = 0usize;
    while let Some(s) = find_sub(bytes, b"stream", i) {
        let body_start = s + 6 + if bytes.get(s + 6) == Some(&b'\r') && bytes.get(s + 7) == Some(&b'\n') { 2 } else if bytes.get(s + 6) == Some(&b'\n') { 1 } else { 0 };
        let Some(e) = find_sub(bytes, b"endstream", body_start) else { break };
        let dict_start = bytes[..s].iter().rposition(|&b| b == b'<').unwrap_or(0);
        let dict = &bytes[dict_start..s];
        let body = &bytes[body_start..e];
        let data: Vec<u8> = if contains_sub(dict, b"/FlateDecode") {
            match miniz_oxide::inflate::decompress_to_vec(body) {
                Ok(v) => v,
                Err(_) => body.to_vec(),
            }
        } else { body.to_vec() };
        texts.push(pdf_strings(&data));
        i = e + 9;
    }
    if texts.is_empty() { texts.push(pdf_strings(bytes)); }
    let joined = texts.join("\n");
    // Unescape PDF string escapes + join hyphenated line breaks.
    let t = joined.replace("\\n", "\n").replace("\\r", "\n").replace("\\t", " ").replace("\\(", "(").replace("\\)", ")").replace("\\\\", "\\");
    let t = t.replace("-\n", "");
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.trim().is_empty() { return Err("no extractable text — that PDF may be scanned images".into()); }
    Ok(t)
}

fn find_sub(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}
fn contains_sub(hay: &[u8], needle: &[u8]) -> bool { find_sub(hay, needle, 0).is_some() }

/// Literal (parenthesized) + hex <…> strings from a PDF content stream.
fn pdf_strings(data: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    while i < data.len() {
        match data[i] {
            b'(' => {
                let mut depth = 1usize; let mut buf: Vec<u8> = vec![]; i += 1;
                while i < data.len() && depth > 0 {
                    match data[i] {
                        b'\\' if i + 1 < data.len() => { buf.push(data[i + 1]); i += 2; }
                        b'(' => { depth += 1; buf.push(b'('); i += 1; }
                        b')' => { depth -= 1; if depth > 0 { buf.push(b')'); } i += 1; }
                        b => { buf.push(b); i += 1; }
                    }
                }
                out.push_str(&String::from_utf8_lossy(&buf)); out.push(' ');
            }
            b'<' if data.get(i + 1) != Some(&b'<') => {
                let mut hex: Vec<u8> = vec![]; i += 1;
                while i < data.len() && data[i] != b'>' { if data[i].is_ascii_hexdigit() { hex.push(data[i]); } i += 1; }
                i += 1; // '>'
                let mut bytes: Vec<u8> = vec![];
                let mut j = 0;
                while j + 1 < hex.len() { if let Ok(b) = u8::from_str_radix(&String::from_utf8_lossy(&hex[j..j + 2]).to_string(), 16) { bytes.push(b); } j += 2; }
                // UTF-16BE BOM → decode; else latin-1-ish lossy.
                if bytes.starts_with(&[0xFE, 0xFF]) {
                    let u16s: Vec<u16> = bytes[2..].chunks(2).filter_map(|c| if c.len() == 2 { Some(u16::from_be_bytes([c[0], c[1]])) } else { None }).collect();
                    out.push_str(&String::from_utf16_lossy(&u16s)); out.push(' ');
                } else {
                    out.push_str(&String::from_utf8_lossy(&bytes)); out.push(' ');
                }
            }
            _ => { i += 1; }
        }
    }
    out
}

/// Minimal DOCX → text: unzip word/document.xml, collect <w:t> runs.
fn extract_docx_text(bytes: &[u8]) -> Result<String, String> {
    let mut cur = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(&mut cur).map_err(|_| "that .docx can't be opened".to_string())?;
    let mut xml = zip.by_name("word/document.xml").map_err(|_| "no document text in that .docx".to_string())?;
    let mut s = String::new();
    use std::io::Read;
    xml.read_to_string(&mut s).map_err(|e| format!("read docx: {e}"))?;
    if s.len() > 8 * 1024 * 1024 { return Err("docx text too large".into()); }
    // Collect <w:t>…</w:t> runs; </w:p> → newline.
    let mut out = String::new();
    let mut i = 0usize;
    let b = s.as_bytes();
    while i < b.len() {
        if s[i..].starts_with("<w:t") {
            let Some(st) = s[i..].find('>') else { break };
            let st = i + st + 1;
            let Some(en) = s[st..].find("</w:t>") else { break };
            out.push_str(&s[st..st + en].replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\""));
            i = st + en + 6;
        } else if s[i..].starts_with("</w:p>") || s[i..].starts_with("<w:tab") {
            out.push(if s[i..].starts_with("</w:p>") { '\n' } else { '\t' });
            i += 1;
        } else { i += 1; }
    }
    let t: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.trim().is_empty() { return Err("no readable text in that .docx".into()); }
    Ok(t)
}

// ---------------------------------------------------------------------------
// Caption timing for the UI (replaces the old ASS preview path)
// ---------------------------------------------------------------------------

/// Caption lines on the timeline (from the transcript through the A-roll cuts)
/// — the UI draws these as a timing strip; the LOOK is the T1 overlay.
pub fn caption_timing(broker: &Broker, agent_id: &str, project: &str, comp: &Composition) -> Result<Value, String> {
    let proj = video::project_dir(broker, agent_id, project)?;
    let Some(mut tr) = load_transcript(&proj) else { return Ok(json!({ "lines": [] })) };
    if tr.asset.is_empty() { if let Some(c) = comp.clips.iter().find(|c| c.track == "V1" && c.kind == "video") { tr.asset = c.asset.clone(); } }
    let words = timeline_words(comp, &tr);
    let lines = caption_lines(&words, &comp.captions.key_words);
    let out: Vec<Value> = lines.iter().map(|l| json!({ "s": l.s, "e": l.e, "words": l.words.iter().map(|w| json!({ "w": w.w, "s": l.s, "e": l.e })).collect::<Vec<_>>() })).collect();
    Ok(json!({ "lines": out }))
}

#[tauri::command]
pub async fn video_caption_timing(broker: tauri::State<'_, Arc<Broker>>, agent_id: String, project: String, composition: Value) -> Result<Value, String> {
    let broker = broker.inner().clone();
    tokio::task::spawn_blocking(move || {
        let assets = video::load_manifest(&broker, &agent_id, &project).assets;
        let comp = crate::video_render::parse_composition(&composition, &assets)?;
        caption_timing(&broker, &agent_id, &project, &comp)
    }).await.map_err(|e| format!("caption timing task: {e}"))?
}

// ---------------------------------------------------------------------------
// Agent tool schemas + shared helpers for video_tools.rs
// ---------------------------------------------------------------------------

pub fn tool_schemas() -> Vec<Value> {
    vec![
        json!({ "name": "video_build_captions", "description": "Build the Hyperframes caption overlay for a project: transcript words (through the A-roll cuts) → a transparent caption composition in the project's style (Graphics panel instructions + style guide) → rendered to media/ and laid as a T1 overlay clip. Replaces the old ASS caption pipeline entirely. Run video_transcribe first.",
            "input_schema": { "type": "object", "properties": { "project": { "type": "string" }, "keyWords": { "type": "array", "items": { "type": "string" }, "description": "words to highlight, e.g. [\"agent\",\"free\"]" } }, "required": ["project"] } }),
        json!({ "name": "video_render_overlay", "description": "Render ONE approved Hyperframes graphic to a transparent overlay clip on V3. comp_html must be a complete Hyperframes composition (data-composition-id, class=\"clip\" timed elements, a paused GSAP timeline on window.__timelines). Styled by the project's Graphics panel — read composition.graphics first and match it. PLAN first, build only after approval, one call per graphic in timeline order.",
            "input_schema": { "type": "object", "properties": { "project": { "type": "string" }, "name": { "type": "string", "description": "short graphic name, e.g. lower-third-intro" }, "start": { "type": "number", "description": "timeline in (seconds)" }, "end": { "type": "number", "description": "timeline out (seconds)" }, "comp_html": { "type": "string", "description": "the full Hyperframes composition HTML" }, "behindSubject": { "type": "boolean", "description": "render behind the person (needs a matte)" } }, "required": ["project", "name", "start", "end", "comp_html"] } }),
    ]
}

pub const HYPERFRAMES_INSTRUCTIONS: &str = "\n\nHYPERFRAMES GRAPHICS + CAPTIONS: every graphic and caption is a transparent Hyperframes overlay clip (captions on T1, graphics on V3), composited by ffmpeg on export — there is no ASS/drawtext caption path.\n- Read composition.graphics (instructions + styleGuide) BEFORE authoring anything and match that style.\n- Captions: video_build_captions {project, keyWords} builds the whole T1 overlay from the transcript. Transcribe first if needed.\n- Graphics: video_render_overlay {project, name, start, end, comp_html, behindSubject?} renders ONE overlay per call. comp_html is a COMPLETE Hyperframes composition: #stage with data-composition-id/data-width/data-height, class=\"clip\" elements with data-start/data-duration/data-track-index, a paused GSAP timeline on window.__timelines.<id> (or CSS/WAAPI seekable animation). Keep it deterministic (no Date.now/random/network). The overlay is transparent — no opaque backgrounds unless the design calls for a panel.\n\nGRAPHICS PROTOCOL (mandatory): when asked for graphics, titles, callouts or overlays, FIRST reply with a PLAN and NO tool calls that build: a numbered list, one line per graphic — timecode range, the on-screen text (exact wording), style/placement, and why it helps. End with \"Approve, or give revision notes.\" Only after the user approves (\"approve\", \"go\", \"yes\", \"do it\") do you build — ONE overlay tool call PER graphic, in timeline order, so each one appears on the timeline as it lands. Revision notes → revise the plan and ask again. Never batch overlays into one call and never skip the plan.";

pub fn is_hyperframes_tool(name: &str) -> bool { name == "video_build_captions" || name == "video_render_overlay" }

/// Dispatch from video_tools::run (sync — runs on the caller's thread).
pub fn exec(broker: &Broker, agent_id: &str, name: &str, input: &Value) -> Result<Value, String> {
    let s = |k: &str| input.get(k).and_then(|v| v.as_str()).map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let project = s("project").ok_or("project is required")?;
    // video_tools::install_app holds the AppHandle; grab it the same way.
    let app = super::video_tools::app_handle().ok_or("video tools not initialized")?;
    match name {
        "video_build_captions" => {
            let kw: Vec<String> = input.get("keyWords").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|y| y.trim().to_string())).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            build_captions(&app, broker, agent_id, &project, kw)
        }
        "video_render_overlay" => {
            let (name, start, end, html) = (
                s("name").ok_or("name is required")?,
                input.get("start").and_then(|v| v.as_f64()).ok_or("start is required")?,
                input.get("end").and_then(|v| v.as_f64()).ok_or("end is required")?,
                input.get("comp_html").and_then(|v| v.as_str()).ok_or("comp_html is required")?.to_string(),
            );
            let behind = input.get("behindSubject").and_then(|v| v.as_bool()).unwrap_or(false);
            render_overlay(&app, broker, agent_id, &project, &name, start, end, &html, behind)
        }
        other => Err(format!("unknown hyperframes tool: {other}")),
    }
}

static HF_MEMO: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Memo of overlay asset ids per project (reserved for a future overlay manager).
#[allow(dead_code)]
pub fn memo_overlay(_project: &str, _asset: &str) {
    let _ = HF_MEMO.get_or_init(HashMap::new);
}
