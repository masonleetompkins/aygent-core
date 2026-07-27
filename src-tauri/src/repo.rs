// AYGENT — Repository layer (M1.1).
//
// Typed CRUD over the SQLite spine. READS use a fresh reader connection (WAL =>
// concurrent, never blocks the writer). WRITES funnel through the single-writer
// actor (writer.rs). This module holds NO connection of its own — it borrows
// `&Db` and picks the right path per operation.
//
// The row types mirror the existing JSON structs (agents.rs / conversations.rs /
// settings.rs) so the UI-facing command shapes don't change: this is a storage
// swap, not a contract change (CONTRACTS.md untouched).

use crate::writer::Db;
use rusqlite::{params, OptionalExtension};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Agents ────────────────────────────────────────────────────────────────

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
    pub context_mode: String,
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

/// App-generated id: time(ms, base36) + small non-crypto random suffix. Same
/// scheme agents.rs used, kept for continuity of existing ids on migration.
fn new_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rand: u32 = {
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

fn row_to_agent(r: &rusqlite::Row) -> rusqlite::Result<AgentProfile> {
    Ok(AgentProfile {
        id: r.get("id")?,
        name: r.get("name")?,
        icon: r.get("icon")?,
        color: r.get("color")?,
        folder_path: r.get("folder_path")?,
        model: r.get("model")?,
        provider: r.get("provider")?,
        context_mode: r.get("context_mode")?,
        system_prompt: r.get("system_prompt")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        archived: r.get::<_, i64>("archived")? != 0,
    })
}

pub fn list_agents(db: &Db) -> Result<Vec<AgentProfile>, String> {
    let conn = db.reader()?;
    let mut stmt = conn
        .prepare("SELECT * FROM agent ORDER BY created_at ASC")
        .map_err(|e| format!("prepare list_agents: {e}"))?;
    let rows = stmt
        .query_map([], row_to_agent)
        .map_err(|e| format!("query list_agents: {e}"))?;
    let mut out = Vec::new();
    for a in rows { out.push(a.map_err(|e| format!("row: {e}"))?); }
    Ok(out)
}

pub fn get_agent(db: &Db, id: &str) -> Result<Option<AgentProfile>, String> {
    let conn = db.reader()?;
    conn.query_row("SELECT * FROM agent WHERE id = ?1", params![id], row_to_agent)
        .optional()
        .map_err(|e| format!("get_agent: {e}"))
}

pub fn active_id(db: &Db) -> Result<String, String> {
    let conn = db.reader()?;
    conn.query_row("SELECT active_id FROM app_state WHERE id = 0", [], |r| r.get(0))
        .map_err(|e| format!("active_id: {e}"))
}

pub fn get_active_agent(db: &Db) -> Result<Option<AgentProfile>, String> {
    let id = active_id(db)?;
    if id.is_empty() { return Ok(None); }
    get_agent(db, &id)
}

#[allow(clippy::too_many_arguments)]
pub fn create_agent(
    db: &Db,
    name: &str, icon: &str, color: &str, folder_path: &str,
    model: &str, provider: &str, context_mode: &str, system_prompt: &str,
) -> Result<AgentProfile, String> {
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
    let p = profile.clone();
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        tx.execute(
            "INSERT INTO agent (id,name,icon,color,folder_path,model,provider,context_mode,system_prompt,created_at,updated_at,archived)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0)",
            params![p.id, p.name, p.icon, p.color, p.folder_path, p.model, p.provider, p.context_mode, p.system_prompt, p.created_at, p.updated_at],
        ).map_err(|e| format!("insert agent: {e}"))?;
        // First agent becomes active; also seed its settings row.
        tx.execute(
            "UPDATE app_state SET active_id = ?1 WHERE id = 0 AND active_id = ''",
            params![p.id],
        ).map_err(|e| format!("seed active: {e}"))?;
        tx.execute(
            "INSERT OR IGNORE INTO agent_settings (agent_id, model, provider) VALUES (?1, ?2, ?3)",
            params![p.id, p.model, p.provider],
        ).map_err(|e| format!("seed settings: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })?;
    Ok(profile)
}

pub fn update_agent(db: &Db, mut profile: AgentProfile) -> Result<(), String> {
    profile.updated_at = now();
    db.write(move |c| {
        let n = c.execute(
            "UPDATE agent SET name=?2,icon=?3,color=?4,folder_path=?5,model=?6,provider=?7,context_mode=?8,system_prompt=?9,updated_at=?10,archived=?11 WHERE id=?1",
            params![profile.id, profile.name, profile.icon, profile.color, profile.folder_path, profile.model, profile.provider, profile.context_mode, profile.system_prompt, profile.updated_at, profile.archived as i64],
        ).map_err(|e| format!("update agent: {e}"))?;
        if n == 0 { return Err("agent not found".into()); }
        Ok(())
    })
}

pub fn delete_agent(db: &Db, id: &str) -> Result<(), String> {
    let id = id.to_string();
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        let n = tx.execute("DELETE FROM agent WHERE id = ?1", params![id])
            .map_err(|e| format!("delete agent: {e}"))?;
        if n == 0 { return Err("agent not found".into()); }
        // If we deleted the active one, fall back to the earliest remaining.
        let active: String = tx.query_row("SELECT active_id FROM app_state WHERE id = 0", [], |r| r.get(0))
            .map_err(|e| format!("read active: {e}"))?;
        if active == id {
            let next: Option<String> = tx.query_row(
                "SELECT id FROM agent ORDER BY created_at ASC LIMIT 1", [], |r| r.get(0),
            ).optional().map_err(|e| format!("next active: {e}"))?;
            tx.execute("UPDATE app_state SET active_id = ?1 WHERE id = 0",
                params![next.unwrap_or_default()])
                .map_err(|e| format!("set active: {e}"))?;
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })
}

pub fn set_active(db: &Db, id: &str) -> Result<(), String> {
    let id = id.to_string();
    db.write(move |c| {
        let exists: bool = c.query_row("SELECT 1 FROM agent WHERE id = ?1", params![id], |_| Ok(true))
            .optional().map_err(|e| format!("check: {e}"))?.unwrap_or(false);
        if !exists { return Err("agent not found".into()); }
        c.execute("UPDATE app_state SET active_id = ?1 WHERE id = 0", params![id])
            .map_err(|e| format!("set active: {e}"))?;
        Ok(())
    })
}

/// Resolve an agentId to its jailed folder path (for the broker scope).
pub fn folder_for(db: &Db, id: &str) -> Result<Option<String>, String> {
    Ok(get_agent(db, id)?.map(|a| a.folder_path).filter(|p| !p.is_empty()))
}

// ── Conversations ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
    pub title: String,
    pub updated: i64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub msgs: serde_json::Value,
    #[serde(default)]
    pub history: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConvMeta {
    pub id: String,
    pub title: String,
    pub updated: i64,
    pub pinned: bool,
    pub order: i64,
}

pub fn list_conversations(db: &Db, agent_id: &str) -> Result<Vec<ConvMeta>, String> {
    let conn = db.reader()?;
    // pinned first, then explicit order (0 = unset sorts last), then newest.
    let mut stmt = conn.prepare(
        "SELECT id,title,updated,pinned,ord FROM conversation WHERE agent_id = ?1
         ORDER BY pinned DESC, (CASE WHEN ord = 0 THEN 1 ELSE 0 END) ASC, ord ASC, updated DESC",
    ).map_err(|e| format!("prepare list_conv: {e}"))?;
    let rows = stmt.query_map(params![agent_id], |r| {
        Ok(ConvMeta {
            id: r.get("id")?,
            title: r.get("title")?,
            updated: r.get("updated")?,
            pinned: r.get::<_, i64>("pinned")? != 0,
            order: r.get("ord")?,
        })
    }).map_err(|e| format!("query list_conv: {e}"))?;
    let mut out = Vec::new();
    for m in rows { out.push(m.map_err(|e| format!("row: {e}"))?); }
    Ok(out)
}

pub fn load_conversation(db: &Db, id: &str) -> Result<Conversation, String> {
    let conn = db.reader()?;
    conn.query_row(
        "SELECT id,agent_id,title,updated,pinned,ord,msgs,history FROM conversation WHERE id = ?1",
        params![id],
        |r| {
            let msgs: String = r.get("msgs")?;
            let history: String = r.get("history")?;
            Ok(Conversation {
                id: r.get("id")?,
                agent_id: r.get("agent_id")?,
                title: r.get("title")?,
                updated: r.get("updated")?,
                pinned: r.get::<_, i64>("pinned")? != 0,
                order: r.get("ord")?,
                msgs: serde_json::from_str(&msgs).unwrap_or(serde_json::json!([])),
                history: serde_json::from_str(&history).unwrap_or(serde_json::json!([])),
            })
        },
    ).map_err(|e| format!("load_conversation: {e}"))
}

/// Save (create or overwrite). Stamps `updated`; PRESERVES existing pinned/order
/// if the incoming payload doesn't carry them (a turn-save must not clobber a
/// pin/reorder) — same rule as conversations.rs::save.
pub fn save_conversation(db: &Db, mut conv: Conversation) -> Result<(), String> {
    conv.updated = now();
    let msgs = serde_json::to_string(&conv.msgs).map_err(|e| format!("ser msgs: {e}"))?;
    let history = serde_json::to_string(&conv.history).map_err(|e| format!("ser history: {e}"))?;
    db.write(move |c| {
        // Preserve prior pinned/order when the caller left them unset.
        let prev: Option<(i64, i64)> = c.query_row(
            "SELECT pinned, ord FROM conversation WHERE id = ?1",
            params![conv.id], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().map_err(|e| format!("prev: {e}"))?;
        let (pinned, ord) = match prev {
            Some((p, o)) => (
                if conv.pinned { 1 } else { p },
                if conv.order == 0 { o } else { conv.order },
            ),
            None => (conv.pinned as i64, conv.order),
        };
        c.execute(
            "INSERT INTO conversation (id,agent_id,title,updated,pinned,ord,msgs,history)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET
               agent_id=excluded.agent_id, title=excluded.title, updated=excluded.updated,
               pinned=excluded.pinned, ord=excluded.ord, msgs=excluded.msgs, history=excluded.history",
            params![conv.id, conv.agent_id, conv.title, conv.updated, pinned, ord, msgs, history],
        ).map_err(|e| format!("save conv: {e}"))?;
        Ok(())
    })
}

pub fn reorder_conversations(db: &Db, updates: Vec<(String, bool, i64)>) -> Result<(), String> {
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        for (id, pinned, order) in &updates {
            tx.execute(
                "UPDATE conversation SET pinned = ?2, ord = ?3 WHERE id = ?1",
                params![id, *pinned as i64, order],
            ).map_err(|e| format!("reorder: {e}"))?;
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })
}

pub fn delete_conversation(db: &Db, id: &str) -> Result<(), String> {
    let id = id.to_string();
    db.write(move |c| {
        c.execute("DELETE FROM conversation WHERE id = ?1", params![id])
            .map_err(|e| format!("delete conv: {e}"))?;
        Ok(())
    })
}

// ── Per-agent settings ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AgentSettings {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider: String,
}

pub fn load_settings(db: &Db, agent_id: &str) -> Result<AgentSettings, String> {
    let conn = db.reader()?;
    let s = conn.query_row(
        "SELECT model, provider FROM agent_settings WHERE agent_id = ?1",
        params![agent_id],
        |r| Ok(AgentSettings { model: r.get(0)?, provider: r.get(1)? }),
    ).optional().map_err(|e| format!("load_settings: {e}"))?;
    Ok(s.unwrap_or_default())
}

pub fn save_settings(db: &Db, agent_id: &str, s: AgentSettings) -> Result<(), String> {
    let agent_id = agent_id.to_string();
    db.write(move |c| {
        c.execute(
            "INSERT INTO agent_settings (agent_id, model, provider) VALUES (?1, ?2, ?3)
             ON CONFLICT(agent_id) DO UPDATE SET model = excluded.model, provider = excluded.provider",
            params![agent_id, s.model, s.provider],
        ).map_err(|e| format!("save_settings: {e}"))?;
        Ok(())
    })
}
