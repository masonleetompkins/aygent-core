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
    let model = if model.is_empty() { "claude-opus-5-5" } else { model };
    let body = json!({
        "model": model,
        "max_tokens": 512,
        "system": [{ "type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude." }],
        "messages": [{ "role": "user", "content": user_msg }],
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let req = crate::subscription::apply_claude_headers(
        client
            .post("https://api.anthropic.com/v1/messages")
            .bearer_auth(access_token)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json"),
    );
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("claude oauth: {e}"))?;
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let text = resp.text().await.unwrap_or_default();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(format!("claude oauth rejected ({status}) — re-run `claude login` then Re-import: {text}"));
    }
    if !status.is_success() {
        // Surface exhausted distinctly so the loop can failover.
        if crate::sub_loop::is_exhausted_error(&format!("{status} {text}")) {
            // 429 on a seat usually means the subscription window is full right
            // now — say when to retry instead of dumping the raw body.
            if status.as_u16() == 429 {
                let when = if retry_after.is_empty() {
                    "wait a bit".to_string()
                } else {
                    format!("retry in {retry_after}s")
                };
                return Err(format!("Claude says rate-limited (429) — {when}, then Test again"));
            }
            return Err(format!("claude {status}: {}", text.chars().take(200).collect::<String>()));
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
pub async fn codex_oauth_complete(access_token: &str, account_id: &str, model: &str, user_msg: &str) -> Result<String, String> {
    let model = if model.is_empty() { "gpt-6.1-sol" } else { model };
    // NOTE: this backend rejects a bare-string input (400 "Input must be a
    // list") — always send the message-object form.
    let body = json!({
        "model": model,
        "input": [{ "role": "user", "content": [{ "type": "input_text", "text": user_msg }] }],
        "store": false,
        // This backend REQUIRES streaming (400 otherwise).
        "stream": true,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    // The Codex backend lives behind ChatGPT, not the platform API: account
    // routing rides on chatgpt-account-id (Mason 10-08, verified against the
    // CLI + codex-rs tests + langchain's codex wrapper).
    let mut req = client
        .post("https://chatgpt.com/backend-api/codex/responses")
        .bearer_auth(access_token)
        .header("content-type", "application/json")
        .header("originator", "codex_cli_rs");
    if !account_id.trim().is_empty() {
        req = req.header("chatgpt-account-id", account_id.trim());
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("codex oauth: {e}"))?;
    let status = resp.status();
    // NOTE: the success path streams (body consumed below), so error
    // branches read the body themselves.
    if status.as_u16() == 401 || status.as_u16() == 403 {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("codex oauth rejected ({status}) — re-run `codex login` then Re-import: {}", text.chars().take(200).collect::<String>()));
    }
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        if crate::sub_loop::is_exhausted_error(&format!("{status} {text}")) {
            if status.as_u16() == 429 {
                return Err("ChatGPT says rate-limited (429) — wait a bit, then Test again".into());
            }
            return Err(format!("codex {status}: {}", text.chars().take(200).collect::<String>()));
        }
        return Err(format!("codex oauth {status}: {}", text.chars().take(200).collect::<String>()));
    }
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut out = String::new();
    let mut last_type = String::new();
    let mut done = false;
    while !done {
        let Some(chunk) = stream.next().await else { break };
        let bytes = chunk.map_err(|e| format!("codex stream: {e}"))?;
        buf.extend_from_slice(&bytes);
        while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
            let frame = String::from_utf8_lossy(&buf[..pos]).into_owned();
            buf.drain(..pos + 2);
            let mut data = String::new();
            for line in frame.lines() {
                let line = line.trim_start();
                if let Some(d) = line.strip_prefix("data:") {
                    data.push_str(d.trim());
                }
            }
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let ev: serde_json::Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let t = ev.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
            if !t.is_empty() {
                last_type = t.clone();
            }
            if t == "response.output_text.delta" {
                if let Some(d) = ev.get("delta").and_then(|d| d.as_str()) {
                    out.push_str(d);
                }
            } else if t == "response.completed" || t == "response.failed" || t == "response.incomplete" {
                done = true;
                break;
            }
        }
    }
    if out.is_empty() {
        return Err(format!("empty completion (last event: {})", if last_type.is_empty() { "none" } else { &last_type }));
    }
    Ok(out)
}

/// Stored ChatGPT account id for a profile ("" when imported before it was
/// captured). Sent as chatgpt-account-id: the Codex backend routes on it.
pub fn profile_account(id: &str) -> String {
    let raw = match crate::keychain::get_key(&crate::subscription::key_slot(id)) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("account_id").and_then(|a| a.as_str()).map(String::from))
        .unwrap_or_default()
}

/// Probe a profile end-to-end: resolve token, run the right complete().
/// Returns (kind, assistant_text). Live-verify entry point.
fn user_facing(e: String) -> String {
    // Test results are human-readable by contract: bounded text only.
    e.chars().take(300).collect()
}

pub async fn sub_probe(id: String, prompt: String) -> Result<serde_json::Value, String> {
    let (kind, access) = profile_token(&id).map_err(user_facing)?;
    let text = match kind.as_str() {
        "claude-code" => claude_oauth_complete(&access, "", &prompt).await.map_err(user_facing)?,
        "codex" => {
                // Account id may predate stored profiles: fall back to a live
                // file read (the CLI rotates these files; never refresh here —
                // refresh tokens are single-use and refreshing would log the
                // CLI out). Stale ACCESS tokens still need Re-import.
                let mut acct = profile_account(&id);
                if acct.is_empty() {
                    if let Ok(t) = crate::subscription::import_cli("codex") {
                        acct = t.account_id;
                    }
                }
                codex_oauth_complete(&access, &acct, "", &prompt).await.map_err(user_facing)?
            }
        k => return Err(user_facing(format!("unknown kind: {k}"))),
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
        "claude-opus-5-5"
    } else {
        "gpt-6.1-sol"
    }
}
