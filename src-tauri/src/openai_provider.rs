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
    let client = reqwest::Client::new();
    let req = client
        .get(format!("{}/models", base_url(provider)))
        .header("Authorization", format!("Bearer {api_key}"));
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
                            out.push(json!({
                                "role": "tool",
                                "tool_call_id": b.get("tool_use_id").cloned().unwrap_or(json!("")),
                                "content": b.get("content").cloned().unwrap_or(json!("")),
                            }));
                        }
                        continue;
                    }
                }
                // Plain user text (string or array of text blocks).
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
pub async fn openai_stream_turn<F: FnMut(StreamEvent)>(
    provider: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
    mut on_event: F,
) -> Result<(serde_json::Value, String), String> {
    let body = json!({
        "model": model,
        "messages": build_openai_messages(system, messages),
        "tools": tools_to_openai(tools),
        "stream": true,
    });

    let client = reqwest::Client::new();
    let req = client
        .post(format!("{}/chat/completions", base_url(provider)))
        .header("Authorization", format!("Bearer {api_key}"))
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
    // index -> (id, name, arguments-so-far)
    let mut tool_acc: std::collections::BTreeMap<u64, (String, String, String)> = std::collections::BTreeMap::new();
    let mut stop_reason = String::from("stop");

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("stream error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].to_string();
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
                        if let Some(a) = f.get("arguments").and_then(|a| a.as_str()) { entry.2.push_str(a); }
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
