// AYGENT — Connections (M1.9, Atlas CONNECTIONS-ARCH). A Connection is a
// keychain-backed bearer credential + NON-secret metadata, exposed to an agent
// as Rust-side token-attached tools. The jailed daemon NEVER holds a raw token;
// the privileged Rust side attaches it to the outbound API call at call time —
// same trust model as the path broker + fetch_url, applied to authenticated APIs.
//
// NOW REGISTRY-DRIVEN (see connectors.rs): this module owns CREDENTIALS —
// keychain storage, per-agent account resolution, read/write access mode. The
// per-provider HTTP details live in connector descriptors, and one generic
// executor (connector_exec.rs) runs them. Adding a provider touches neither.
//
// PER-AGENT ACCOUNTS: multiple accounts per provider coexist (a personal GitHub
// and a work GitHub), distinguished by `nickname` and namespaced in the keychain
// by `key_ref`. At most ONE may be enabled per (agent, provider) — enforced by a
// unique index in schema v10, because a wrong-credential bug does not fail
// loudly, it succeeds against the wrong account.

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

/// SELF-HOSTED BUILD: resolve a GitHub PAT + login for `git push`/`git pull`.
/// Prefers a connection enabled for `agent_id`; falls back to ANY connected
/// GitHub connection (Mason's personal harness — one login is the norm). Returns
/// (token, login). Used by github_git_auth to seed the osxkeychain git helper.
pub fn resolve_github_push_token(db: &Db, agent_id: Option<&str>) -> Result<(String, String), String> {
    // NO CROSS-AGENT FALLBACK. This used to fall back to "any connected GitHub
    // account" when the per-agent lookup missed — which, once a work account
    // exists alongside a personal one, means an agent could push with a token it
    // was never granted. A wrong-credential bug does not fail loudly, it
    // SUCCEEDS against the wrong account, so this refuses instead of guessing.
    let aid = agent_id.ok_or(
        "no agent identity for this git operation — cannot choose a GitHub account safely",
    )?;
    let (token, cid) = token_for_agent_provider(db, aid, "github")?;
    let login = {
        let conn = db.reader()?;
        let key_ref: String = conn
            .query_row("SELECT key_ref FROM connection WHERE id = ?1", params![cid], |r| r.get(0))
            .map_err(|e| format!("resolve github login: {e}"))?;
        key_ref.strip_prefix("github-").unwrap_or(&key_ref).to_string()
    };
    Ok((token, login))
}

/// Resolve the live bearer token for a provider enabled on this agent (reads the
/// keychain via the connection's key_ref). Returns (token, connection_id).
fn token_for_agent_provider(db: &Db, agent_id: &str, provider: &str) -> Result<(String, i64), String> {
    let (id, key_ref): (i64, String) = {
        let conn = db.reader()?;
        conn.query_row(
            "SELECT c.id, c.key_ref FROM connection c JOIN agent_connection ac ON ac.connection_id = c.id
             WHERE c.provider = ?1 AND c.status = 'connected' AND ac.agent_id = ?2 AND ac.enabled = 1
             ORDER BY c.updated_at DESC, c.id DESC LIMIT 1",
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
// CONNECTIONS v2 — per-agent accounts, access mode, generic credentials.
// Used by connector_exec for every registry-driven provider. The functions
// above are the GitHub-specific originals (kept: git push auth needs them).
// ---------------------------------------------------------------------------

/// The access mode ('read' | 'write') for this agent's enabled connection to
/// `provider`. Errors if there is no enabled connection — callers treat that as
/// "the tool isn't available", which is the correct fail-closed behavior.
pub fn access_for_agent(db: &Db, agent_id: &str, provider: &str) -> Result<String, String> {
    let conn = db.reader()?;
    conn.query_row(
        "SELECT COALESCE(ac.access_mode,'read') FROM connection c
         JOIN agent_connection ac ON ac.connection_id = c.id
         WHERE c.provider = ?1 AND c.status = 'connected'
           AND ac.agent_id = ?2 AND ac.enabled = 1
         ORDER BY c.updated_at DESC, c.id DESC LIMIT 1",
        params![provider, agent_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("access mode: {e}"))?
    .ok_or_else(|| format!("no {provider} connection is enabled for this agent"))
}

/// Set read/write mode for one (agent, connection) pair.
pub fn set_access_mode(db: &Db, agent_id: &str, connection_id: i64, write: bool) -> Result<(), String> {
    let (a, c) = (agent_id.to_string(), connection_id);
    let mode = if write { "write" } else { "read" };
    db.write(move |conn| {
        let n = conn.execute(
            "UPDATE agent_connection SET access_mode = ?3 WHERE agent_id = ?1 AND connection_id = ?2",
            params![a, c, mode],
        ).map_err(|e| format!("set access_mode: {e}"))?;
        if n == 0 {
            return Err("enable this connection for the agent first".to_string());
        }
        Ok(())
    })
}

/// Resolve ALL credential material for this agent's enabled connection to
/// `provider`: secret fields from the keychain + non-secret fields from
/// config_json. Multi-field by design (Supabase needs URL + key).
pub fn creds_for_agent(
    db: &Db,
    agent_id: &str,
    provider: &str,
) -> Result<crate::connector_exec::Creds, String> {
    let (key_ref, config_json): (String, String) = {
        let conn = db.reader()?;
        conn.query_row(
            "SELECT c.key_ref, c.config_json FROM connection c
             JOIN agent_connection ac ON ac.connection_id = c.id
             WHERE c.provider = ?1 AND c.status = 'connected'
               AND ac.agent_id = ?2 AND ac.enabled = 1
             ORDER BY c.updated_at DESC, c.id DESC LIMIT 1",
            params![provider, agent_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("resolve connection: {e}"))?
        .ok_or_else(|| format!("no {provider} connection is enabled for this agent"))?
    };

    let mut fields = serde_json::Map::new();
    // Non-secret config first (project URLs, account ids).
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&config_json) {
        for (k, v) in map { fields.insert(k, v); }
    }
    // Then the secrets, from the keychain, keyed by the descriptor's field names.
    let def = crate::connectors::by_id(provider)
        .ok_or_else(|| format!("unknown connector `{provider}`"))?;
    for f in def.auth_fields {
        if !f.secret { continue; }
        let val = read_token(&key_ref, f.key)
            .or_else(|_| read_token(&key_ref, "token")) // legacy GitHub slot
            .map_err(|_| format!(
                "the saved {} credential couldn't be read from your keychain — reconnect it in Connections.",
                def.label
            ))?;
        fields.insert(f.key.to_string(), serde_json::Value::String(val));
    }
    Ok(crate::connector_exec::Creds { fields })
}

/// Which providers are enabled for this agent, with their access mode. Drives
/// tool-list assembly in the agent loop (one query instead of N probes).
pub fn enabled_providers_for_agent(db: &Db, agent_id: &str) -> Vec<(String, String)> {
    let Ok(conn) = db.reader() else { return vec![] };
    let Ok(mut stmt) = conn.prepare(
        "SELECT c.provider, COALESCE(ac.access_mode,'read') FROM connection c
         JOIN agent_connection ac ON ac.connection_id = c.id
         WHERE c.status = 'connected' AND ac.agent_id = ?1 AND ac.enabled = 1",
    ) else { return vec![] };
    let rows = stmt.query_map(params![agent_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    });
    match rows { Ok(it) => it.flatten().collect(), Err(_) => vec![] }
}

/// Generic connect for any registry connector: validate the credential, learn
/// which ACCOUNT it belongs to, store secrets in the keychain, upsert the row.
/// `values` maps auth-field key -> user-entered value.
pub async fn connect_connector(
    db: &Db,
    provider: &str,
    nickname: &str,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<(i64, String), String> {
    let def = crate::connectors::by_id(provider)
        .ok_or_else(|| format!("unknown connector `{provider}`"))?;

    for f in def.auth_fields {
        let v = values.get(f.key).and_then(|v| v.as_str()).unwrap_or("").trim();
        if v.is_empty() {
            return Err(format!("{} is required.", f.label));
        }
    }
    let ctx = serde_json::Value::Object(values.clone());

    // Validate against the live API so a bad credential is caught at PASTE time,
    // not on the agent's first call an hour later.
    let account = validate_credential(def, &ctx).await?;

    // key_ref namespaces the keychain per ACCOUNT, so a personal and a work
    // token for the same provider never collide.
    let slug: String = account
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect();
    let key_ref = format!("{provider}-{}", slug.trim_matches('-'));

    let mut config = serde_json::Map::new();
    for f in def.auth_fields {
        let v = values.get(f.key).and_then(|v| v.as_str()).unwrap_or("").trim();
        if f.secret {
            store_token(&key_ref, f.key, v)?;
        } else {
            config.insert(f.key.to_string(), serde_json::Value::String(v.to_string()));
        }
    }

    let nick = if nickname.trim().is_empty() { account.clone() } else { nickname.trim().to_string() };
    let label = format!("{} ({})", def.label, nick);
    let (prov, kr, lbl, acct, nk) =
        (provider.to_string(), key_ref.clone(), label, account.clone(), nick);
    let auth_kind = def.auth_kind.to_string();
    let cfg = serde_json::to_string(&serde_json::Value::Object(config)).unwrap_or_else(|_| "{}".into());

    let id = db.write(move |conn| {
        // Same account reconnecting = replace (a token refresh, not a new account).
        conn.execute("DELETE FROM connection WHERE provider=?1 AND key_ref=?2", params![prov, kr])
            .map_err(|e| format!("clear old: {e}"))?;
        conn.execute(
            "INSERT INTO connection (provider,kind,auth_kind,label,account,nickname,scopes,config_json,key_ref,status,created_at,updated_at)
             VALUES (?1,'api',?2,?3,?4,?5,'',?6,?7,'connected',?8,?8)",
            params![prov, auth_kind, lbl, acct, nk, cfg, kr, now()],
        ).map_err(|e| format!("insert conn: {e}"))?;
        Ok(conn.last_insert_rowid())
    })?;
    Ok((id, account))
}

/// Run the descriptor's validation call and return the account identity.
async fn validate_credential(
    def: &crate::connectors::Connector,
    ctx: &serde_json::Value,
) -> Result<String, String> {
    let Some(v) = def.validate.as_ref() else {
        return Ok(def.label.to_string());
    };
    let client = reqwest::Client::builder()
        .user_agent("AYGENT/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let url = crate::connectors::fill(v.url, ctx);
    let method = reqwest::Method::from_bytes(v.method.as_bytes())
        .map_err(|_| "bad validate method".to_string())?;
    let mut req = client.request(method, &url);
    if !def.auth_header.is_empty() {
        req = req.header(def.auth_header, crate::connectors::fill(def.auth_value, ctx));
    }
    for (k, val) in def.headers {
        req = req.header(*k, crate::connectors::fill(val, ctx));
    }
    if !v.body.is_empty() {
        let body: serde_json::Value = serde_json::from_str(&crate::connectors::fill(v.body, ctx))
            .map_err(|e| format!("validate body: {e}"))?;
        req = req.json(&body);
    }
    let resp = req.send().await.map_err(|e| format!("{} request: {e}", def.label))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(format!(
            "{} rejected that credential (401). Check it's valid and not expired.",
            def.label
        ));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        // 403 means the credential is real but under-permissioned — a different
        // fix than a bad token, so don't collapse them into one message.
        return Err(format!(
            "{} accepted the credential but refused this request (403) — it's probably missing a \
             permission. Re-check the scopes in the setup steps.",
            def.label
        ));
    }
    if !status.is_success() {
        return Err(format!("{} validation failed: HTTP {}", def.label, status.as_u16()));
    }
    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if v.identity_path.is_empty() {
        return Ok(def.label.to_string());
    }
    let ident = crate::connectors::dig(&body, v.identity_path)
        .and_then(|x| match x {
            serde_json::Value::String(s) => Some(s.clone()),
            other if !other.is_null() => Some(other.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| def.label.to_string());
    Ok(ident)
}
