// AYGENT — Conversation persistence (Phase 1).
//
// Chat history is APP STATE, not user files — so it lives in the app's data dir,
// NOT inside the agent folder. Two reasons this matters:
//   1. The vault stays clean (no agent metadata cluttering the user's notes).
//   2. Conversations don't get swept into CHECKPOINTS (which snapshot the agent
//      folder). Storing chat logs in the folder would recurse the two systems.
//
// LAYOUT:  <app_data>/conversations/<folder_key>/<conv_id>.json
//   - folder_key = a stable hash of the agent folder path, so each folder has
//     its own independent set of conversations.
//   - each file = one conversation: { id, title, updated, msgs, history }.
//     `msgs` is the UI-facing render list; `history` is the provider-format
//     message array the agent loop needs to continue the thread.
//
// No extra deps: we use a tiny FNV-1a hash for the folder key and serde_json for
// the files. SQLite can replace this later if we need querying/scale.

use std::fs;
use std::path::{Path, PathBuf};

/// One stored conversation. `msgs` + `history` are opaque JSON blobs the UI /
/// agent loop own; the backend only persists them verbatim.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub updated: i64, // unix seconds
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub order: i64, // manual sort key (lower = higher in the list); 0 = unset
    #[serde(default)]
    pub msgs: serde_json::Value,    // UI render list
    #[serde(default)]
    pub history: serde_json::Value, // provider-format history
}

/// Lightweight list entry (no bodies) for the history sidebar.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConvMeta {
    pub id: String,
    pub title: String,
    pub updated: i64,
    pub pinned: bool,
    pub order: i64,
}

/// FNV-1a 64-bit — stable, dependency-free hash for the folder key.
fn folder_key(folder: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in folder.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// The per-folder conversations directory, created if missing.
fn conv_dir(app_data: &Path, folder: &str) -> Result<PathBuf, String> {
    let dir = app_data.join("conversations").join(folder_key(folder));
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir conversations: {e}"))?;
    Ok(dir)
}

fn conv_path(app_data: &Path, folder: &str, id: &str) -> Result<PathBuf, String> {
    // Guard the id so it can't traverse out of the dir.
    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err("invalid conversation id".into());
    }
    Ok(conv_dir(app_data, folder)?.join(format!("{id}.json")))
}

/// List conversation metadata for a folder. Sort: pinned first, then by manual
/// `order` (if set), then by `updated` newest-first. This gives pin-to-top +
/// drag-reorder, with a sensible default for untouched items.
pub fn list(app_data: &Path, folder: &str) -> Result<Vec<ConvMeta>, String> {
    let dir = conv_dir(app_data, folder)?;
    let mut out = Vec::new();
    let rd = match fs::read_dir(&dir) { Ok(rd) => rd, Err(_) => return Ok(out) };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(c) = serde_json::from_str::<Conversation>(&text) {
                out.push(ConvMeta { id: c.id, title: c.title, updated: c.updated, pinned: c.pinned, order: c.order });
            }
        }
    }
    out.sort_by(|a, b| {
        // pinned above unpinned
        b.pinned.cmp(&a.pinned)
            // then manual order (0 = unset sorts to the bottom of its group)
            .then_with(|| order_key(a.order).cmp(&order_key(b.order)))
            // then newest-updated first
            .then_with(|| b.updated.cmp(&a.updated))
    });
    Ok(out)
}

/// Unset order (0) should sort AFTER any explicit order, so map it to max.
fn order_key(o: i64) -> i64 {
    if o == 0 { i64::MAX } else { o }
}

/// Update just the sort metadata (pinned + order) for a set of conversations,
/// without rewriting their bodies. `updates` = list of (id, pinned, order).
pub fn reorder(app_data: &Path, folder: &str, updates: Vec<(String, bool, i64)>) -> Result<(), String> {
    for (id, pinned, order) in updates {
        let path = conv_path(app_data, folder, &id)?;
        if !path.exists() { continue; }
        let text = fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?;
        let mut c: Conversation = serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))?;
        c.pinned = pinned;
        c.order = order;
        let out = serde_json::to_string_pretty(&c).map_err(|e| format!("serialize: {e}"))?;
        fs::write(&path, out).map_err(|e| format!("write: {e}"))?;
    }
    Ok(())
}

/// Load one full conversation.
pub fn load(app_data: &Path, folder: &str, id: &str) -> Result<Conversation, String> {
    let path = conv_path(app_data, folder, id)?;
    let text = fs::read_to_string(&path).map_err(|e| format!("read conversation: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("parse conversation: {e}"))
}

/// Save (create or overwrite) a conversation. Stamps `updated` to now, but
/// PRESERVES any existing `pinned`/`order` (the caller's save payload may not
/// carry them, and we don't want a turn-save to clobber a pin/reorder).
pub fn save(app_data: &Path, folder: &str, mut conv: Conversation) -> Result<(), String> {
    conv.updated = now_secs();
    let path = conv_path(app_data, folder, &conv.id)?;
    if path.exists() {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(prev) = serde_json::from_str::<Conversation>(&text) {
                if !conv.pinned { conv.pinned = prev.pinned; }
                if conv.order == 0 { conv.order = prev.order; }
            }
        }
    }
    let text = serde_json::to_string_pretty(&conv).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, text).map_err(|e| format!("write conversation: {e}"))
}

/// Delete a conversation.
pub fn delete(app_data: &Path, folder: &str, id: &str) -> Result<(), String> {
    let path = conv_path(app_data, folder, id)?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("delete conversation: {e}"))?;
    }
    Ok(())
}

fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
