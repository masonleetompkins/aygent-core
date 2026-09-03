// AYGENT — VIDEO TOOL v0.1 (project store).
//
// File-backed video projects live in the agent's jailed folder under `Video/`:
//   Video/<project>/composition.json   (the edit — agent + UI both read/write)
//   Video/<project>/cutlist.json       (optional: approved selects)
//   Video/<project>/transcript.json    (optional: cached transcript)
// The RENDER itself is the agent's job (Pro Mode shell + provisioned ffmpeg /
// HyperFrames); these commands are only the project store + toolchain status,
// so the Video tab needs no new exec authority. Mirrors sparks_list/read.

use std::sync::Arc;
use crate::broker::{self, Broker};

/// Projects root inside the agent folder.
const VIDEO_ROOT: &str = "Video";

fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_missing(e: &broker::BrokerError) -> bool {
    let s = format!("{e:?}");
    s.contains("NotFound") || s.contains("No such file") || s.contains("errno Some(2)")
}

fn read_json(
    broker: &tauri::State<Arc<Broker>>,
    agent_id: &str,
    rel: &str,
) -> Option<serde_json::Value> {
    let abs = broker.resolve(agent_id, rel, broker::Mode::Read).ok()?;
    let text = std::fs::read_to_string(&abs).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&text).ok()
}

/// Toolchain status: provisioned ffmpeg (+ hyperframes, which owns it).
#[tauri::command]
pub fn video_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let ffmpeg = crate::provision::ffmpeg_bin(&app).map(|p| p.to_string_lossy().to_string());
    let hyperframes = crate::provision::hyperframes_installed(&app);
    Ok(serde_json::json!({ "ffmpeg": ffmpeg, "hyperframes": hyperframes }))
}

/// List video projects (dirs under Video/ containing a composition.json).
#[tauri::command]
pub fn video_projects(
    broker: tauri::State<Arc<Broker>>,
    agent_id: String,
) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() {
        return Err("select an agent first".into());
    }
    let dir = match broker.resolve(&agent_id, VIDEO_ROOT, broker::Mode::Read) {
        Ok(p) => p,
        Err(e) if is_missing(&e) => return Ok(serde_json::json!([])),
        Err(e) => return Err(format!("refused by jail: {e:?}")),
    };
    let mut out: Vec<serde_json::Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if !slug_ok(&name) {
                continue;
            }
            if !p.join("composition.json").is_file() {
                continue;
            }
            let modified = std::fs::metadata(p.join("composition.json"))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push(serde_json::json!({ "name": name, "modified": modified }));
        }
    }
    out.sort_by(|a, b| {
        b.get("modified")
            .and_then(|x| x.as_i64())
            .unwrap_or(0)
            .cmp(&a.get("modified").and_then(|x| x.as_i64()).unwrap_or(0))
    });
    Ok(serde_json::json!(out))
}

/// Load a project's composition (+ optional cutlist/transcript sidecars).
#[tauri::command]
pub fn video_load(
    broker: tauri::State<Arc<Broker>>,
    agent_id: String,
    project: String,
) -> Result<serde_json::Value, String> {
    if agent_id.trim().is_empty() {
        return Err("select an agent first".into());
    }
    if !slug_ok(&project) {
        return Err("invalid project name".into());
    }
    let base = format!("{}/{}", VIDEO_ROOT, project);
    let composition = read_json(&broker, &agent_id, &format!("{}/composition.json", base))
        .ok_or_else(|| format!("no such project: {project}"))?;
    let cutlist = read_json(&broker, &agent_id, &format!("{}/cutlist.json", base))
        .unwrap_or(serde_json::Value::Null);
    let transcript = read_json(&broker, &agent_id, &format!("{}/transcript.json", base))
        .unwrap_or(serde_json::Value::Null);
    Ok(serde_json::json!({
        "project": project,
        "composition": composition,
        "cutlist": cutlist,
        "transcript": transcript,
    }))
}

/// Save (create or overwrite) a project's composition.json. Must be an object.
#[tauri::command]
pub fn video_save(
    broker: tauri::State<Arc<Broker>>,
    agent_id: String,
    project: String,
    composition: serde_json::Value,
) -> Result<(), String> {
    if agent_id.trim().is_empty() {
        return Err("select an agent first".into());
    }
    if !slug_ok(&project) {
        return Err("invalid project name — use letters, digits, - _".into());
    }
    if !composition.is_object() {
        return Err("composition must be a JSON object".into());
    }
    let text =
        serde_json::to_string_pretty(&composition).map_err(|e| format!("serialize: {e}"))?;
    if text.len() > 2 * 1024 * 1024 {
        return Err("composition too large (>2MB)".into());
    }
    let rel = format!("{}/{}/composition.json", VIDEO_ROOT, project);
    let abs = broker
        .resolve(&agent_id, &rel, broker::Mode::Write)
        .map_err(|e| format!("refused by jail: {e:?}"))?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    std::fs::write(&abs, text.as_bytes()).map_err(|e| format!("write: {e}"))?;
    Ok(())
}
