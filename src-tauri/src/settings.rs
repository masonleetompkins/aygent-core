// AYGENT — Per-folder settings (Phase 1).
//
// Small key/value settings that are scoped to an AGENT FOLDER (not global, not
// per-conversation). First use: the selected MODEL. Like conversations, this is
// APP STATE — it lives in the app data dir keyed by a hash of the folder path,
// NOT inside the user's folder (vault stays clean; nothing gets swept into
// checkpoints).
//
// LAYOUT:  <app_data>/settings/<folder_key>.json
//   { "model": "claude-..." }   — empty/missing model = "auto" (backend picks
//                                 haiku, today's default behavior).

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct FolderSettings {
    /// Selected model id ("" = auto: prefer haiku, else first available).
    #[serde(default)]
    pub model: String,
}

/// FNV-1a 64-bit — same stable, dependency-free hash the conversations store
/// uses, so a folder's settings and conversations share a key scheme.
fn folder_key(folder: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in folder.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn settings_path(app_data: &Path, folder: &str) -> Result<PathBuf, String> {
    let dir = app_data.join("settings");
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir settings: {e}"))?;
    Ok(dir.join(format!("{}.json", folder_key(folder))))
}

/// Load settings for a folder. Missing/corrupt file = defaults (never errors
/// the UI over a settings read).
pub fn load(app_data: &Path, folder: &str) -> FolderSettings {
    let path = match settings_path(app_data, folder) {
        Ok(p) => p,
        Err(_) => return FolderSettings::default(),
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Save settings for a folder (whole-struct write; it's tiny).
pub fn save(app_data: &Path, folder: &str, s: &FolderSettings) -> Result<(), String> {
    let path = settings_path(app_data, folder)?;
    let text = serde_json::to_string_pretty(s).map_err(|e| format!("serialize settings: {e}"))?;
    fs::write(&path, text).map_err(|e| format!("write settings: {e}"))
}
