// AYGENT — One-time JSON → SQLite import (M1.1).
//
// The pre-M1.1 stores were JSON files under <app_data>:
//   agents/index.json                      -> agent profiles + active id
//   settings/<folderKey>.json              -> per-folder {model, provider}
//   conversations/<folderKey>/<id>.json    -> per-folder conversations
//
// This runs ONCE (guarded by a marker row) at boot: it reads those files and
// writes them into the SQLite spine, then drops a `migrated_json` flag so it
// never runs again. It is ADDITIVE and NON-DESTRUCTIVE — the JSON files are left
// on disk untouched (a safety net; a later version can archive them). If there
// are no JSON files (fresh install), it's a no-op.
//
// FOLDER→AGENT MAPPING: legacy conversations/settings were keyed by a folder-
// path hash. Agent profiles (agents/index.json) carry `folder_path`, so we map
// each legacy folder store onto the agent that owns that folder. If a folder has
// no matching agent (shouldn't happen post ensure_migrated, but be safe), we
// synthesize nothing here — those orphan stores are skipped and left on disk.

use crate::writer::Db;
use rusqlite::params;
use std::path::Path;

/// FNV-1a 64-bit — the SAME hash agents/settings/conversations used for folder
/// keys, so we can find each folder's legacy store directory/file.
fn folder_key(folder: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in folder.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Has the one-time import already run? Stored as a row in a tiny marker table.
fn already_migrated(db: &Db) -> Result<bool, String> {
    let conn = db.reader()?;
    // Create the marker table lazily on the reader is not allowed (read-only
    // intent), so we tolerate "no such table" as "not yet migrated".
    let res: Result<i64, rusqlite::Error> = conn.query_row(
        "SELECT COUNT(*) FROM migration_marker WHERE key = 'migrated_json'",
        [], |r| r.get(0),
    );
    match res {
        Ok(n) => Ok(n > 0),
        Err(_) => Ok(false), // table doesn't exist yet => not migrated
    }
}

fn mark_migrated(db: &Db) -> Result<(), String> {
    db.write(|c| {
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS migration_marker (key TEXT PRIMARY KEY, at INTEGER);",
        ).map_err(|e| format!("marker table: {e}"))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        c.execute(
            "INSERT OR IGNORE INTO migration_marker (key, at) VALUES ('migrated_json', ?1)",
            params![now],
        ).map_err(|e| format!("mark: {e}"))?;
        Ok(())
    })
}

// ── Legacy JSON shapes (minimal mirrors, tolerant of missing fields) ─────────

#[derive(serde::Deserialize)]
struct LegacyAgent {
    id: String,
    #[serde(default)] name: String,
    #[serde(default)] icon: String,
    #[serde(default)] color: String,
    #[serde(default)] folder_path: String,
    #[serde(default)] model: String,
    #[serde(default)] provider: String,
    #[serde(default)] context_mode: String,
    #[serde(default)] system_prompt: String,
    #[serde(default)] created_at: i64,
    #[serde(default)] updated_at: i64,
    #[serde(default)] archived: bool,
}
#[derive(serde::Deserialize, Default)]
struct LegacyIndex {
    #[serde(default)] agents: Vec<LegacyAgent>,
    #[serde(default)] active_id: String,
}
#[derive(serde::Deserialize, Default)]
struct LegacySettings {
    #[serde(default)] model: String,
    #[serde(default)] provider: String,
}
#[derive(serde::Deserialize)]
struct LegacyConv {
    id: String,
    #[serde(default)] title: String,
    #[serde(default)] updated: i64,
    #[serde(default)] pinned: bool,
    #[serde(default)] order: i64,
    #[serde(default)] msgs: serde_json::Value,
    #[serde(default)] history: serde_json::Value,
}

/// Run the one-time import. Safe to call every boot; no-ops after the first.
pub fn run(db: &Db, app_data: &Path) -> Result<(), String> {
    if already_migrated(db)? {
        return Ok(());
    }

    // 1) Agents index → agent + app_state.active_id.
    let idx_path = app_data.join("agents").join("index.json");
    let idx: LegacyIndex = std::fs::read_to_string(&idx_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();

    // Build (folder_path -> agent_id) so we can attach legacy folder stores.
    let mut folder_to_agent: Vec<(String, String)> = Vec::new();

    for a in &idx.agents {
        if !a.folder_path.is_empty() {
            folder_to_agent.push((a.folder_path.clone(), a.id.clone()));
        }
        let a_id = a.id.clone();
        let (name, icon, color) = (a.name.clone(), a.icon.clone(), a.color.clone());
        let (fp, model, provider) = (a.folder_path.clone(), a.model.clone(), a.provider.clone());
        let (cm, sp) = (a.context_mode.clone(), a.system_prompt.clone());
        let (ca, ua, arch) = (a.created_at, a.updated_at, a.archived);
        db.write(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO agent (id,name,icon,color,folder_path,model,provider,context_mode,system_prompt,created_at,updated_at,archived)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![a_id, name,
                    if icon.is_empty() { "🤖".to_string() } else { icon },
                    if color.is_empty() { "#5b8cff".to_string() } else { color },
                    fp, model, provider,
                    if cm.is_empty() { "isolated".to_string() } else { cm },
                    sp, ca, ua, arch as i64],
            ).map_err(|e| format!("import agent: {e}"))?;
            Ok(())
        })?;
    }
    if !idx.active_id.is_empty() {
        let active = idx.active_id.clone();
        db.write(move |c| {
            c.execute("UPDATE app_state SET active_id = ?1 WHERE id = 0", params![active])
                .map_err(|e| format!("import active: {e}"))?;
            Ok(())
        })?;
    }

    // 2) Per-folder settings.json → agent_settings (mapped via folder_to_agent).
    for (folder, agent_id) in &folder_to_agent {
        let sp = app_data.join("settings").join(format!("{}.json", folder_key(folder)));
        if let Some(s) = std::fs::read_to_string(&sp).ok()
            .and_then(|t| serde_json::from_str::<LegacySettings>(&t).ok())
        {
            let (aid, model, provider) = (agent_id.clone(), s.model, s.provider);
            db.write(move |c| {
                c.execute(
                    "INSERT INTO agent_settings (agent_id, model, provider) VALUES (?1,?2,?3)
                     ON CONFLICT(agent_id) DO UPDATE SET model=excluded.model, provider=excluded.provider",
                    params![aid, model, provider],
                ).map_err(|e| format!("import settings: {e}"))?;
                Ok(())
            })?;
        }
    }

    // 3) Per-folder conversations/*.json → conversation (owned by the agent).
    for (folder, agent_id) in &folder_to_agent {
        let dir = app_data.join("conversations").join(folder_key(folder));
        let rd = match std::fs::read_dir(&dir) { Ok(rd) => rd, Err(_) => continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let conv: LegacyConv = match std::fs::read_to_string(&path).ok()
                .and_then(|t| serde_json::from_str(&t).ok())
            { Some(c) => c, None => continue };

            let aid = agent_id.clone();
            let msgs = serde_json::to_string(&conv.msgs).unwrap_or_else(|_| "[]".into());
            let history = serde_json::to_string(&conv.history).unwrap_or_else(|_| "[]".into());
            let (id, title, updated) = (conv.id.clone(), conv.title.clone(), conv.updated);
            let (pinned, ord) = (conv.pinned as i64, conv.order);
            db.write(move |c| {
                c.execute(
                    "INSERT OR IGNORE INTO conversation (id,agent_id,title,updated,pinned,ord,msgs,history)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![id, aid, title, updated, pinned, ord, msgs, history],
                ).map_err(|e| format!("import conv: {e}"))?;
                Ok(())
            })?;
        }
    }

    mark_migrated(db)?;
    eprintln!("[aygent] JSON→SQLite migration complete ({} agents)", idx.agents.len());
    Ok(())
}
