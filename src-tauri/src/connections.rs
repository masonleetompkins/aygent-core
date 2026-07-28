// AYGENT — Connections (M1.9, Atlas CONNECTIONS-ARCH). A Connection is a
// keychain-backed bearer credential + NON-secret metadata, exposed to an agent
// as Rust-side token-attached tools. The jailed daemon NEVER holds a raw token;
// the privileged Rust side attaches it to the outbound API call at call time —
// same trust model as the path broker + fetch_url, applied to authenticated APIs.
//
// SLICE 1 SCOPE (GitHub-first, the smallest end-to-end proof):
//   connect via PAT paste -> validate GET /user -> store token in keychain ->
//   github_list_prs tool (token-attached, Rust-side) -> enable per agent ->
//   agent uses it live. No OAuth loopback, no verification: fastest shape proof.
//
// Per Atlas: a PER-PROVIDER AUTH ADAPTER. GitHub = pat (no loopback/refresh).
// Google = oauth_pkce (Slice 3). auth_kind drives connect/refresh/revoke.

use crate::writer::Db;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// KEYCHAIN token storage. key_ref is a POINTER base; concrete keychain accounts
// are derived by suffix (Google needs :access + :refresh; GitHub just :token).
// Never store a token in SQLite/config_json.
// ---------------------------------------------------------------------------

/// The keychain account for a connection's credential slot (e.g. "token",
/// "access", "refresh"). Namespaced so it can't collide with provider API keys.
fn kc_account(key_ref: &str, slot: &str) -> String {
    format!("conn:{key_ref}:{slot}")
}

fn store_token(key_ref: &str, slot: &str, token: &str) -> Result<(), String> {
    crate::keychain::set_key(&kc_account(key_ref, slot), token)
}
fn read_token(key_ref: &str, slot: &str) -> Result<String, String> {
    crate::keychain::get_key(&kc_account(key_ref, slot))
}
fn delete_token(key_ref: &str, slot: &str) {
    let _ = crate::keychain::delete_key(&kc_account(key_ref, slot));
}

// ---------------------------------------------------------------------------
// The Connection row (non-secret view for the UI).
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Clone)]
pub struct ConnectionRow {
    pub id: i64,
    pub provider: String,
    pub kind: String,
    pub auth_kind: String,
    pub label: String,
    pub account: Option<String>,
    pub scopes: Option<String>,
    pub status: String,
}

/// List all connections (non-secret metadata) for the Connections catalog.
pub fn list(db: &Db) -> Result<Vec<ConnectionRow>, String> {
    let conn = db.reader()?;
    let mut stmt = conn
        .prepare("SELECT id, provider, kind, auth_kind, label, account, scopes, status FROM connection ORDER BY created_at ASC")
        .map_err(|e| format!("prep list: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ConnectionRow {
                id: r.get(0)?,
                provider: r.get(1)?,
                kind: r.get(2)?,
                auth_kind: r.get(3)?,
                label: r.get(4)?,
                account: r.get(5)?,
                scopes: r.get(6)?,
                status: r.get(7)?,
            })
        })
        .map_err(|e| format!("query list: {e}"))?;
    let mut out = Vec::new();
    for row in rows.flatten() { out.push(row); }
    Ok(out)
}

/// Is `provider` connected AND enabled for this agent? Drives whether the
/// provider's tools appear in the agent's tool list.
pub fn provider_enabled_for_agent(db: &Db, agent_id: &str, provider: &str) -> bool {
    let Ok(conn) = db.reader() else { return false };
    conn.query_row(
        "SELECT 1 FROM connection c JOIN agent_connection ac ON ac.connection_id = c.id
         WHERE c.provider = ?1 AND c.status = 'connected' AND ac.agent_id = ?2 AND ac.enabled = 1
         LIMIT 1",
        params![provider, agent_id],
        |_| Ok(true),
    ).optional().ok().flatten().unwrap_or(false)
}

/// Resolve the live bearer token for a provider enabled on this agent (reads the
/// keychain via the connection's key_ref). Returns (token, connection_id).
fn token_for_agent_provider(db: &Db, agent_id: &str, provider: &str) -> Result<(String, i64), String> {
    let (id, key_ref): (i64, String) = {
        let conn = db.reader()?;
        conn.query_row(
            "SELECT c.id, c.key_ref FROM connection c JOIN agent_connection ac ON ac.connection_id = c.id
             WHERE c.provider = ?1 AND c.status = 'connected' AND ac.agent_id = ?2 AND ac.enabled = 1
             LIMIT 1",
            params![provider, agent_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().map_err(|e| format!("resolve conn: {e}"))?
         .ok_or_else(|| format!("no enabled {provider} connection for this agent"))?
    };
    let token = read_token(&key_ref, "token")?;
    Ok((token, id))
}

/// Enable/disable a connection for an agent (per-agent toggle).
pub fn set_agent_enabled(db: &Db, agent_id: &str, connection_id: i64, enabled: bool) -> Result<(), String> {
    let (a, c) = (agent_id.to_string(), connection_id);
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO agent_connection (agent_id, connection_id, enabled) VALUES (?1,?2,?3)
             ON CONFLICT(agent_id, connection_id) DO UPDATE SET enabled = excluded.enabled",
            params![a, c, if enabled { 1 } else { 0 }],
        ).map_err(|e| format!("set agent_connection: {e}"))?;
        Ok(())
    })
}

/// Which connections are enabled for an agent (ids) — for the per-agent UI.
pub fn enabled_ids_for_agent(db: &Db, agent_id: &str) -> Result<Vec<i64>, String> {
    let conn = db.reader()?;
    let mut stmt = conn
        .prepare("SELECT connection_id FROM agent_connection WHERE agent_id = ?1 AND enabled = 1")
        .map_err(|e| format!("prep: {e}"))?;
    let rows = stmt.query_map(params![agent_id], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("query: {e}"))?;
    Ok(rows.flatten().collect())
}

/// Disconnect: delete the row (cascades agent_connection) + wipe keychain slots.
pub fn disconnect(db: &Db, id: i64) -> Result<(), String> {
    let key_ref: Option<String> = {
        let conn = db.reader()?;
        conn.query_row("SELECT key_ref FROM connection WHERE id = ?1", params![id], |r| r.get(0))
            .optional().map_err(|e| format!("lookup: {e}"))?
    };
    if let Some(kr) = key_ref {
        for slot in ["token", "access", "refresh"] { delete_token(&kr, slot); }
    }
    db.write(move |conn| {
        conn.execute("DELETE FROM connection WHERE id = ?1", params![id])
            .map_err(|e| format!("delete conn: {e}"))?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// GITHUB adapter (auth_kind = 'pat'). Validate the PAT, capture identity, store.
// ---------------------------------------------------------------------------

/// Connect GitHub via a fine-grained/classic PAT. Validates with GET /user,
/// captures login + granted scopes, stores the token in the keychain, upserts
/// the connection row. Returns the new/updated connection id + resolved login.
pub async fn connect_github_pat(db: &Db, token: &str) -> Result<(i64, String), String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("paste a GitHub token".into());
    }
    // Validate against the real API (also captures identity + scopes).
    let client = reqwest::Client::builder()
        .user_agent("AYGENT/0.1")
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("github request: {e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("GitHub rejected that token (401). Check it's valid and not expired.".into());
    }
    if !resp.status().is_success() {
        return Err(format!("GitHub validation failed: HTTP {}", resp.status()));
    }
    // Classic PATs report scopes in this header; fine-grained PATs don't (their
    // permissions aren't a simple scope list) — that's fine, we store what we get.
    let scopes = resp
        .headers()
        .get("x-oauth-scopes")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("github decode: {e}"))?;
    let login = body.get("login").and_then(|l| l.as_str()).unwrap_or("").to_string();
    if login.is_empty() {
        return Err("GitHub returned no user login — token may lack read:user".into());
    }

    // key_ref = a stable base derived from provider + login. Store the token.
    let key_ref = format!("github-{login}");
    store_token(&key_ref, "token", &token)?;

    // Upsert the connection row (one GitHub connection per login).
    let label = format!("GitHub (@{login})");
    let account = format!("@{login}");
    let (kr, lbl, acct, scp) = (key_ref.clone(), label, account, scopes);
    let id = db.write(move |conn| {
        // Replace an existing github connection for the same key_ref.
        conn.execute("DELETE FROM connection WHERE provider='github' AND key_ref=?1", params![kr])
            .map_err(|e| format!("clear old: {e}"))?;
        conn.execute(
            "INSERT INTO connection (provider,kind,auth_kind,label,account,scopes,config_json,key_ref,status,created_at,updated_at)
             VALUES ('github','api','pat',?1,?2,?3,'{}',?4,'connected',?5,?5)",
            params![lbl, acct, scp, kr, now()],
        ).map_err(|e| format!("insert conn: {e}"))?;
        Ok(conn.last_insert_rowid())
    })?;
    Ok((id, login))
}

// ---------------------------------------------------------------------------
// GITHUB TOOLS (Rust-side, token-attached). Slice 1 = github_list_prs.
// ---------------------------------------------------------------------------

/// List the user's open pull requests (authored by them, across all repos) via
/// the GitHub search API. Token attached Rust-side; the jailed brain only gets
/// the formatted result. Returns (text, is_error).
pub async fn github_list_prs(db: &Db, agent_id: &str) -> (String, bool) {
    let (token, _cid) = match token_for_agent_provider(db, agent_id, "github") {
        Ok(t) => t,
        Err(e) => return (e, true),
    };
    let client = match reqwest::Client::builder().user_agent("AYGENT/0.1").build() {
        Ok(c) => c,
        Err(e) => return (format!("http: {e}"), true),
    };
    // Open PRs authored by the authenticated user, newest first.
    let url = "https://api.github.com/search/issues?q=is:open+is:pr+author:@me&sort=updated&order=desc&per_page=20";
    let resp = match client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (format!("github request: {e}"), true),
    };
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return ("GitHub token was rejected (401) — reconnect GitHub in Connections.".into(), true);
    }
    if !resp.status().is_success() {
        return (format!("GitHub error: HTTP {}", resp.status()), true);
    }
    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => return (format!("github decode: {e}"), true),
    };
    let items = body.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
    if items.is_empty() {
        return ("You have no open pull requests.".into(), false);
    }
    let mut out = format!("Your open pull requests ({}):\n", items.len());
    for it in &items {
        let title = it.get("title").and_then(|t| t.as_str()).unwrap_or("(untitled)");
        let num = it.get("number").and_then(|n| n.as_i64()).unwrap_or(0);
        let html_url = it.get("html_url").and_then(|u| u.as_str()).unwrap_or("");
        // Derive repo "owner/name" from the html_url or repository_url.
        let repo = it
            .get("repository_url")
            .and_then(|u| u.as_str())
            .and_then(|u| u.strip_prefix("https://api.github.com/repos/"))
            .unwrap_or("");
        out.push_str(&format!("- {repo}#{num}: {title}\n  {html_url}\n"));
    }
    (out.trim_end().to_string(), false)
}
