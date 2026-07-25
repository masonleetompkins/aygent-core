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

// --- Tool-use aware call (M0.3 agent loop) ---------------------------------

/// One Anthropic Messages turn WITH tools + prior message history. Returns the
/// raw response JSON so the agent loop can inspect stop_reason / tool_use.
/// `messages` is the running conversation array; `tools` the tool schemas.
pub async fn anthropic_turn(
    api_key: &str,
    model: &str,
    system: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = json!({
        "model": model,
        "max_tokens": 1024,
        "system": system,
        "tools": tools,
        "messages": messages,
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
    serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))
}

// --- STREAMING (Phase 1) ---------------------------------------------------
// Provider-agnostic streaming: a turn emits a sequence of StreamEvents. The
// agent loop forwards them live over the WS to the UI. Providers that can't
// stream fall back to a single "turn-based" completion (the UI shows a thinking
// animation instead of live tokens).

use futures_util::StreamExt;

/// Normalized streaming events — same shape regardless of provider. The Chat UI
/// only ever knows these, so adding OpenAI/OpenRouter/Ollama = a new parser, not
/// a new UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind")]
pub enum StreamEvent {
    /// A chunk of assistant text.
    TextDelta { text: String },
    /// The model wants to call a tool (emitted once its input is assembled).
    ToolUse { id: String, name: String, input: serde_json::Value },
    /// The turn finished. `stop_reason` = "tool_use" | "end_turn" | ...
    Done { stop_reason: String },
    /// A non-fatal note (e.g. fell back to non-streaming).
    Info { text: String },
    /// Fatal error for this turn.
    Error { text: String },
}

/// Does this provider/model support server-sent streaming? (Phase-1 providers
/// all do; kept as a hook so a future provider can declare turn-based only.)
pub fn provider_supports_streaming(provider: &str) -> bool {
    matches!(provider, "anthropic" | "openai" | "openrouter" | "ollama")
}

/// Stream one Anthropic turn. Calls `on_event` for each normalized StreamEvent
/// as it arrives off the wire. Assembles tool_use input deltas into a single
/// ToolUse event. Returns the assistant `content` array (for history) + stop.
pub async fn anthropic_stream_turn<F: FnMut(StreamEvent)>(
    api_key: &str,
    model: &str,
    system: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
    mut on_event: F,
) -> Result<(serde_json::Value, String), String> {
    let body = json!({
        "model": model,
        "max_tokens": 1024,
        "system": system,
        "tools": tools,
        "messages": messages,
        "stream": true,
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

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("anthropic {status}: {text}"));
    }

    // Reassemble the assistant content array from the SSE stream so we can put
    // it back into `messages` for the next turn (tool_result needs it verbatim).
    let mut blocks: Vec<serde_json::Value> = Vec::new();
    // per-index scratch for tool_use input JSON being streamed as partial_json
    let mut tool_json: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut stop_reason = String::from("end_turn");

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    // Whether we saw a terminal message_stop. If the stream closes WITHOUT one
    // (final frame lacked a trailing "\n\n", so it was never parsed), we drain
    // the remainder below and synthesize Done — otherwise the UI spinner hangs
    // forever waiting for a Done event that never comes.
    let mut saw_done = false;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("stream error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        // SSE frames are separated by blank lines; each has `data: {...}` lines.
        while let Some(pos) = buf.find("\n\n") {
            let frame = buf[..pos].to_string();
            buf.drain(..pos + 2);
            for line in frame.lines() {
                let line = line.trim_start();
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data.is_empty() { continue; }
                let ev: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v, Err(_) => continue,
                };
                match ev.get("type").and_then(|t| t.as_str()) {
                    Some("content_block_start") => {
                        let idx = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let block = ev.get("content_block").cloned().unwrap_or(json!({}));
                        // seed the block; text accumulates via deltas, tool_use via partial_json
                        while blocks.len() <= idx { blocks.push(json!({})); }
                        blocks[idx] = block.clone();
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            tool_json.insert(idx, String::new());
                        }
                    }
                    Some("content_block_delta") => {
                        let idx = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let delta = ev.get("delta").cloned().unwrap_or(json!({}));
                        match delta.get("type").and_then(|t| t.as_str()) {
                            Some("text_delta") => {
                                if let Some(t) = delta.get("text").and_then(|t| t.as_str()) {
                                    // accumulate into the block + emit live
                                    if let Some(b) = blocks.get_mut(idx) {
                                        let cur = b.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        b["text"] = json!(format!("{cur}{t}"));
                                        b["type"] = json!("text");
                                    }
                                    on_event(StreamEvent::TextDelta { text: t.to_string() });
                                }
                            }
                            // Thinking-enabled models (Opus 5, etc.) emit thinking_delta
                            // + signature_delta. We accumulate them so the block isn't
                            // empty, but they get STRIPPED before replay (see below) —
                            // Anthropic rejects a replayed thinking block that lacks its
                            // exact signature, and we don't round-trip that reliably.
                            Some("thinking_delta") => {
                                if let Some(t) = delta.get("thinking").and_then(|t| t.as_str()) {
                                    if let Some(b) = blocks.get_mut(idx) {
                                        let cur = b.get("thinking").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        b["thinking"] = json!(format!("{cur}{t}"));
                                    }
                                }
                            }
                            Some("signature_delta") => {
                                if let Some(s) = delta.get("signature").and_then(|x| x.as_str()) {
                                    if let Some(b) = blocks.get_mut(idx) {
                                        let cur = b.get("signature").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                        b["signature"] = json!(format!("{cur}{s}"));
                                    }
                                }
                            }
                            Some("input_json_delta") => {
                                if let Some(pj) = delta.get("partial_json").and_then(|x| x.as_str()) {
                                    tool_json.entry(idx).or_default().push_str(pj);
                                }
                            }
                            _ => {}
                        }
                    }
                    Some("content_block_stop") => {
                        let idx = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        // finalize a tool_use block: parse its assembled input JSON + emit
                        if let Some(raw) = tool_json.get(&idx) {
                            let input: serde_json::Value =
                                serde_json::from_str(raw).unwrap_or(json!({}));
                            if let Some(b) = blocks.get_mut(idx) {
                                b["input"] = input.clone();
                            }
                            let (id, name) = blocks.get(idx).map(|b| (
                                b.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                b.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            )).unwrap_or_default();
                            on_event(StreamEvent::ToolUse { id, name, input });
                        }
                    }
                    Some("message_delta") => {
                        if let Some(sr) = ev.get("delta").and_then(|d| d.get("stop_reason")).and_then(|s| s.as_str()) {
                            stop_reason = sr.to_string();
                        }
                    }
                    Some("message_stop") => {
                        on_event(StreamEvent::Done { stop_reason: stop_reason.clone() });
                        saw_done = true;
                    }
                    Some("error") => {
                        let msg = ev.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).unwrap_or("stream error");
                        on_event(StreamEvent::Error { text: msg.to_string() });
                        return Err(msg.to_string());
                    }
                    _ => {}
                }
            }
        }
    }

    // HANG FIX: the stream can close with a final frame (often message_stop) that
    // lacks a trailing "\n\n", so it never matched the delimiter loop above and
    // sits unparsed in `buf`. Parse whatever remains, using "\n" boundaries so a
    // dangling frame is still handled.
    if !buf.trim().is_empty() {
        for line in buf.lines() {
            let line = line.trim_start();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data.is_empty() { continue; }
            let ev: serde_json::Value = match serde_json::from_str(data) { Ok(v) => v, Err(_) => continue };
            match ev.get("type").and_then(|t| t.as_str()) {
                Some("message_delta") => {
                    if let Some(sr) = ev.get("delta").and_then(|d| d.get("stop_reason")).and_then(|s| s.as_str()) {
                        stop_reason = sr.to_string();
                    }
                }
                Some("message_stop") => {
                    on_event(StreamEvent::Done { stop_reason: stop_reason.clone() });
                    saw_done = true;
                }
                Some("error") => {
                    let msg = ev.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).unwrap_or("stream error");
                    on_event(StreamEvent::Error { text: msg.to_string() });
                    return Err(msg.to_string());
                }
                _ => {}
            }
        }
    }

    // FINAL SAFETY NET: if the stream ended and we STILL never emitted Done (no
    // message_stop at all — abrupt close, proxy cut, etc.), synthesize one so the
    // UI spinner is always resolved. The turn's content is intact either way.
    if !saw_done {
        on_event(StreamEvent::Done { stop_reason: stop_reason.clone() });
    }

    // Fix for the "each thinking block must contain thinking" 400 on Opus 5 and
    // other thinking-enabled models: STRIP thinking/redacted_thinking blocks from
    // the content we return for replay. On multi-turn tool loops Anthropic
    // requires a replayed thinking block to carry its exact original signature;
    // rather than risk an invalid round-trip, we drop them. The user still saw
    // the final answer stream live — only the internal thinking is omitted from
    // history. Keeps text + tool_use blocks intact (what the loop actually needs).
    let cleaned: Vec<serde_json::Value> = blocks
        .into_iter()
        .filter(|b| {
            let t = b.get("type").and_then(|x| x.as_str()).unwrap_or("");
            t != "thinking" && t != "redacted_thinking"
        })
        .collect();

    Ok((serde_json::Value::Array(cleaned), stop_reason))
}
