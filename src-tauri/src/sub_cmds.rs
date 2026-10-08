// AYGENT — Subscription Tauri commands (slice 2, UNWIRED).
//
// NOT YET COMPILED IN: add to lib.rs `mod sub_cmds;` + `mod subscription;`
// and register these fns in invoke_handler. Landing the file first keeps the
// slice reviewable without rewriting 300KB lib.rs over the API.
//
// v1 stores profile METADATA in the Keychain vault too (slot "sub:profiles"
// as a JSON map) so slice 2 needs NO db migration. Secrets per profile live
// under subscription::key_slot(id). The v16 tables (subscription_turn history)
// land when agent_stream wiring happens.

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

/// Import OAuth tokens from the official CLI login into this profile.
/// v1 = read ~/.claude/.credentials.json | ~/.codex/auth.json (user runs
/// `claude login` / `codex login` in terminal first). Native OAuth later.
#[tauri::command]
pub fn sub_profile_import_cli(id: String) -> Result<serde_json::Value, String> {
    let map = load_profiles();
    let p = map.get(&id).ok_or("unknown subscription profile")?;
    let toks = crate::subscription::import_cli(&p.kind)?;
    let json = serde_json::to_string(&serde_json::json!({
        "access_token": toks.access_token,
        "refresh_token": toks.refresh_token,
        "expires_at_ms": toks.expires_at_ms,
    }))
    .map_err(|e| format!("encode tokens: {e}"))?;
    crate::keychain::set_key(&crate::subscription::key_slot(&id), &json)?;
    Ok(serde_json::json!({ "ok": true, "expired": toks.expired() }))
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
