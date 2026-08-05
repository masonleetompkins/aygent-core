// AYGENT — Agent profiles (multi-agent core, Phase 1).
//
// "An Agent is just a config profile." The daemon runs ONE process; many agent
// profiles live inside it. A profile owns its model, provider, folder scope,
// context mode, and system prompt. The NEW organizing unit is the `agentId`
// (was: the folder-path hash). Per-agent state now lives under the agent id.
//
// The security kernel is UNCHANGED: the broker still jails by the profile's
// `folder_path`. Agent identity does not touch the jail — it only chooses which
// path to scope to.
//
// LAYOUT:
//   <app_data>/agents/index.json            -> { agents: [AgentProfile], activeId }
//   <app_data>/agents/<agentId>/settings.json, conversations/, tools/...
//
// Save Points stay keyed by FOLDER PATH (a shadow git repo per real folder), so
// two agents scoped to the same folder share ONE history + the folder write lock
// (CONTRACTS §4). Identity must not fork the git timeline.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub color: String,

    #[serde(default)]
    pub folder_path: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider: String,

    #[serde(default = "default_context_mode")]
    pub context_mode: String, // "isolated" | "shared:<poolId>"
    #[serde(default)]
    pub system_prompt: String,

    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub archived: bool,
}

fn default_context_mode() -> String { "isolated".to_string() }

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AgentIndex {
    #[serde(default)]
    pub agents: Vec<AgentProfile>,
    #[serde(default)]
    pub active_id: String,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Dependency-free id: time (ms, base36) + a small random suffix. Sortable-ish,
/// unique enough for a local single-user app; no ULID crate needed.
fn new_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rand: u32 = {
        // cheap non-crypto entropy: mix time + address of a stack local
        let x = &ms as *const _ as usize as u64;
        let mut h = x ^ (ms as u64).wrapping_mul(0x9E3779B97F4A7C15);
        h ^= h >> 29; h = h.wrapping_mul(0xBF58476D1CE4E5B9); h ^= h >> 32;
        (h & 0xFFFFFFFF) as u32
    };
    format!("a{}{:07x}", to_base36(ms as u64), rand)
}

fn to_base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 { return "0".into(); }
    let mut out = Vec::new();
    while n > 0 { out.push(D[(n % 36) as usize]); n /= 36; }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn agents_dir(app_data: &Path) -> Result<PathBuf, String> {
    let d = app_data.join("agents");
    fs::create_dir_all(&d).map_err(|e| format!("mkdir agents: {e}"))?;
    Ok(d)
}

fn index_path(app_data: &Path) -> Result<PathBuf, String> {
    Ok(agents_dir(app_data)?.join("index.json"))
}

pub fn load_index(app_data: &Path) -> AgentIndex {
    let p = match index_path(app_data) { Ok(p) => p, Err(_) => return AgentIndex::default() };
    fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_index(app_data: &Path, idx: &AgentIndex) -> Result<(), String> {
    let text = serde_json::to_string_pretty(idx).map_err(|e| format!("serialize index: {e}"))?;
    fs::write(index_path(app_data)?, text).map_err(|e| format!("write index: {e}"))
}

/// Create a new agent profile and persist it. Becomes active if it's the first.
#[allow(clippy::too_many_arguments)]
pub fn create(
    app_data: &Path,
    name: &str,
    icon: &str,
    color: &str,
    folder_path: &str,
    model: &str,
    provider: &str,
    context_mode: &str,
    system_prompt: &str,
) -> Result<AgentProfile, String> {
    let mut idx = load_index(app_data);
    let profile = AgentProfile {
        id: new_id(),
        name: if name.trim().is_empty() { "My Agent".into() } else { name.trim().to_string() },
        icon: if icon.is_empty() { "🤖".into() } else { icon.to_string() },
        color: if color.is_empty() { "#5b8cff".into() } else { color.to_string() },
        folder_path: folder_path.to_string(),
        model: model.to_string(),
        provider: provider.to_string(),
        context_mode: if context_mode.is_empty() { default_context_mode() } else { context_mode.to_string() },
        system_prompt: system_prompt.to_string(),
        created_at: now(),
        updated_at: now(),
        archived: false,
    };
    let first = idx.agents.is_empty();
    idx.agents.push(profile.clone());
    if first || idx.active_id.is_empty() { idx.active_id = profile.id.clone(); }
    save_index(app_data, &idx)?;
    Ok(profile)
}

/// BACK-COMPAT MIGRATION: if there are no agent profiles yet but a legacy
/// agent-folder record exists, synthesize "My Agent" pointing at that folder so
/// the current single-folder user becomes Agent 1 with zero action / data loss.
/// Legacy per-folder stores (conversations/settings/tools keyed by folder hash)
/// keep working because the daemon can still resolve a folder path directly;
/// this just gives the UI a profile to show and select.
pub fn ensure_migrated(app_data: &Path, legacy_folder: Option<&str>) -> Result<(), String> {
    let idx = load_index(app_data);
    if !idx.agents.is_empty() { return Ok(()); }
    if let Some(folder) = legacy_folder.filter(|f| !f.is_empty()) {
        create(app_data, "My Agent", "🤖", "#5b8cff", folder, "", "", "isolated", "")?;
    }
    Ok(())
}
