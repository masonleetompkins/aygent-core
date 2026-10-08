// AYGENT — Subscription providers (Claude Code + Codex).
//
// WHY: use Claude Pro/Max and ChatGPT Plus/Pro SUBSCRIPTIONS instead of
// metered API keys. Same agent loop + same StreamEvents — only auth and
// billing differ. Billed $0, tokens + api-equivalent still logged.
//
// AUTH: OAuth tokens live in the Keychain vault (never files/DB). Imported
// from the official CLIs the user already trusts:
//   Claude: ~/.claude/.credentials.json (claude login)
//   Codex:  ~/.codex/auth.json (codex login)
// Multi-profile: personal/work/etc — N profiles per kind, failover in order.
//
// USAGE: real 5h-window polling, CACHED 90s (both endpoints 429 themselves
// if hammered):
//   Claude: GET api.anthropic.com/api/oauth/usage (Bearer + oauth-2025-04-20)
//   Codex:  GET chatgpt.com/backend-api/wham/usage (Bearer ChatGPT OAuth)
// This module is pure logic — Tauri commands wire in lib.rs next.

use std::collections::HashMap;
use std::sync::Mutex;

pub const CLAUDE_KIND: &str = "claude-code";
pub const CODEX_KIND: &str = "codex";

/// True for subscription providers (billing_mode=subscription, $0 billed).
pub fn is_subscription_provider(provider: &str) -> bool {
    matches!(provider, "claude-code" | "codex")
}

/// Keychain slot for a profile's OAuth secret.
pub fn key_slot(profile_id: &str) -> String {
    format!("sub:{profile_id}")
}

#[derive(Debug, Clone)]
pub struct SubTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
    pub account_id: String,
}

impl SubTokens {
    pub fn expired(&self) -> bool {
        if self.expires_at_ms <= 0 {
            return false; // unknown expiry = assume usable, refresh on 401
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        now + 60_000 > self.expires_at_ms
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn read_json(path: &std::path::Path) -> Result<serde_json::Value, String> {
    let b = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn s(v: &serde_json::Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(x) = v.get(*k).and_then(|x| x.as_str()) {
            if !x.is_empty() {
                return x.to_string();
            }
        }
    }
    String::new()
}

fn i64_at(v: &serde_json::Value, keys: &[&str]) -> i64 {
    for k in keys {
        if let Some(n) = v.get(*k).and_then(|x| x.as_i64()) {
            return n;
        }
        // seconds -> ms heuristic for small values
        if let Some(n) = v.get(*k).and_then(|x| x.as_u64()) {
            let n = n as i64;
            return if n > 0 && n < 10_000_000_000 { n * 1000 } else { n };
        }
    }
    0
}

/// Parse Claude CLI credentials (tolerant — shape drifts across versions).
/// Looks for access/refresh under claudeAiOauth, oauth, or top level.
pub fn import_claude_cli() -> Result<SubTokens, String> {
    let home = home_dir().ok_or("no HOME".to_string())?;
    let p = home.join(".claude/.credentials.json");
    let v = read_json(&p)?;
    let cands: Vec<serde_json::Value> = vec![
        v.get("claudeAiOauth").cloned().unwrap_or(serde_json::Value::Null),
        v.get("oauth").cloned().unwrap_or(serde_json::Value::Null),
        v.clone(),
    ];
    for c in &cands {
        let access = s(c, &["accessToken", "access_token"]);
        if access.is_empty() {
            continue;
        }
        return Ok(SubTokens {
            access_token: access,
            refresh_token: s(c, &["refreshToken", "refresh_token"]),
            expires_at_ms: i64_at(c, &["expiresAt", "expires_at", "expiresAtMs"]),
            account_id: String::new(),
        });
    }
    Err(format!("no OAuth token in {}", p.display()))
}

/// Parse Codex CLI auth (tolerant). Prefers ChatGPT OAuth over API key.
pub fn import_codex_cli() -> Result<SubTokens, String> {
    let home = home_dir().ok_or("no HOME".to_string())?;
    let p = home.join(".codex/auth.json");
    let v = read_json(&p)?;
    let t = v.get("tokens").cloned().unwrap_or(v.clone());
    let access = s(&t, &["access_token", "accessToken", "id_token"]);
    if access.is_empty() {
        return Err(format!("no OAuth token in {} (auth_mode={})", p.display(),
            v.get("auth_mode").and_then(|x| x.as_str()).unwrap_or("?")));
    }
    Ok(SubTokens {
        access_token: access,
        refresh_token: s(&t, &["refresh_token", "refreshToken"]),
        expires_at_ms: i64_at(&t, &["expires_at", "expiresAt"]),
        account_id: {
            let a = s(&t, &["account_id"]);
            if a.is_empty() { s(&v, &["account_id"]) } else { a }
        },
    })
}

// --- Auto-detect (Mason 10-08): what login material exists on this machine? ---
// No secrets — bools only. Drives the one-click connect UI so nobody types a
// label before connecting. Note Claude usually has NO file (its CLI keeps
// OAuth in its own Keychain, unreadable to us) — that case resolves to the
// `claude setup-token` paste flow, not Import.

fn cli_file(kind: &str) -> Option<std::path::PathBuf> {
    let home = home_dir()?;
    match kind {
        "claude-code" => Some(home.join(".claude/.credentials.json")),
        "codex" => Some(home.join(".codex/auth.json")),
        _ => None,
    }
}

fn cli_binary(kind: &str) -> &'static str {
    match kind {
        "claude-code" => "claude",
        "codex" => "codex",
        _ => "",
    }
}

/// Is `bin` executable somewhere visible (PATH + the usual Homebrew/local bins
/// GUI apps inherit a thin PATH, so check the common homes explicitly).
fn binary_found(bin: &str) -> bool {
    if bin.is_empty() {
        return false;
    }
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local/bin"));
    }
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    dirs.iter().any(|d| d.join(bin).is_file())
}

/// What login material exists for a seat kind. Bools only — never secrets.
/// `has_token` reuses the real import parsers, so detect agrees with import.
pub fn detect_source(kind: &str) -> serde_json::Value {
    let file_found = cli_file(kind).map(|p| p.is_file()).unwrap_or(false);
    let has_token = import_cli(kind)
        .map(|t| !t.access_token.is_empty())
        .unwrap_or(false);
    serde_json::json!({
        "file_found": file_found,
        "has_token": has_token,
        "cli_found": binary_found(cli_binary(kind)),
    })
}

// --- Claude Code wire identity (Mason 10-08) ---------------------------------
// Anthropic classifies subscription-token (sk-ant-oat*) traffic: requests that
// do not look like Claude Code get throttled or refused. Shape mirrors the
// working opencode-claude-subscription plugin (MIT): Claude Code UA + x-app,
// both OAuth betas, direct-browser-access flag, and a per-request session id.
// (Tool-name aliasing from that plugin is OpenCode-specific; our probe sends
// no tools, so headers are the whole fix. Identity system block is the
// documented fallback if classification persists.)
const CLAUDE_UA: &str = "claude-cli/2.1.295 (external, cli)";
const CLAUDE_BETAS: &str = "claude-code-20250219,oauth-2025-04-20";

fn new_session_uuid() -> String {
    let mut b = [0u8; 16];
    rand::Rng::fill(&mut rand::thread_rng(), &mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11],
        b[12], b[13], b[14], b[15]
    )
}

/// Apply the Claude Code request shape to an outgoing call (auth left to caller).
/// Use on EVERY Claude OAuth call: complete, stream, usage.
pub fn apply_claude_headers(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    req.header("User-Agent", CLAUDE_UA)
        .header("x-app", "cli")
        .header("anthropic-beta", CLAUDE_BETAS)
        .header("anthropic-dangerous-direct-browser-access", "true")
        .header("x-claude-code-session-id", new_session_uuid())
}

pub fn import_cli(kind: &str) -> Result<SubTokens, String> {
    match kind {
        "claude-code" => import_claude_cli(),
        "codex" => import_codex_cli(),
        _ => Err(format!("unknown subscription kind: {kind}")),
    }
}

// --- Usage (5h window), cached 90s ------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct WindowUsage {
    pub pct_5h: Option<f64>,      // 0..100
    pub reset_at_ms: Option<i64>, // epoch ms
    pub weekly_pct: Option<f64>,
    pub raw_note: String,
}

struct Cached {
    at: std::time::Instant,
    usage: WindowUsage,
}

static USAGE_CACHE: Mutex<Option<HashMap<String, Cached>>> = Mutex::new(None);
const USAGE_TTL: std::time::Duration = std::time::Duration::from_secs(90);

fn cache_get(profile_id: &str) -> Option<WindowUsage> {
    let g = USAGE_CACHE.lock().ok()?;
    let m = g.as_ref()?;
    let c = m.get(profile_id)?;
    if c.at.elapsed() < USAGE_TTL {
        Some(c.usage.clone())
    } else {
        None
    }
}

fn cache_put(profile_id: &str, u: WindowUsage) {
    if let Ok(mut g) = USAGE_CACHE.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        m.insert(profile_id.to_string(), Cached { at: std::time::Instant::now(), usage: u });
    }
}

fn f64_at(v: &serde_json::Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for k in path {
        cur = cur.get(*k)?;
    }
    cur.as_f64().or_else(|| cur.as_u64().map(|n| n as f64))
}

/// Claude /api/oauth/usage shape (tolerant): { five_hour: { utilization },
/// seven_day / weekly: { utilization }, resets_at ... }. Utilization may be
/// 0..1 or 0..100 — normalize to 0..100.
fn parse_claude_usage(v: &serde_json::Value) -> WindowUsage {
    let norm = |x: Option<f64>| x.map(|n| if n <= 1.0 && n > 0.0 { n * 100.0 } else { n });
    let five = v.get("five_hour").or_else(|| v.get("fiveHour")).or_else(|| v.get("rate_limit"));
    let week = v.get("seven_day").or_else(|| v.get("sevenDay")).or_else(|| v.get("weekly"));
    WindowUsage {
        pct_5h: five.and_then(|f| norm(f64_at(f, &["utilization"]).or_else(|| f64_at(v, &["five_hour_utilization"])))),
        reset_at_ms: five.and_then(|f| f.get("resets_at").and_then(|x| x.as_i64()))
            .or_else(|| v.get("resets_at").and_then(|x| x.as_i64())),
        weekly_pct: week.and_then(|w| norm(f64_at(w, &["utilization"]))),
        raw_note: String::new(),
    }
}

/// Codex /wham/usage shape (tolerant — internal endpoint, drifts).
fn parse_codex_usage(v: &serde_json::Value) -> WindowUsage {
    let norm = |x: Option<f64>| x.map(|n| if n <= 1.0 && n > 0.0 { n * 100.0 } else { n });
    WindowUsage {
        pct_5h: norm(f64_at(v, &["rate_limit", "primary_window", "utilization"])
            .or_else(|| f64_at(v, &["primary", "used_percent"]))
            .or_else(|| f64_at(v, &["usage", "primary_window", "used_percent"]))),
        reset_at_ms: v.get("resets_at").and_then(|x| x.as_i64()),
        weekly_pct: norm(f64_at(v, &["secondary_window", "utilization"])
            .or_else(|| f64_at(v, &["weekly", "used_percent"]))),
        raw_note: String::new(),
    }
}

pub async fn fetch_usage(kind: &str, access_token: &str) -> Result<WindowUsage, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    match kind {
        "claude-code" => {
            let resp = apply_claude_headers(
                client
                    .get("https://api.anthropic.com/api/oauth/usage")
                    .bearer_auth(access_token),
            )
                .send()
                .await
                .map_err(|e| format!("claude usage: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err("claude usage rate-limited (endpoint throttles — retry in 60s)".into());
            }
            if !status.is_success() {
                if status.as_u16() == 403 && text.contains("oauth_scope_insufficient") {
                    return Err("usage meter needs a full CLI login — this token can still chat (use Test)".into());
                }
                let short: String = text.chars().take(200).collect();
                return Err(format!("claude usage {status}: {short}"));
            }
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("claude usage parse: {e}"))?;
            Ok(parse_claude_usage(&v))
        }
        "codex" => {
            let resp = client
                .get("https://chatgpt.com/backend-api/wham/usage")
                .bearer_auth(access_token)
                .send()
                .await
                .map_err(|e| format!("codex usage: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err("codex usage rate-limited (retry in 60s)".into());
            }
            if !status.is_success() {
                return Err(format!("codex usage {status}: {text}"));
            }
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("codex usage parse: {e}"))?;
            Ok(parse_codex_usage(&v))
        }
        _ => Err(format!("unknown kind: {kind}")),
    }
}

/// Cached wrapper: fresh cache wins, else fetch + store (failures don't poison).
pub async fn usage_cached(profile_id: &str, kind: &str, access_token: &str) -> Result<WindowUsage, String> {
    if let Some(u) = cache_get(profile_id) {
        return Ok(u);
    }
    match fetch_usage(kind, access_token).await {
        Ok(u) => {
            cache_put(profile_id, u.clone());
            Ok(u)
        }
        Err(e) => Err(e),
    }
}

// --- API-equivalent value ($0 billed, still shown) ----------------------------
// Rough per-1M list rates used ONLY for display ("what API would charge").
// Refined later from model_registry; intentionally conservative + labeled estimate.

/// Returns equivalent USD cents for the turn's tokens.
pub fn api_equivalent_cents(kind: &str, input: u64, output: u64, cache_read: u64) -> i64 {
    let (inp, out, cached) = match kind {
        "claude-code" => (3.0, 15.0, 0.30), // Sonnet-class default
        "codex" => (2.0, 8.0, 0.20),        // GPT-5-class default
        _ => (0.0, 0.0, 0.0),
    };
    let usd = (input as f64) / 1e6 * inp
        + (output as f64) / 1e6 * out
        + (cache_read as f64) / 1e6 * cached;
    (usd * 100.0).round() as i64
}
