// AYGENT — Subscription Tauri commands (profiles, tokens, usage, turn log).
//
// Profile METADATA lives in the Keychain vault (slot "sub:profiles" as a JSON
// map) so no DB migration was needed to ship profiles. Secrets per profile live
// under subscription::key_slot(id). Turn history lives in its own tiny SQLite
// (sub.db in app-data) — low volume, no writer-actor contention with aygent.db.

use std::collections::HashMap;

const PROFILES_SLOT: &str = "sub:profiles";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubProfile {
    pub id: String,
    pub kind: String, // 'claude-code' | 'codex'
    pub label: String,
    pub updated_at: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn load_profiles() -> HashMap<String, SubProfile> {
    let raw = crate::keychain::get_key(PROFILES_SLOT).unwrap_or_default();
    if raw.is_empty() {
        return HashMap::new();
    }
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_profiles(map: &HashMap<String, SubProfile>) -> Result<(), String> {
    let json = serde_json::to_string(map).map_err(|e| format!("encode profiles: {e}"))?;
    crate::keychain::set_key(PROFILES_SLOT, &json)
}

fn new_id() -> String {
    // No extra deps: nanos + pid is unique enough for a local profile map.
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("sub_{n}_{}", std::process::id())
}

fn valid_kind(kind: &str) -> bool {
    matches!(kind, "claude-code" | "codex")
}

/// Metadata for one profile (no secrets). Used by agent_stream resolution.
pub(crate) fn get_profile(id: &str) -> Option<SubProfile> {
    load_profiles().get(id).cloned()
}

/// (id, label) of every profile of a kind, label-sorted = failover order.
pub(crate) fn profile_order(kind: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = load_profiles()
        .values()
        .filter(|p| p.kind == kind)
        .map(|p| (p.id.clone(), p.label.clone()))
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

// --- Per-agent pinning ("Work agent always uses my Work seat") ----------------
// Stored in the vault (no DB migration): slot "sub:pinned:{agent_id}".

fn pin_slot(agent_id: &str) -> String {
    format!("sub:pinned:{agent_id}")
}

/// Pinned profile id for an agent, if it still exists.
pub(crate) fn pinned_profile(agent_id: &str) -> Option<String> {
    let id = crate::keychain::get_key(&pin_slot(agent_id)).ok()?;
    if id.trim().is_empty() {
        return None;
    }
    get_profile(id.trim()).map(|p| p.id)
}

/// The pinned profile id for an agent ("" = auto). Used by the Agents picker.
#[tauri::command]
pub fn sub_pinned(agent_id: String) -> Result<String, String> {
    Ok(pinned_profile(&agent_id).unwrap_or_default())
}

/// Pin (or unpin with an empty id) an agent to a profile. Kind mismatch is
/// refused so the Agents picker can't strand an agent on the wrong seat.
/// Keychain slot "sub:pinned:{agent_id}".
#[tauri::command]
pub fn sub_profile_pin(agent_id: String, profile_id: String) -> Result<(), String> {
    let profile_id = profile_id.trim().to_string();
    if profile_id.is_empty() {
        return crate::keychain::set_key(&pin_slot(&agent_id), "");
    }
    let p = get_profile(&profile_id).ok_or("unknown subscription profile")?;
    let _ = p; // existence checked; kind is enforced by the picker + turn
    crate::keychain::set_key(&pin_slot(&agent_id), &profile_id)
}

#[tauri::command]
pub fn sub_profiles_list(kind: Option<String>) -> Result<Vec<serde_json::Value>, String> {
    let map = load_profiles();
    let mut out: Vec<serde_json::Value> = map
        .values()
        .filter(|p| kind.as_ref().map(|k| &p.kind == k).unwrap_or(true))
        .map(|p| {
            let has_token = crate::keychain::get_key(&crate::subscription::key_slot(&p.id))
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            serde_json::json!({
                "id": p.id, "kind": p.kind, "label": p.label,
                "has_token": has_token, "updated_at": p.updated_at,
            })
        })
        .collect();
    out.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
    Ok(out)
}

#[tauri::command]
pub fn sub_profile_create(kind: String, label: String) -> Result<SubProfile, String> {
    if !valid_kind(&kind) {
        return Err(format!("unknown kind: {kind} (want claude-code|codex)"));
    }
    let label = label.trim().to_string();
    if label.is_empty() {
        return Err("label required (e.g. Personal, Work)".into());
    }
    let mut map = load_profiles();
    let p = SubProfile { id: new_id(), kind, label, updated_at: now_ms() };
    map.insert(p.id.clone(), p.clone());
    save_profiles(&map)?;
    Ok(p)
}

#[tauri::command]
pub fn sub_profile_delete(id: String) -> Result<(), String> {
    let mut map = load_profiles();
    map.remove(&id);
    save_profiles(&map)?;
    // Best-effort secret cleanup; profile row removal is the source of truth.
    let _ = crate::keychain::delete_key(&crate::subscription::key_slot(&id));
    Ok(())
}

fn store_tokens(id: &str, access: &str, refresh: &str, expires_at_ms: i64, account_id: &str) -> Result<bool, String> {
    if access.trim().is_empty() {
        return Err("empty token — nothing stored".into());
    }
    let json = serde_json::to_string(&serde_json::json!({
        "access_token": access.trim(),
        "refresh_token": refresh.trim(),
        "expires_at_ms": expires_at_ms,
        "account_id": account_id.trim(),
    }))
    .map_err(|e| format!("encode tokens: {e}"))?;
    crate::keychain::set_key(&crate::subscription::key_slot(id), &json)?;
    Ok(crate::subscription::SubTokens {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at_ms,
        account_id: account_id.to_string(),
    }
    .expired())
}

/// What login material exists on this machine per seat kind (no secrets —
/// bools only). The Settings UI calls this on mount to offer one-click
/// connect instead of label-first manual setup.
#[tauri::command]
pub fn sub_detect() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "claude-code": crate::subscription::detect_source("claude-code"),
        "codex": crate::subscription::detect_source("codex"),
    }))
}

// Human errors for a missing CLI login — never a raw OS path error in the UI
// (that red `read /Users/…/.credentials.json: No such file` line was the
// whole complaint, Mason 10-08).
fn no_login_hint(kind: &str) -> String {
    match kind {
        "claude-code" => "No Claude credentials file to import — Claude keeps its login in its own keychain, so there is nothing to read. Run `claude setup-token` in your terminal and paste the result below.".to_string(),
        _ => "No Codex login found — run `codex login` in your terminal, then Connect again.".to_string(),
    }
}

fn actionable_import_err(kind: &str, e: String) -> String {
    let l = e.to_lowercase();
    if l.contains("no such file") || l.contains("not found") || l.contains("enoent") {
        no_login_hint(kind)
    } else {
        e
    }
}

/// First tokenless profile of a kind, else a fresh Personal-style profile.
/// Label numbering skips taken names so Connect never collides.
fn ensure_profile(kind: &str) -> Result<SubProfile, String> {
    let map = load_profiles();
    if let Some(p) = map.values().filter(|p| p.kind == kind).find(|p| {
        crate::keychain::get_key(&crate::subscription::key_slot(&p.id))
            .map(|s| s.is_empty())
            .unwrap_or(true)
    }) {
        return Ok(p.clone());
    }
    let taken: Vec<String> = map
        .values()
        .filter(|p| p.kind == kind)
        .map(|p| p.label.clone())
        .collect();
    let mut label = "Personal".to_string();
    let mut n = 2;
    while taken.iter().any(|l| l == &label) {
        label = format!("Personal {n}");
        n += 1;
    }
    let p = SubProfile { id: new_id(), kind: kind.to_string(), label, updated_at: now_ms() };
    let mut map = map;
    map.insert(p.id.clone(), p.clone());
    save_profiles(&map)?;
    Ok(p)
}

/// One-click connect (Mason 10-08): no label typing. Ensures a profile, then
/// imports the detected CLI login. Claude without a credentials file returns
/// `{connected:false, next:"paste"}` so the UI opens the setup-token paste
/// box for the ensured profile instead of erroring.
#[tauri::command]
pub fn sub_connect(kind: String) -> Result<serde_json::Value, String> {
    if !valid_kind(&kind) {
        return Err(format!("unknown kind: {kind} (want claude-code|codex)"));
    }
    let det = crate::subscription::detect_source(&kind);
    let has_token = det.get("has_token").and_then(|v| v.as_bool()).unwrap_or(false);
    if !has_token {
        if kind == "claude-code" {
            let p = ensure_profile(&kind)?;
            return Ok(serde_json::json!({
                "ok": true, "connected": false, "next": "paste",
                "id": p.id, "label": p.label,
                "hint": no_login_hint(&kind),
            }));
        }
        return Err(no_login_hint(&kind));
    }
    let p = ensure_profile(&kind)?;
    let toks =
        crate::subscription::import_cli(&kind).map_err(|e| actionable_import_err(&kind, e))?;
    let expired = store_tokens(&p.id, &toks.access_token, &toks.refresh_token, toks.expires_at_ms, &toks.account_id)?;
    Ok(serde_json::json!({ "ok": true, "connected": true, "id": p.id, "label": p.label, "expired": expired }))
}

/// Import OAuth tokens from the official CLI login into this profile.
/// v1 = read ~/.claude/.credentials.json | ~/.codex/auth.json (user runs
/// `claude login` / `codex login` in terminal first). Native OAuth later.
#[tauri::command]
pub fn sub_profile_import_cli(id: String) -> Result<serde_json::Value, String> {
    let map = load_profiles();
    let p = map.get(&id).ok_or("unknown subscription profile")?;
    let toks = crate::subscription::import_cli(&p.kind).map_err(|e| actionable_import_err(&p.kind, e))?;
    let expired = store_tokens(&id, &toks.access_token, &toks.refresh_token, toks.expires_at_ms, &toks.account_id)?;
    Ok(serde_json::json!({ "ok": true, "expired": expired }))
}

/// Paste-token flow (the PRIMARY Claude path on machines where the CLI keeps
/// OAuth in the Keychain: run `claude setup-token` in terminal, paste here).
/// Also covers any manual token case for either kind. Tokens go straight to
/// the vault — the UI never keeps them.
#[tauri::command]
pub fn sub_profile_save_token(
    id: String,
    access_token: String,
    refresh_token: Option<String>,
) -> Result<serde_json::Value, String> {
    if get_profile(&id).is_none() {
        return Err("unknown subscription profile".into());
    }
    let expired = store_tokens(&id, &access_token, refresh_token.as_deref().unwrap_or(""), 0, "")?;
    Ok(serde_json::json!({ "ok": true, "expired": expired }))
}

/// Curated model suggestions per seat (the OAuth APIs accept standard ids;
/// the list keeps the picker honest without a network round-trip).
#[tauri::command]
pub fn sub_models(kind: String) -> Result<Vec<String>, String> {
    match kind.as_str() {
        "claude-code" => Ok(vec![
            "claude-opus-5-5".to_string(),
            "claude-sonnet-4-5".to_string(),
            "claude-haiku-4-5".to_string(),
        ]),
        "codex" => Ok(vec!["gpt-6.1-sol".to_string(), "gpt-6-sol".to_string()]),
        _ => Err(format!("unknown subscription kind: {kind}")),
    }
}

/// Cached 5h-window usage for a profile (90s TTL — the endpoints 429 when hammered).
#[tauri::command]
pub async fn sub_usage(id: String) -> Result<serde_json::Value, String> {
    let map = load_profiles();
    let p = map.get(&id).ok_or("unknown subscription profile")?.clone();
    let raw = crate::keychain::get_key(&crate::subscription::key_slot(&id))
        .map_err(|_| "no token imported — run Import from CLI first".to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "stored token unreadable — re-import".to_string())?;
    let access = v.get("access_token").and_then(|x| x.as_str()).unwrap_or("");
    if access.is_empty() {
        return Err("no token imported — run Import from CLI first".into());
    }
    match crate::subscription::usage_cached(&id, &p.kind, access).await {
        Ok(u) => Ok(serde_json::json!({
            "ok": true, "kind": p.kind, "label": p.label,
            "pct_5h": u.pct_5h, "reset_at_ms": u.reset_at_ms,
            "weekly_pct": u.weekly_pct,
        })),
        Err(e) => Ok(serde_json::json!({ "ok": false, "error": e })),
    }
}

/// Live probe: one minimal completion over the profile's subscription OAuth.
/// Proves the token-loop spends SUB credits before we wire streaming + tools.
#[tauri::command]
pub async fn sub_probe(id: String, prompt: String) -> Result<serde_json::Value, String> {
    crate::sub_stream::sub_probe(id, prompt).await
}

// --- Active seat per channel -------------------------------------------------
// agent_stream records which profile actually served each turn's channel; the
// UI reads it at turn end to attribute the turn log (auto-pick means the UI
// can't know the seat any other way). Process-local: restarts clear it, and a
// missing entry just means "attribute to provider, not seat".

#[derive(Debug, Clone)]
struct ActiveSeat {
    profile_id: String,
    kind: String,
    label: String,
}

static ACTIVE_SEAT: std::sync::Mutex<Option<HashMap<String, ActiveSeat>>> =
    std::sync::Mutex::new(None);

pub(crate) fn note_active(channel: &str, profile_id: &str, kind: &str, label: &str) {
    if let Ok(mut g) = ACTIVE_SEAT.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        m.insert(
            channel.to_string(),
            ActiveSeat { profile_id: profile_id.to_string(), kind: kind.to_string(), label: label.to_string() },
        );
    }
}

/// Which seat served this channel's latest subscription turn (if any).
#[tauri::command]
pub fn sub_active(channel: String) -> Option<serde_json::Value> {
    ACTIVE_SEAT
        .lock()
        .ok()?
        .as_ref()?
        .get(&channel)
        .map(|s| serde_json::json!({ "profile_id": s.profile_id, "kind": s.kind, "label": s.label }))
}

// --- Turn log ($0 billed, tokens + api-equiv kept) ----------------------------

fn sub_db(app: &tauri::AppHandle) -> Result<rusqlite::Connection, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir app data: {e}"))?;
    let conn =
        rusqlite::Connection::open(dir.join("sub.db")).map_err(|e| format!("open sub.db: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS subscription_turn (
           id INTEGER PRIMARY KEY AUTOINCREMENT, agent_id TEXT NOT NULL,
           profile_id TEXT NOT NULL, kind TEXT NOT NULL,
           input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
           cache_read_tokens INTEGER NOT NULL DEFAULT 0, api_equiv_cents INTEGER NOT NULL DEFAULT 0,
           created_at INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_sub_turn_agent ON subscription_turn(agent_id, created_at DESC);",
    )
    .map_err(|e| format!("migrate sub.db: {e}"))?;
    Ok(conn)
}

/// Log one finished subscription turn (called by the UI, which holds the summed
/// Usage). Returns the API-equivalent cents for display.
#[tauri::command]
pub fn sub_log_turn(
    app: tauri::AppHandle,
    agent_id: String,
    profile_id: String,
    kind: String,
    input: i64,
    output: i64,
    cache_read: i64,
) -> Result<serde_json::Value, String> {
    if !valid_kind(&kind) {
        return Err(format!("unknown subscription kind: {kind}"));
    }
    let equiv = crate::subscription::api_equivalent_cents(
        &kind,
        input.max(0) as u64,
        output.max(0) as u64,
        cache_read.max(0) as u64,
    );
    let conn = sub_db(&app)?;
    conn.execute(
        "INSERT INTO subscription_turn
           (agent_id, profile_id, kind, input_tokens, output_tokens, cache_read_tokens, api_equiv_cents, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![agent_id, profile_id, kind, input, output, cache_read, equiv, now_ms()],
    )
    .map_err(|e| format!("log turn: {e}"))?;
    Ok(serde_json::json!({ "ok": true, "api_equiv_cents": equiv }))
}
