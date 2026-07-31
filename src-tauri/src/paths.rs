// AYGENT — PATHS SEAM (config relocation, Atlas config-relocation design).
//
// THE GOVERNING RULE: the AYGENT ROOT FOLDER is the source of truth. App-support
// holds ONE pointer file (root.json). The macOS Keychain holds keys. Nothing else
// lives outside the root. Have the folder + Keychain => you have your whole setup
// (that's the new-machine port story, for free).
//
// LAYOUT:
//   <app_support>/root.json            -> { v, root, lastOpened }   (the ONLY pointer)
//   <root>/.aygent/state.db            -> SQLite state spine
//   <root>/.aygent/stores/             -> folderkey/agent JSON stores (tools, policy, pro-mode)
//   <root>/.aygent/logs/               -> shell logs etc.
//   <root>/.aygent/backups/            -> relocation archives
//   <root>/aygent-root.json            -> the ROOT MANIFEST (self-describing; enables detect+restore)
//   <root>/<AgentName>/                -> each agent's HOME (AYGENT.md, context/, memory/, .aygent/)
//
// Step 0 (this file, pure refactor): resolve the STATE DIR from the pointer if
// present, else fall back to app_data. Every `app_data()` call site now routes
// through `state_dir()`, so flipping the pointer relocates the whole config with
// zero call-site churn.

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Current manifest/pointer schema version (bump on breaking layout changes).
pub const AYGENT_LAYOUT_VERSION: u32 = 1;

/// The single bootstrap pointer, stored in app-support. Points at the root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootPointer {
    pub v: u32,
    pub root: String,
    #[serde(default)]
    pub last_opened: Option<String>,
}

/// The root manifest, stored at <root>/aygent-root.json. Makes a folder
/// self-describing as an AYGENT home so onboarding can DETECT + RESTORE it on a
/// new machine. Non-secret; safe to commit/back up with the folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootManifest {
    pub v: u32,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub app: String, // "aygent"
}

/// The app-support pointer path: <app_support>/root.json. This is the ONLY thing
/// AYGENT ever writes outside the root folder (besides Keychain).
pub fn pointer_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| format!("app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir app_support: {e}"))?;
    Ok(dir.join("root.json"))
}

/// Read the pointer, if it exists and is valid.
pub fn read_pointer(app: &tauri::AppHandle) -> Option<RootPointer> {
    let p = pointer_path(app).ok()?;
    let text = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str::<RootPointer>(&text).ok()
}

/// Write/overwrite the pointer to point at `root`.
pub fn write_pointer(app: &tauri::AppHandle, root: &Path) -> Result<(), String> {
    let p = pointer_path(app)?;
    let ptr = RootPointer {
        v: AYGENT_LAYOUT_VERSION,
        root: root.to_string_lossy().to_string(),
        last_opened: Some(now_iso()),
    };
    let body = serde_json::to_string_pretty(&ptr).map_err(|e| format!("serialize pointer: {e}"))?;
    std::fs::write(&p, body).map_err(|e| format!("write pointer: {e}"))
}

/// The configured ROOT folder, if the pointer exists AND the folder still
/// exists on disk. Returns None to trigger onboarding (missing pointer OR the
/// root folder is gone — e.g. an unmounted/renamed volume).
pub fn configured_root(app: &tauri::AppHandle) -> Option<PathBuf> {
    let ptr = read_pointer(app)?;
    let root = PathBuf::from(&ptr.root);
    if root.is_dir() { Some(root) } else { None }
}

/// THE STATE DIR — where SQLite + JSON stores live. Step-0 behavior:
///   - if a valid root is configured -> <root>/.aygent   (the "engine bay")
///   - else                          -> app_data          (legacy, pre-onboarding)
/// This is the single chokepoint the old `app_data()` helper now routes through.
/// Created if missing.
pub fn state_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = match configured_root(app) {
        Some(root) => root.join(".aygent"),
        None => {
            use tauri::Manager;
            app.path().app_data_dir().map_err(|e| format!("app_data_dir: {e}"))?
        }
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir state_dir: {e}"))?;
    Ok(dir)
}

/// The JSON-store subdir under the state dir (tools/policy/pro-mode files).
/// Kept as a helper so those stores can be flattened under stores/ later without
/// touching call sites that only need the state dir.
pub fn stores_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let d = state_dir(app)?.join("stores");
    std::fs::create_dir_all(&d).map_err(|e| format!("mkdir stores: {e}"))?;
    Ok(d)
}

/// Is a folder already an AYGENT root? (Has the manifest.) Used by onboarding to
/// DETECT + RESTORE instead of clobbering.
pub fn is_aygent_root(folder: &Path) -> bool {
    folder.join("aygent-root.json").is_file()
}

/// Read a folder's root manifest if present.
pub fn read_manifest(folder: &Path) -> Option<RootManifest> {
    let text = std::fs::read_to_string(folder.join("aygent-root.json")).ok()?;
    serde_json::from_str::<RootManifest>(&text).ok()
}

/// Initialize a folder as an AYGENT root: write the manifest + create the
/// `.aygent/` engine bay. Idempotent (won't overwrite an existing manifest).
pub fn init_root(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder.join(".aygent")).map_err(|e| format!("mkdir .aygent: {e}"))?;
    std::fs::create_dir_all(folder.join(".aygent").join("stores")).map_err(|e| format!("mkdir stores: {e}"))?;
    let manifest = folder.join("aygent-root.json");
    if !manifest.is_file() {
        let m = RootManifest {
            v: AYGENT_LAYOUT_VERSION,
            created_at: Some(now_iso()),
            app: "aygent".into(),
        };
        let body = serde_json::to_string_pretty(&m).map_err(|e| format!("serialize manifest: {e}"))?;
        std::fs::write(&manifest, body).map_err(|e| format!("write manifest: {e}"))?;
    }
    Ok(())
}

/// An agent's HOME folder under the root: <root>/<sanitized name>/. Its config,
/// soul (AYGENT.md), context, memory, and per-agent .aygent/ live here.
pub fn agent_home(root: &Path, agent_name: &str) -> PathBuf {
    root.join(sanitize_folder_name(agent_name))
}

/// Create an agent's home skeleton: the home dir + context/ + memory/ + .aygent/.
pub fn init_agent_home(root: &Path, agent_name: &str) -> Result<PathBuf, String> {
    let home = agent_home(root, agent_name);
    for sub in ["", "context", "memory", ".aygent"] {
        let d = if sub.is_empty() { home.clone() } else { home.join(sub) };
        std::fs::create_dir_all(&d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
    }
    Ok(home)
}

/// Make an agent name safe as a folder name (strip path separators + trim).
/// Keeps it human-readable; only removes what would break a path.
pub fn sanitize_folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '\0') { '-' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() { "Agent".to_string() } else { trimmed.to_string() }
}

fn now_iso() -> String {
    // Lightweight ISO-8601 UTC without pulling chrono into this module's API.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // format via chrono (already a dep) for correctness.
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| secs.to_string())
}
