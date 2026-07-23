// AYGENT — Anthropic provider (M0.3).
// The Rust side makes the provider call so the API key NEVER leaves Rust (Atlas
// C1): key fetched from Keychain at request time, used in-memory, never handed
// to the daemon/WebView. Phase 0 = one non-streaming Messages call to prove the
// end-to-end path (key -> model -> answer). Streaming + tool-use land in M0.3+.

use serde_json::json;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_MODELS_URL: &str = "https://api.anthropic.com/v1/models";
const API_VERSION: &str = "2023-06-01";

/// Ask the account which models the key can actually use. Robust vs guessing
/// model IDs (and needed for the Settings UI model picker anyway). Returns the
/// list of model id strings, newest first (as the API returns them).
pub async fn anthropic_list_models(api_key: &str) -> Result<Vec<String>, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(ANTHROPIC_MODELS_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read body failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("anthropic {status}: {text}"));
    }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let ids = v
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ids)
}

/// One-shot Anthropic Messages call. Returns the assistant text, or an error.
/// `system` may include a note that the agent has a jailed file tool (tool-use
/// wiring is the next M0.3 sub-step; here we prove key->model->text).
pub async fn anthropic_complete(
    api_key: &str,
    model: &str,
    user_msg: &str,
) -> Result<String, String> {
    let body = json!({
        "model": model,
        "max_tokens": 1024,
        "messages": [{ "role": "user", "content": user_msg }]
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(ANTHROPIC_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read body failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("anthropic {status}: {text}"));
    }

    // Parse { content: [ { type:"text", text:"..." }, ... ] }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let out = v
        .get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|blk| blk.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if out.is_empty() {
        return Err(format!("empty completion; raw: {text}"));
    }
    Ok(out)
}
