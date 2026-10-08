// AYGENT — Subscription token-loop probe (slice 4).
//
// Proves the OAuth token-loop spends SUBSCRIPTION credits (not API $) with two
// minimal non-streaming calls. Streaming + tools reuse provider.rs /
// openai_provider.rs parsers in the wiring slice — this file is the auth proof
// Mason verifies live first (both CLIs logged in).
//
//   Claude: POST api.anthropic.com/v1/messages (Bearer OAuth + oauth beta)
//   Codex:  POST api.openai.com/v1/chat/completions (Bearer ChatGPT OAuth)
// Secrets never leave Rust; tokens come from sub_cmds profiles via Keychain.

use serde_json::json;

/// Load (kind, access_token) for a subscription profile id.
pub fn profile_token(id: &str) -> Result<(String, String), String> {
    let raw = crate::keychain::get_key("sub:profiles").unwrap_or_default();
    let map: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(&raw).unwrap_or_default();
    let kind = map
        .get(id)
        .and_then(|p| p.get("kind"))
        .and_then(|k| k.as_str())
        .ok_or("unknown subscription profile")?
        .to_string();
    let sec = crate::keychain::get_key(&crate::subscription::key_slot(id))
        .map_err(|_| "no token imported — run Import from CLI first".to_string())?;
    let v: serde_json::Value =
        serde_json::from_str(&sec).map_err(|_| "stored token unreadable — re-import".to_string())?;
    let access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if access.is_empty() {
        return Err("no token imported — run Import from CLI first".into());
    }
    Ok((kind, access))
}

/// Minimal Claude Messages call over the subscription OAuth.
/// Returns assistant text; any 401/403 means re-login + re-import.
pub async fn claude_oauth_complete(access_token: &str, model: &str, user_msg: &str) -> Result<String, String> {
    let model = if model.is_empty() { "claude-sonnet-4-5" } else { model };
    let body = json!({
        "model": model,
        "max_tokens": 512,
        "messages": [{ "role": "user", "content": user_msg }],
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .bearer_auth(access_token)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("claude oauth: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(format!("claude oauth rejected ({status}) — re-run `claude login` then Re-import: {text}"));
    }
    if !status.is_success() {
        // Surface exhausted distinctly so the loop can failover.
        if crate::sub_loop::is_exhausted_error(&format!("{status} {text}")) {
            return Err(format!("__EXHAUSTED__ claude {status}: {text}"));
        }
        return Err(format!("claude oauth {status}: {text}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let out = v
        .get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if out.is_empty() {
        return Err(format!("empty completion; raw: {text}"));
    }
    Ok(out)
}

/// Minimal OpenAI chat call over the ChatGPT (Codex) OAuth.
pub async fn codex_oauth_complete(access_token: &str, model: &str, user_msg: &str) -> Result<String, String> {
    let model = if model.is_empty() { "gpt-5.1-codex-mini" } else { model };
    let body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": user_msg }],
        "stream": false,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(access_token)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("codex oauth: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(format!("codex oauth rejected ({status}) — re-run `codex login` then Re-import: {text}"));
    }
    if !status.is_success() {
        if crate::sub_loop::is_exhausted_error(&format!("{status} {text}")) {
            return Err(format!("__EXHAUSTED__ codex {status}: {text}"));
        }
        return Err(format!("codex oauth {status}: {text}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let out = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    if out.is_empty() {
        return Err(format!("empty completion; raw: {text}"));
    }
    Ok(out)
}

/// Probe a profile end-to-end: resolve token, run the right complete().
/// Returns (kind, assistant_text). Live-verify entry point.
pub async fn sub_probe(id: String, prompt: String) -> Result<serde_json::Value, String> {
    let (kind, access) = profile_token(&id)?;
    let text = match kind.as_str() {
        "claude-code" => claude_oauth_complete(&access, "", &prompt).await?,
        "codex" => codex_oauth_complete(&access, "", &prompt).await?,
        k => return Err(format!("unknown kind: {k}")),
    };
    Ok(serde_json::json!({ "ok": true, "kind": kind, "text": text }))
}

/// Resolve the serving seat for a subscription kind + agent: pinned profile
/// first, else first healthy in label order (skips spent windows via cached
/// usage). Returns (native_provider, oauth_token, profile_id, label).
pub async fn resolve_seat(kind: &str, agent_id: &str) -> Result<(String, String, String, String), String> {
    let mut order = crate::sub_cmds::profile_order(kind);
    if order.is_empty() {
        return Err(format!("no {kind} profiles yet — add one in Settings → Subscriptions"));
    }
    if let Some(pinned) = crate::sub_cmds::pinned_profile(agent_id) {
        if crate::sub_cmds::get_profile(&pinned).map(|p| p.kind == kind).unwrap_or(false) {
            order.retain(|(id, _)| id != &pinned);
            if let Some(p) = crate::sub_cmds::get_profile(&pinned) {
                order.insert(0, (p.id, p.label));
            }
        }
    }
    let mut last_err = String::from("no stored token — Import or paste one per profile");
    for (id, label) in &order {
        let access = match profile_token(id) {
            Ok((k, a)) if k == kind && !a.is_empty() => a,
            _ => continue,
        };
        match crate::subscription::usage_cached(id, kind, &access).await {
            Ok(u) if crate::sub_loop::usage_exhausted(u.pct_5h) => {
                last_err = format!(
                    "{label} window spent{}",
                    u.reset_at_ms.map(|ms| format!(" — resets {ms}")).unwrap_or_default()
                );
                continue;
            }
            _ => {}
        }
        let native = if kind == "claude-code" { "anthropic" } else { "openai" }.to_string();
        return Ok((native, access, id.clone(), label.clone()));
    }
    Err(format!("no usable {kind} seat right now — {last_err}"))
}

/// Default model when the agent leaves model blank on a seat.
pub fn default_model(kind: &str) -> &'static str {
    if kind == "claude-code" {
        "claude-sonnet-4-5"
    } else {
        "gpt-5.2"
    }
}
