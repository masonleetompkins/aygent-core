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

/// Fetch a model's real metadata from the STANDARD models endpoint
/// (`GET /v1/models/{id}`). VERIFIED LIVE (2026-08-12, Mason's account): the
/// standard endpoint already returns `max_input_tokens` (the real context
/// window, e.g. 1,000,000 for the Opus 4.8 / 5-series), `max_tokens` (max
/// output), and `display_name` on every model object — NO beta flag needed.
/// (An earlier version used `?beta=true` on a mistaken reading of the SDK spec;
/// the live standard endpoint carries the same fields, so we dropped it.)
/// Returns (context_window, max_output, display_name). Errors let the caller
/// fall back to the curated table.
pub async fn anthropic_model_info(api_key: &str, model_id: &str) -> Result<(u32, u32, String), String> {
    let url = format!("{ANTHROPIC_MODELS_URL}/{model_id}");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
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
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let ctx = v.get("max_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let max_out = v.get("max_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    let name = v.get("display_name").and_then(|x| x.as_str()).unwrap_or(model_id).to_string();
    Ok((ctx, max_out, name))
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
        "max_tokens": 16384,
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


// --- PROMPT CACHING (2026-08-03) --------------------------------------------
// Anthropic bills 100% of input tokens on EVERY call unless blocks carry an
// explicit cache_control breakpoint (it does NOT auto-cache like OpenAI).
// Before this, every round of the tool loop (up to 20/turn) and every turn of
// a long chat re-billed the full system prompt + tool schemas + entire history
// at fresh-input price. These helpers mark three breakpoints (limit is 4):
//   1. the system prompt (stable per agent),
//   2. the last tool schema (caches the whole tools array prefix),
//   3. the last message (caches the growing conversation prefix, so each
//      tool-loop round / follow-up turn only pays fresh price for what's new;
//      the rest is a cache read at ~10% of input price, 5-min TTL).
// All three CLONE their input — the caller's history array (which gets
// persisted) is never mutated with cache markers.

/// System prompt as a content-block array carrying a cache breakpoint.
fn cacheable_system(system: &str) -> serde_json::Value {
    if system.is_empty() {
        return json!("");
    }
    json!([{ "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }])
}

/// Tools array with a cache breakpoint on the LAST schema (caches the prefix).
fn cacheable_tools(tools: &serde_json::Value) -> serde_json::Value {
    let mut arr = tools.as_array().cloned().unwrap_or_default();
    if let Some(last) = arr.last_mut() {
        if let Some(obj) = last.as_object_mut() {
            obj.insert("cache_control".into(), json!({ "type": "ephemeral" }));
        }
    }
    json!(arr)
}

/// Messages with a cache breakpoint on the last block of the LAST message, so
/// the whole prior conversation is a cacheable prefix. A plain-string content
/// is converted to the equivalent single text block (same tokens).
fn cacheable_messages(messages: &serde_json::Value) -> serde_json::Value {
    let mut arr = messages.as_array().cloned().unwrap_or_default();
    if let Some(last) = arr.last_mut() {
        if let Some(content) = last.get_mut("content") {
            match content {
                serde_json::Value::String(s) => {
                    let text = s.clone();
                    *content = json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }]);
                }
                serde_json::Value::Array(blocks) => {
                    if let Some(b) = blocks.last_mut() {
                        if let Some(obj) = b.as_object_mut() {
                            obj.insert("cache_control".into(), json!({ "type": "ephemeral" }));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    json!(arr)
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
        "max_tokens": 16384,
        "system": cacheable_system(system),
        "tools": cacheable_tools(tools),
        "messages": cacheable_messages(messages),
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
    /// A tool call BEGAN streaming (name known, args still arriving). The UI
    /// opens a live card immediately instead of waiting for full assembly.
    ToolUseStart { id: String, name: String },
    /// A chunk of the streaming tool-call args (raw partial JSON). For
    /// write_file this is literally the code being written, live.
    ToolUseDelta { id: String, text: String },
    /// The model wants to call a tool (emitted once its input is assembled).
    ToolUse { id: String, name: String, input: serde_json::Value },
    /// The turn finished. `stop_reason` = "tool_use" | "end_turn" | ...
    Done { stop_reason: String },
    /// Token accounting for ONE provider turn, parsed from the provider usage
    /// block (Anthropic message_start/message_delta). Emitted once per streamed
    /// turn so the UI can meter context fill + $ cost. Counts are for THIS turn.
    Usage { input: u64, output: u64, cache_read: u64, cache_write: u64, #[serde(default)] context_window: u32 },
    /// A non-fatal note (e.g. fell back to non-streaming).
    Info { text: String },
    /// Fatal error for this turn.
    Error { text: String },
}

/// Find the byte-offset of the next SSE frame delimiter (a blank line, i.e.
/// \n\n or \r\n\r\n) in a raw byte buffer. Operating on bytes (not a decoded
/// String) means we never risk splitting a multi-byte UTF-8 character while
/// searching -- the delimiter itself is pure ASCII.
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
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
    cancel: Option<&crate::cancel::CancelFlag>,
    mut on_event: F,
) -> Result<(serde_json::Value, String), String> {
    let body = json!({
        "model": model,
        // 1024 was FAR too low: the model streams a preamble, then must emit a
        // tool_use block whose `content` arg can be a whole markdown document.
        // Hitting the cap mid-tool-block truncates the tool_use — the input JSON
        // never closes, content_block_stop never finalizes it, so NO ToolUse is
        // emitted, tool_results stays empty, and the loop breaks: a silent death
        // exactly at "generation". 8192 gives tool calls real room.
        "max_tokens": 64000,
        "system": cacheable_system(system),
        "tools": cacheable_tools(tools),
        "messages": cacheable_messages(messages),
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
    // USAGE (context meter + $ cost): Anthropic streams token counts in the
    // message_start frame (input_tokens + cache_read/creation) and the final
    // message_delta frame (output_tokens). We accumulate them and emit a single
    // Usage event at end-of-turn.
    let (mut u_in, mut u_out, mut u_cr, mut u_cw): (u64, u64, u64, u64) = (0, 0, 0, 0);

    let mut stream = resp.bytes_stream();
    // BUG FIX: buffer RAW BYTES, not a String. The old code did
    // `String::from_utf8_lossy(&bytes)` on every raw network chunk BEFORE
    // buffering -- if a multi-byte UTF-8 character (very common in non-English
    // text, which is exactly what triggered this) straddled a chunk boundary,
    // each half got independently mangled into U+FFFD replacement junk. Now we
    // only decode once a COMPLETE SSE frame is assembled, so a split char is
    // reassembled correctly before decoding.
    let mut buf: Vec<u8> = Vec::new();
    // Whether we saw a terminal message_stop. If the stream closes WITHOUT one
    // (final frame lacked a trailing "\n\n", so it was never parsed), we drain
    // the remainder below and synthesize Done — otherwise the UI spinner hangs
    // forever waiting for a Done event that never comes.
    let mut saw_done = false;

    while let Some(chunk) = stream.next().await {
        // STOP BUTTON: check the cancel flag on every chunk so a user-requested
        // stop takes effect the instant the next byte arrives, not after the
        // model finishes its whole turn. Drop the connection + return a distinct
        // error the caller (agent_stream) recognizes as "cancelled", not a crash.
        if cancel.map(|c| c.load(std::sync::atomic::Ordering::SeqCst)).unwrap_or(false) {
            on_event(StreamEvent::Done { stop_reason: "cancelled".into() });
            return Err("__CANCELLED__".into());
        }
        let bytes = chunk.map_err(|e| format!("stream error: {e}"))?;
        buf.extend_from_slice(&bytes);

        // SSE frames are separated by blank lines; each has `data: {...}` lines.
        // Find the frame boundary in BYTES first, then decode just that slice --
        // `buf` up to `pos` is guaranteed complete (the boundary itself is ASCII
        // "\n\n"), so decoding here can never split a multi-byte character.
        while let Some(pos) = find_double_newline(&buf) {
            let frame = String::from_utf8_lossy(&buf[..pos]).into_owned();
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
                    Some("message_start") => {
                        // Initial usage: input tokens + cache read/creation counts.
                        if let Some(us) = ev.get("message").and_then(|m| m.get("usage")) {
                            u_in = us.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(u_in);
                            u_cr = us.get("cache_read_input_tokens").and_then(|x| x.as_u64()).unwrap_or(u_cr);
                            u_cw = us.get("cache_creation_input_tokens").and_then(|x| x.as_u64()).unwrap_or(u_cw);
                        }
                    }
                    Some("content_block_start") => {
                        let idx = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let block = ev.get("content_block").cloned().unwrap_or(json!({}));
                        // seed the block; text accumulates via deltas, tool_use via partial_json
                        while blocks.len() <= idx { blocks.push(json!({})); }
                        blocks[idx] = block.clone();
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            tool_json.insert(idx, String::new());
                            on_event(StreamEvent::ToolUseStart {
                                id: block.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                name: block.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            });
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
                                    on_event(StreamEvent::ToolUseDelta {
                                        id: blocks.get(idx).and_then(|b| b.get("id")).and_then(|x| x.as_str()).unwrap_or("").to_string(),
                                        text: pj.to_string(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    Some("content_block_stop") => {
                        let idx = ev.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        // finalize a tool_use block: parse its assembled input JSON + emit
                        if let Some(raw) = tool_json.get(&idx) {
                            let input: serde_json::Value = match serde_json::from_str::<serde_json::Value>(raw) {
                                Ok(v) => v,
                                Err(e) => {
                                    eprintln!("[aygent][provider] tool input JSON truncated/parse failed ({} bytes): {}", raw.len(), e);
                                    serde_json::json!({"__harness_parse_error": e.to_string(), "__raw_len": raw.len()})
                                }
                            };
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
                        // Final usage lives on message_delta.usage (output_tokens,
                        // and Anthropic re-states input on some responses).
                        if let Some(us) = ev.get("usage") {
                            if let Some(o) = us.get("output_tokens").and_then(|x| x.as_u64()) { u_out = o; }
                            if let Some(i) = us.get("input_tokens").and_then(|x| x.as_u64()) { if i > 0 { u_in = i; } }
                        }
                    }
                    Some("message_stop") => {
                        on_event(StreamEvent::Usage { input: u_in, output: u_out, cache_read: u_cr, cache_write: u_cw, context_window: 0 });
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
    let tail = String::from_utf8_lossy(&buf).into_owned();
    if !tail.trim().is_empty() {
        for line in tail.lines() {
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
                    if let Some(us) = ev.get("usage") {
                        if let Some(o) = us.get("output_tokens").and_then(|x| x.as_u64()) { u_out = o; }
                    }
                }
                Some("message_stop") => {
                    on_event(StreamEvent::Usage { input: u_in, output: u_out, cache_read: u_cr, cache_write: u_cw, context_window: 0 });
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
        on_event(StreamEvent::Usage { input: u_in, output: u_out, cache_read: u_cr, cache_write: u_cw, context_window: 0 });
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
