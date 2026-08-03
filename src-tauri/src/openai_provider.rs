// AYGENT — OpenAI-compatible provider (OpenAI + OpenRouter).
//
// OpenAI and OpenRouter share the SAME wire format (Chat Completions API), so
// this is ONE implementation with two base URLs. It emits the SAME normalized
// StreamEvents as the Anthropic provider, so the Chat UI + agent loop need no
// changes (the provider-agnostic design pays off again).
//
// Keys stay Rust-side (Atlas C1): fetched from Keychain at call time, used
// in-memory, never handed to the WebView/daemon.
//
// TOOL-USE: full. We translate our internal tool schema to OpenAI's `tools`
// format and parse `tool_calls` back. The caller (agent_stream) drives the loop
// with the same jailed exec_tool as every other provider.

use serde_json::json;
use futures_util::StreamExt;
use crate::provider::StreamEvent;

const OPENAI_BASE: &str = "https://api.openai.com/v1";
const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

fn base_url(provider: &str) -> &'static str {
    match provider {
        "openrouter" => OPENROUTER_BASE,
        _ => OPENAI_BASE, // "openai" default
    }
}

/// OpenRouter likes these headers for attribution/ranking (optional but polite).
fn apply_extra_headers(req: reqwest::RequestBuilder, provider: &str) -> reqwest::RequestBuilder {
    if provider == "openrouter" {
        req.header("HTTP-Referer", "https://aygent.app")
            .header("X-Title", "AYGENT")
    } else {
        req
    }
}

/// List models the key can use, via GET /models (live — no hardcoded list, so
/// new models appear without an app update). OpenRouter returns hundreds; the
/// caller can filter/sort. Returns model id strings.
pub async fn list_models(provider: &str, api_key: &str) -> Result<Vec<String>, String> {
    // No-redirect client: reqwest STRIPS the Authorization header across a
    // redirect (a security default), which is what caused OpenRouter's
    // "Missing Authentication header" 401 — its endpoint 30x-normalizes and the
    // bearer token was dropped on the follow. Disabling redirects keeps auth on.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| format!("http: {e}"))?;
    let req = client
        .get(format!("{}/models", base_url(provider)))
        .bearer_auth(api_key);
    let resp = apply_extra_headers(req, provider)
        .send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() {
        return Err(format!("{provider} {status}: {text}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let ids = v.get("data").and_then(|d| d.as_array()).map(|arr| {
        arr.iter().filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from)).collect::<Vec<_>>()
    }).unwrap_or_default();
    Ok(ids)
}

/// Actually verify a key works — unlike `list_models`, which for OpenRouter
/// hits a PUBLIC endpoint that returns 200 with a full model list even with an
/// empty/garbage key (confirmed live 2026-08-02: no auth header, still 200).
/// OpenRouter's /key endpoint DOES require auth (confirmed: 401 with none) —
/// use that to actually prove the key works. OpenAI has no equivalent "whoami"
/// on the same base, so /models (which IS auth-gated there) still applies.
pub async fn verify_key(provider: &str, api_key: &str) -> Result<(), String> {
    if provider == "openrouter" {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build().map_err(|e| format!("http: {e}"))?;
        let resp = client.get(format!("{OPENROUTER_BASE}/key")).bearer_auth(api_key)
            .send().await.map_err(|e| format!("request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("openrouter {status}: {text}"));
        }
        Ok(())
    } else {
        list_models(provider, api_key).await.map(|_| ())
    }
}

/// Convert our internal Anthropic-style tool schema to OpenAI `tools` format.
/// Ours: [{name, description, input_schema}]  ->  OpenAI:
/// [{type:"function", function:{name, description, parameters}}]
fn tools_to_openai(tools: &serde_json::Value) -> serde_json::Value {
    let arr = tools.as_array().cloned().unwrap_or_default();
    let out: Vec<serde_json::Value> = arr.iter().map(|t| json!({
        "type": "function",
        "function": {
            "name": t.get("name").cloned().unwrap_or(json!("")),
            "description": t.get("description").cloned().unwrap_or(json!("")),
            "parameters": t.get("input_schema").cloned().unwrap_or(json!({"type":"object"})),
        }
    })).collect();
    json!(out)
}

/// Convert our running message history (Anthropic-ish shape) into OpenAI
/// messages. We normalize:
///   - system prompt -> a leading {role:"system"} message
///   - assistant content that is a string -> content string
///   - assistant content array with tool_use blocks -> assistant {tool_calls:[...]}
///   - our tool_result user turns -> {role:"tool", tool_call_id, content}
/// The agent loop stores assistant turns as the OpenAI-native shape we return
/// from stream_turn, so round-tripping stays consistent.
fn build_openai_messages(system: &str, messages: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if !system.is_empty() {
        out.push(json!({ "role": "system", "content": system }));
    }
    if let Some(arr) = messages.as_array() {
        for m in arr {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = m.get("content").cloned().unwrap_or(json!(""));

            // Tool results: our loop pushes {role:"user", content:[{type:"tool_result",
            // tool_use_id, content, is_error}]}. Translate each to an OpenAI
            // {role:"tool"} message.
            if role == "user" {
                if let Some(blocks) = content.as_array() {
                    let tool_blocks: Vec<&serde_json::Value> = blocks.iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                        .collect();
                    if !tool_blocks.is_empty() {
                        for b in tool_blocks {
                            // CLEO-GUARD (2026-08-02): only emit a tool result if it references a
                            // REAL preceding assistant tool_call (non-empty, matching id). Some
                            // reasoning models (gpt-5.6-sol) yield orphaned tool blocks; drop them.
                            let tcid = b.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                            let prevOk = !tcid.is_empty()
                                && out.last()
                                    .and_then(|pm| pm.get("tool_calls"))
                                    .and_then(|tc| tc.as_array())
                                    .map(|arr| arr.iter().any(|c| c.get("id").and_then(|i| i.as_str()) == Some(tcid)))
                                    .unwrap_or(false);
                            if (!prevOk) { continue; }

                            out.push(json!({
                                "role": "tool",
                                "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(json!("")),
                                "content": b.get("content").cloned().unwrap_or(json!("")),
                            }));
                        }
                        continue;
                    }
                }
                // Plain user content: a string, OR an array of blocks that may include
                // our attachment blocks ({type:"text"} / {type:"image_url",image_url:{url}}
                // from attachment_blocks_openai in lib.rs). BUG FIX: this used to call
                // flatten_text(), which reduces the array to a joined STRING — silently
                // stripping any image_url block right back out, so an attached image
                // still vanished even after lib.rs started building it. OpenAI's Chat
                // Completions API accepts multipart content verbatim in this shape, so
                // pass the array through as-is; only flatten when it's plain text blocks
                // with nothing else (keeps prior behavior/log shape for the common case).
                if let Some(blocks) = content.as_array() {
                    let has_media = blocks.iter().any(|b| b.get("type").and_then(|t| t.as_str()) != Some("text"));
                    if has_media {
                        out.push(json!({ "role": "user", "content": blocks }));
                        continue;
                    }
                }
                out.push(json!({ "role": "user", "content": flatten_text(&content) }));
                continue;
            }

            if role == "assistant" {
                // Assistant may carry tool_calls (already OpenAI-shaped from us) or text.
                if let Some(tc) = m.get("tool_calls") {
                    out.push(json!({ "role": "assistant", "content": m.get("content").cloned().unwrap_or(json!("")), "tool_calls": tc }));
                } else {
                    out.push(json!({ "role": "assistant", "content": flatten_text(&content) }));
                }
                continue;
            }

            // Fallback: pass through as-is with flattened text.
            out.push(json!({ "role": role, "content": flatten_text(&content) }));
        }
    }
    out
}

/// Reduce a content value (string, or array of {type:text,text}) to a string.
fn flatten_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks.iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Stream one OpenAI/OpenRouter turn. Emits TextDelta live + ToolUse events.
/// Returns (assistant_message, stop_reason) where assistant_message is the
/// OpenAI-native assistant object (with tool_calls if any) to push into history.
/// M1.4: a NON-streaming completion (used by Soul.md generation). Collects the
/// streamed text into one string via openai_stream_turn with no tools.
pub async fn complete(provider: &str, api_key: &str, model: &str, user_msg: &str) -> Result<String, String> {
    let messages = json!([{ "role": "user", "content": user_msg }]);
    let no_tools = json!([]);
    let mut text = String::new();
    let (assistant, _stop) = openai_stream_turn(
        provider, api_key, model, "", &messages, &no_tools, None,
        |ev| {
            if let StreamEvent::TextDelta { text: t } = &ev { text.push_str(t); }
        },
    ).await?;
    // Prefer accumulated stream text; fall back to the assistant message content.
    if !text.trim().is_empty() { return Ok(text); }
    Ok(assistant.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string())
}

pub async fn openai_stream_turn<F: FnMut(StreamEvent)>(
    provider: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
    cancel: Option<&crate::cancel::CancelFlag>,
    mut on_event: F,
) -> Result<(serde_json::Value, String), String> {
    let tools_json = tools_to_openai(tools);
    let mut body = json!({
        "model": model,
        "messages": build_openai_messages(system, messages),
        "tools": tools_json,
        "stream": true,
    });
    // BUG FIX (Mason 08-02): OpenAI's reasoning-family models (o-series, gpt-5.x
    // "reasoning" variants) reject function tools on /v1/chat/completions unless
    // reasoning_effort is explicitly "none" — 400 "Function tools with
    // reasoning_effort are not supported ... use /v1/responses or set
    // reasoning_effort to 'none'". We need tool-use for the agent loop, so set
    // it whenever tools are present. Only OpenAI proper defines this param;
    // OpenRouter passes it through fine but doesn't require it — harmless either way.
    if !tools_json.as_array().map(|a| a.is_empty()).unwrap_or(true) {
        body["reasoning_effort"] = json!("none");
    }

    // No-redirect client (see list_models): keeps the bearer token attached so
    // OpenRouter doesn't 401 with "Missing Authentication header" on a redirect.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| format!("http: {e}"))?;
    let req = client
        .post(format!("{}/chat/completions", base_url(provider)))
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .json(&body);
    let resp = apply_extra_headers(req, provider)
        .send().await.map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("{provider} {status}: {text}"));
    }

    // Accumulate streamed text + tool_calls. OpenAI streams tool_calls as
    // deltas: each has an index, and (on the first delta) id + function.name,
    // then function.arguments arrives in pieces we concatenate.
    let mut text = String::new();
    // indexes whose ToolUseStart already fired (live-args streaming)
    let mut started: std::collections::HashSet<u64> = std::collections::HashSet::new();
    // index -> (id, name, arguments-so-far)
    let mut tool_acc: std::collections::BTreeMap<u64, (String, String, String)> = std::collections::BTreeMap::new();
    let mut stop_reason = String::from("stop");

    let mut stream = resp.bytes_stream();
    // BUG FIX (same class as provider.rs): buffer RAW BYTES, not a String. The
    // old code ran `String::from_utf8_lossy(&bytes)` on every raw network chunk
    // BEFORE buffering -- a multi-byte UTF-8 character split across two chunks
    // (very likely with a model like DeepSeek that streams a lot of non-ASCII
    // tokens) got mangled into replacement-character junk on EACH half
    // independently. This is the most likely cause of the "tons of random
    // numbers and letters" report. We now only decode once a complete LINE
    // (the `\n` boundary is pure ASCII, so it can't split a char) is assembled.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        // STOP BUTTON: see provider.rs — same per-chunk cancel check.
        if cancel.map(|c| c.load(std::sync::atomic::Ordering::SeqCst)).unwrap_or(false) {
            on_event(StreamEvent::Done { stop_reason: "cancelled".into() });
            return Err("__CANCELLED__".into());
        }
        let bytes = chunk.map_err(|e| format!("stream error: {e}"))?;
        buf.extend_from_slice(&bytes);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&buf[..pos]).into_owned();
            buf.drain(..pos + 1);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data.is_empty() { continue; }
            if data == "[DONE]" { continue; }
            let ev: serde_json::Value = match serde_json::from_str(data) { Ok(v) => v, Err(_) => continue };
            let Some(choice) = ev.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first()) else { continue };

            if let Some(fr) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                stop_reason = fr.to_string();
            }
            let delta = choice.get("delta").cloned().unwrap_or(json!({}));

            // Text delta.
            if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                if !t.is_empty() {
                    text.push_str(t);
                    on_event(StreamEvent::TextDelta { text: t.to_string() });
                }
            }
            // Tool-call deltas.
            if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tcs {
                    let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                    let entry = tool_acc.entry(idx).or_insert((String::new(), String::new(), String::new()));
                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) { if !id.is_empty() { entry.0 = id.to_string(); } }
                    if let Some(f) = tc.get("function") {
                        if let Some(n) = f.get("name").and_then(|n| n.as_str()) { if !n.is_empty() { entry.1 = n.to_string(); } }
                        if let Some(a) = f.get("arguments").and_then(|a| a.as_str()) {
                            entry.2.push_str(a);
                            // Live args streaming (parity with the Anthropic parser):
                            // fire ToolUseStart once id+name are known, then route
                            // each fragment as a ToolUseDelta to that card.
                            if !entry.0.is_empty() && !entry.1.is_empty() {
                                if started.insert(idx) {
                                    on_event(StreamEvent::ToolUseStart { id: entry.0.clone(), name: entry.1.clone() });
                                    // The fragments BEFORE start fired (id/name mid-assembly)
                                    // are already in entry.2 — replay them so nothing is lost.
                                    if entry.2.len() > a.len() {
                                        let backlog = entry.2[..entry.2.len() - a.len()].to_string();
                                        if !backlog.is_empty() { on_event(StreamEvent::ToolUseDelta { id: entry.0.clone(), text: backlog }); }
                                    }
                                }
                                if !a.is_empty() { on_event(StreamEvent::ToolUseDelta { id: entry.0.clone(), text: a.to_string() }); }
                            }
                        }
                    }
                }
            }
        }
    }

    // Assemble the assistant message + emit ToolUse events for the loop.
    let mut tool_calls_json = Vec::new();
    for (_idx, (id, name, args)) in &tool_acc {
        if name.is_empty() { continue; }
        let input: serde_json::Value = serde_json::from_str(args).unwrap_or(json!({}));
        on_event(StreamEvent::ToolUse { id: id.clone(), name: name.clone(), input: input.clone() });
        tool_calls_json.push(json!({
            "id": id,
            "type": "function",
            "function": { "name": name, "arguments": args },
        }));
    }

    on_event(StreamEvent::Done { stop_reason: stop_reason.clone() });

    let mut assistant = json!({ "role": "assistant", "content": text });
    if !tool_calls_json.is_empty() {
        assistant["tool_calls"] = json!(tool_calls_json);
        // Normalize stop so the agent loop knows to run tools.
        if stop_reason == "tool_calls" || !tool_calls_json.is_empty() { stop_reason = "tool_use".into(); }
    }
    Ok((assistant, stop_reason))
}
