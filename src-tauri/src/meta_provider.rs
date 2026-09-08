// AYGENT — Muse (Meta) provider.
//
// Muse is Meta's model service on the OpenAI **RESPONSES API** shape (a
// reasoning model): POST https://api.meta.ai/v1/responses, Bearer auth.
// Verified live against Mason's key (2026), BOTH non-stream and SSE:
//   REQUEST : { model, input:[{role, content:[{type:"input_text", text}]}],
//              stream, tools:[{type:"function",name,description,parameters}], tool_choice }
//   NON-STREAM RESPONSE: { id, object:"response", status, output:[ {type:"reasoning"},
//              {type:"message", role:"assistant", content:[{type:"output_text", text}]} ],
//              usage, error }
//   STREAM SSE frames (event: <name>\ndata: {json}): response.output_text.delta carries
//              {delta} live text; response.output_item.done carries a completed
//              function_call item; response.completed is terminal.
//   Model id seen: "muse-spark-1.2".
//
// It differs from every other provider on BOTH sides (input parts typed
// input_text, assistant text output_text, top-level output[] of typed items,
// reasoning item first), hence its own module. Keys stay Rust-side (Atlas C1).
// Emits the SAME normalized StreamEvents as every other provider, so the Chat
// UI + agent loop are unchanged.

use serde_json::json;
use futures_util::StreamExt;
use crate::provider::StreamEvent;

const MUSE_BASE: &str = "https://api.meta.ai/v1";

/// List models via GET /models (auth-gated). Parses data[].id; tolerates a bare
/// array. If a deployment hides /models the user can still type a model id.
pub async fn list_models(api_key: &str) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| format!("http: {e}"))?;
    let resp = client
        .get(format!("{MUSE_BASE}/models"))
        .bearer_auth(api_key)
        .send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() { return Err(format!("muse {status}: {text}")); }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let arr = v.get("data").and_then(|d| d.as_array())
        .or_else(|| v.as_array()).cloned().unwrap_or_default();
    Ok(arr.iter().filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from)).collect())
}

/// DYNAMIC model metadata for Muse: fetch /models and read this model's context
/// window IF the deployment publishes one. Meta's Responses-API /models is NOT
/// guaranteed to include a window field, and its name isn't standardized, so we
/// probe the common keys (context_window / context_length / max_input_tokens /
/// max_context_tokens, nested under a `capabilities` obj too). Returns the window
/// in tokens, or 0 when absent -> the caller uses the curated fallback (1M for
/// the Spark family). Best-effort: any error returns 0, never breaks the meter.
pub async fn muse_model_info(api_key: &str, model_id: &str) -> Result<u32, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| format!("http: {e}"))?;
    let resp = client
        .get(format!("{MUSE_BASE}/models"))
        .bearer_auth(api_key)
        .send().await.map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("read body: {e}"))?;
    if !status.is_success() { return Err(format!("muse {status}: {text}")); }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("bad json: {e}"))?;
    let arr = v.get("data").and_then(|d| d.as_array()).or_else(|| v.as_array()).cloned().unwrap_or_default();
    let Some(m) = arr.iter().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(model_id)) else {
        return Ok(0);
    };
    // Probe common window field names, top-level and under `capabilities`.
    let keys = ["context_window", "context_length", "max_input_tokens", "max_context_tokens", "max_context_window"];
    let read = |obj: &serde_json::Value| -> u32 {
        for k in keys { if let Some(n) = obj.get(k).and_then(|x| x.as_u64()) { if n > 0 { return n as u32; } } }
        0
    };
    let mut ctx = read(m);
    if ctx == 0 { if let Some(cap) = m.get("capabilities") { ctx = read(cap); } }
    Ok(ctx)
}

/// Verify a key works. Prefer /models (auth-gated); if that endpoint is absent
/// (non-401 error), fall back to a tiny /responses probe so a valid key passes.
pub async fn verify_key(api_key: &str) -> Result<(), String> {
    match list_models(api_key).await {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.contains(" 401") { return Err(e); }
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build().map_err(|e| format!("http: {e}"))?;
            let body = json!({
                "model": "muse-spark-1.2",
                "input": [ { "role": "user", "content": [ { "type": "input_text", "text": "ping" } ] } ],
                "max_output_tokens": 1, "stream": false,
            });
            let resp = client.post(format!("{MUSE_BASE}/responses"))
                .bearer_auth(api_key).header("content-type", "application/json")
                .json(&body).send().await.map_err(|e| format!("request failed: {e}"))?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err("muse 401: invalid api key".into());
            }
            Ok(())
        }
    }
}

/// Our tool schema [{name,description,input_schema}] -> Responses tools
/// [{type:"function", name, description, parameters}] (top-level, not nested).
fn tools_to_muse(tools: &serde_json::Value) -> serde_json::Value {
    let arr = tools.as_array().cloned().unwrap_or_default();
    let out: Vec<serde_json::Value> = arr.iter().map(|t| json!({
        "type": "function",
        "name": t.get("name").cloned().unwrap_or(json!("")),
        "description": t.get("description").cloned().unwrap_or(json!("")),
        "parameters": t.get("input_schema").cloned().unwrap_or(json!({"type":"object"})),
    })).collect();
    json!(out)
}

/// Reduce a content value (string, or array of {type,text}) to a plain string;
/// handles text blocks under any label (text/input_text/output_text).
fn flatten_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks.iter()
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Build the Responses `input[]` from our running history:
///   system -> {role:system, content:[input_text]}
///   user text -> {role:user, content:[input_text]} (image parts pass through)
///   assistant text -> {role:assistant, content:[output_text]}
///   assistant tool_calls -> {type:function_call, call_id, name, arguments} each
///   our tool_result blocks -> {type:function_call_output, call_id, output}
/// Muse rejects a request whose INLINE IMAGES total more than ~18MB with a bare
/// `400 Invalid upload request.` (verified live 09-08: 7x a 1.9MB screenshot OK,
/// 8x -> that 400, 9x -> 413 payload_too_large). History resends every image on
/// every turn, so a chat with a few full-res screenshots dies permanently.
/// Budget: keep the NEWEST images whole until this many data-URL bytes are
/// used; older ones degrade to a text stub. The model already saw them.
const MUSE_IMAGE_BUDGET_BYTES: usize = 12 * 1024 * 1024;

/// Byte length of a data: URL image (0 for remote URLs, which cost nothing inline).
fn inline_image_len(b: &serde_json::Value) -> usize {
    b.get("image_url").and_then(|i| i.get("url")).and_then(|u| u.as_str())
        .filter(|u| u.starts_with("data:")).map(|u| u.len()).unwrap_or(0)
}

/// Ids (message index, block index) of image blocks that fit the budget,
/// newest first. Anything not in the set is stubbed by build_muse_input.
fn images_within_budget(arr: &[serde_json::Value]) -> std::collections::HashSet<(usize, usize)> {
    let mut keep = std::collections::HashSet::new();
    let mut used = 0usize;
    for (mi, m) in arr.iter().enumerate().rev() {
        let Some(blocks) = m.get("content").and_then(|c| c.as_array()) else { continue };
        for (bi, b) in blocks.iter().enumerate().rev() {
            if b.get("type").and_then(|t| t.as_str()) != Some("image_url") { continue; }
            let n = inline_image_len(b);
            if used + n <= MUSE_IMAGE_BUDGET_BYTES { used += n; keep.insert((mi, bi)); }
        }
    }
    keep
}

fn build_muse_input(system: &str, messages: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if !system.is_empty() {
        out.push(json!({ "role": "system", "content": [ { "type": "input_text", "text": system } ] }));
    }
    let Some(arr) = messages.as_array() else { return out; };
    let keep_img = images_within_budget(arr);
    // call_ids of function_call items emitted so far. A function_call_output whose
    // call isn't in this window (history sliced mid-pair, e.g. the headless
    // last-40 window, or a compacted chat) 400s with "No function call found for
    // function call output with call_id" — drop it instead; the result text is
    // already reflected in the assistant's later turns.
    let mut seen_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (mi, m) in arr.iter().enumerate() {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = m.get("content").cloned().unwrap_or(json!(""));
        if role == "user" {
            if let Some(blocks) = content.as_array() {
                let tool_blocks: Vec<&serde_json::Value> = blocks.iter()
                    .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")).collect();
                if !tool_blocks.is_empty() {
                    for b in tool_blocks {
                        let call_id = b.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                        if call_id.is_empty() || !seen_calls.contains(call_id) { continue; }
                        let output = match b.get("content") {
                            Some(serde_json::Value::String(s)) => s.clone(),
                            Some(v) => flatten_text(v),
                            None => String::new(),
                        };
                        out.push(json!({ "type": "function_call_output", "call_id": call_id, "output": output }));
                    }
                    continue;
                }
                let has_media = blocks.iter().any(|b| {
                    let t = b.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    t != "text" && t != "input_text"
                });
                if has_media {
                    let parts: Vec<serde_json::Value> = blocks.iter().enumerate().filter_map(|(bi, b)| {
                        match b.get("type").and_then(|t| t.as_str()) {
                            Some("text") | Some("input_text") =>
                                Some(json!({ "type": "input_text", "text": b.get("text").and_then(|t| t.as_str()).unwrap_or("") })),
                            Some("image_url") => {
                                if inline_image_len(b) > 0 && !keep_img.contains(&(mi, bi)) {
                                    return Some(json!({ "type": "input_text", "text": "[earlier image omitted to stay under the provider's upload limit — it was already seen; ask the user to re-attach if you need it again]" }));
                                }
                                let url = b.get("image_url").and_then(|i| i.get("url")).and_then(|u| u.as_str()).unwrap_or("");
                                Some(json!({ "type": "input_image", "image_url": url }))
                            }
                            _ => None,
                        }
                    }).collect();
                    out.push(json!({ "role": "user", "content": parts }));
                    continue;
                }
            }
            out.push(json!({ "role": "user", "content": [ { "type": "input_text", "text": flatten_text(&content) } ] }));
            continue;
        }
        if role == "assistant" {
            // AYGENT also stores Anthropic-shaped history (content = [{type:"tool_use",id,name,input}])
            // from the browser agent loop. Handle that here so Muse sees the function calls
            // even when history was written in Anthropic shape (otherwise second turn 400s with
            // "No function call found for function call output with call_id").
            let anthropic_uses: Vec<serde_json::Value> = content.as_array().map(|arr|
                arr.iter().filter(|b| b.get("type").and_then(|x| x.as_str())==Some("tool_use")).cloned().collect()
            ).unwrap_or_default();
            if !anthropic_uses.is_empty() {
                let txt = flatten_text(&content);
                if !txt.is_empty() {
                    out.push(json!({ "role": "assistant", "content": [ { "type": "output_text", "text": txt } ] }));
                }
                for blk in &anthropic_uses {
                    let id = blk.get("id").and_then(|x| x.as_str()).unwrap_or("");
                    let name = blk.get("name").and_then(|x| x.as_str()).unwrap_or("");
                    let input = blk.get("input").cloned().unwrap_or(json!({}));
                    let args = if input.is_string() { input.as_str().unwrap_or("{}").to_string() } else { serde_json::to_string(&input).unwrap_or("{}".to_string()) };
                    seen_calls.insert(id.to_string());
                    out.push(json!({ "type": "function_call", "call_id": id, "name": name, "arguments": args }));
                }
                continue;
            }
            if let Some(tcs) = m.get("tool_calls").and_then(|t| t.as_array()) {
                let txt = flatten_text(&content);
                if !txt.is_empty() {
                    out.push(json!({ "role": "assistant", "content": [ { "type": "output_text", "text": txt } ] }));
                }
                for tc in tcs {
                    let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    let f = tc.get("function").cloned().unwrap_or(json!({}));
                    seen_calls.insert(id.to_string());
                    out.push(json!({
                        "type": "function_call", "call_id": id,
                        "name": f.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                        "arguments": f.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}"),
                    }));
                }
                continue;
            }
            out.push(json!({ "role": "assistant", "content": [ { "type": "output_text", "text": flatten_text(&content) } ] }));
            continue;
        }
        out.push(json!({ "role": role, "content": [ { "type": "input_text", "text": flatten_text(&content) } ] }));
    }
    out
}

/// Concatenate every output_text .text inside `message` items of a completed
/// `output[]` array. Skips reasoning items. (Fallback when deltas were missed.)
fn extract_output_text(output: &serde_json::Value) -> String {
    let mut s = String::new();
    if let Some(items) = output.as_array() {
        for it in items {
            if it.get("type").and_then(|t| t.as_str()) != Some("message") { continue; }
            if let Some(content) = it.get("content").and_then(|c| c.as_array()) {
                for part in content {
                    if part.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()) { s.push_str(t); }
                    }
                }
            }
        }
    }
    s
}

/// Pull function calls out of a `function_call` output ITEM (from either the
/// stream's response.output_item.done or the completed output[]). Returns
/// (call_id, name, arguments-json-string).
fn call_from_item(it: &serde_json::Value) -> Option<(String, String, String)> {
    if it.get("type").and_then(|t| t.as_str()) != Some("function_call") { return None; }
    let id = it.get("call_id").and_then(|x| x.as_str())
        .or_else(|| it.get("id").and_then(|x| x.as_str())).unwrap_or("").to_string();
    let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if name.is_empty() { return None; }
    let args = it.get("arguments").and_then(|x| x.as_str()).map(String::from)
        .or_else(|| it.get("arguments").map(|a| a.to_string()))
        .unwrap_or_else(|| "{}".to_string());
    Some((id, name, args))
}

/// NON-streaming completion (Soul.md generation): runs the turn with no tools
/// and collects the streamed text.
pub async fn complete(api_key: &str, model: &str, user_msg: &str) -> Result<String, String> {
    let messages = json!([{ "role": "user", "content": user_msg }]);
    let no_tools = json!([]);
    let mut text = String::new();
    let (assistant, _stop) = meta_stream_turn(
        api_key, model, None, "", &messages, &no_tools, None,
        |ev| { if let StreamEvent::TextDelta { text: t } = &ev { text.push_str(t); } },
    ).await?;
    if !text.trim().is_empty() { return Ok(text); }
    Ok(assistant.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string())
}

/// Stream one Muse (Responses API) turn over SSE. Parses the typed events:
///   response.output_text.delta      -> live text chunk (data.delta)
///   response.output_item.done       -> a completed item; function_call items
///                                      become ToolUse events
///   response.completed              -> terminal; data.response.status is the
///                                      stop reason; its output[] is the fallback
///   response.error / error / .failed-> failure
/// Reasoning items stream too but carry no user text; they are ignored.
/// Returns (assistant_message, stop_reason) where assistant_message is the
/// OpenAI-native assistant object (with tool_calls if any) the agent loop stores
/// + round-trips; stop_reason becomes "tool_use" when tool calls are present.
pub async fn meta_stream_turn<F: FnMut(StreamEvent)>(
    api_key: &str,
    model: &str,
    variant: Option<&str>,
    system: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
    cancel: Option<&crate::cancel::CancelFlag>,
    mut on_event: F,
) -> Result<(serde_json::Value, String), String> {
    let tools_json = tools_to_muse(tools);
    let mut body = json!({
        "model": model,
        "input": build_muse_input(system, messages),
        "max_output_tokens": 32000,
        "stream": true,
    });
    // MODEL VARIANT (Mason 09-05): per-agent reasoning knob for Spark models
    // (minimal|low|medium|high|xhigh|max). Same model id, more/less thinking.
    // "" / absent / unknown = provider default (omit the field entirely).
    if let Some(v) = variant.map(str::trim).filter(|v| !v.is_empty()) {
        if ["minimal", "low", "medium", "high", "xhigh", "max"].contains(&v) {
            body["reasoning"] = json!({ "effort": v });
        }
    }
    if !tools_json.as_array().map(|a| a.is_empty()).unwrap_or(true) {
        body["tools"] = tools_json;
        body["tool_choice"] = json!("auto");
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| format!("http: {e}"))?;
    let resp = client
        .post(format!("{MUSE_BASE}/responses"))
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&body)
        .send().await.map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("muse {status}: {text}"));
    }

    let mut answer = String::new();
    // Tool calls collected from function_call items, in first-seen order.
    let mut calls: Vec<(String, String, String)> = Vec::new();
    let mut started: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Streamed function_call arguments accumulate under the item's fc_ id
    // (response.function_call_arguments.delta/.done use item_id, NOT call_id).
    let mut arg_acc: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // Map an fc_ item id -> its call_id (from output_item.added/done) so a
    // streamed args frame can be tied to the right call.
    let mut item_to_call: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    let mut stop_reason = String::from("completed");
    // USAGE (context meter + $ cost): the Responses API reports token counts on
    // the response.completed frame (response.usage). Muse price is $0 in the
    // catalog, but the token counts still drive the context-fill meter.
    let (mut u_in, mut u_out, mut u_cr, mut u_cw): (u64, u64, u64, u64) = (0, 0, 0, 0);
    let mut saw_done = false;

    let mut stream = resp.bytes_stream();
    // Buffer RAW BYTES; split on the SSE frame delimiter (blank line) so a
    // multi-byte UTF-8 char is never decoded across a chunk boundary. Each SSE
    // frame is one or more `event:`/`data:` lines terminated by "\n\n".
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        if cancel.map(|c| c.load(std::sync::atomic::Ordering::SeqCst)).unwrap_or(false) {
            on_event(StreamEvent::Done { stop_reason: "cancelled".into() });
            return Err("__CANCELLED__".into());
        }
        let bytes = chunk.map_err(|e| format!("stream error: {e}"))?;
        buf.extend_from_slice(&bytes);
        while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
            let frame = String::from_utf8_lossy(&buf[..pos]).into_owned();
            buf.drain(..pos + 2);
            // Concatenate all `data:` lines in this frame (SSE allows multiple).
            let mut data = String::new();
            for line in frame.lines() {
                let line = line.trim_start();
                if let Some(d) = line.strip_prefix("data:") { data.push_str(d.trim()); }
            }
            if data.is_empty() || data == "[DONE]" { continue; }
            let ev: serde_json::Value = match serde_json::from_str(&data) { Ok(v) => v, Err(_) => continue };
            match ev.get("type").and_then(|t| t.as_str()) {
                Some("response.output_text.delta") => {
                    if let Some(d) = ev.get("delta").and_then(|d| d.as_str()) {
                        if !d.is_empty() { answer.push_str(d); on_event(StreamEvent::TextDelta { text: d.to_string() }); }
                    }
                }
                // Track a function_call item as it OPENS (args are empty here) so a
                // later arguments frame can be tied back to its call_id + name.
                Some("response.output_item.added") => {
                    if let Some(item) = ev.get("item") {
                        if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                            let item_id = item.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let call_id = item.get("call_id").and_then(|x| x.as_str())
                                .or_else(|| item.get("id").and_then(|x| x.as_str())).unwrap_or("").to_string();
                            let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            if !item_id.is_empty() { item_to_call.insert(item_id.clone(), (call_id, name)); }
                            arg_acc.entry(item_id).or_default();
                        }
                    }
                }
                // Streamed argument fragments (keyed by item_id, NOT call_id).
                Some("response.function_call_arguments.delta") => {
                    if let (Some(iid), Some(d)) = (ev.get("item_id").and_then(|x| x.as_str()), ev.get("delta").and_then(|x| x.as_str())) {
                        arg_acc.entry(iid.to_string()).or_default().push_str(d);
                    }
                }
                Some("response.function_call_arguments.done") => {
                    if let (Some(iid), Some(a)) = (ev.get("item_id").and_then(|x| x.as_str()), ev.get("arguments").and_then(|x| x.as_str())) {
                        // .done carries the FULL arguments string — authoritative.
                        arg_acc.insert(iid.to_string(), a.to_string());
                    }
                }
                // Item FINALIZED. For a function_call this frame carries the final
                // arguments; emit the ToolUse here (NOT on .added, where args are "").
                Some("response.output_item.done") => {
                    if let Some(item) = ev.get("item") {
                        if let Some((call_id, name, mut args)) = call_from_item(item) {
                            let item_id = item.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            // Prefer accumulated streamed args if the item's own are empty/blank.
                            if args.trim().is_empty() || args == "{}" {
                                if let Some(acc) = arg_acc.get(&item_id) {
                                    if !acc.trim().is_empty() { args = acc.clone(); }
                                }
                            }
                            let key = if call_id.is_empty() { format!("{name}:{args}") } else { call_id.clone() };
                            if started.insert(key) {
                                let id = if call_id.is_empty() { format!("muse-tool-{}", calls.len()) } else { call_id.clone() };
                                let input: serde_json::Value = serde_json::from_str(&args).unwrap_or(json!({}));
                                on_event(StreamEvent::ToolUse { id: id.clone(), name: name.clone(), input });
                                calls.push((id, name, args));
                            }
                        }
                    }
                }
                Some("response.completed") => {
                    if let Some(st) = ev.get("response").and_then(|r| r.get("status")).and_then(|s| s.as_str()) {
                        stop_reason = st.to_string();
                    }
                    // Token usage lives on response.usage (Responses API shape).
                    if let Some(us) = ev.get("response").and_then(|r| r.get("usage")) {
                        if let Some(i) = us.get("input_tokens").and_then(|x| x.as_u64()) { u_in = i; }
                        if let Some(o) = us.get("output_tokens").and_then(|x| x.as_u64()) { u_out = o; }
                        if let Some(c) = us.get("input_tokens_details").and_then(|d| d.get("cached_tokens")).and_then(|x| x.as_u64()) { u_cr = c; }
                        // OpenCode parity: the Responses API also reports cache WRITE
                        // tokens — bill them at the write rate, not full input price.
                        if let Some(w) = us.get("input_tokens_details").and_then(|d| d.get("cache_write_tokens")).and_then(|x| x.as_u64()) { u_cw = w; }
                    }
                    // Fallback: if no text deltas arrived, recover text + calls
                    // from the completed output[].
                    if answer.is_empty() {
                        if let Some(out) = ev.get("response").and_then(|r| r.get("output")) {
                            answer = extract_output_text(out);
                            if calls.is_empty() {
                                if let Some(items) = out.as_array() {
                                    for it in items {
                                        if let Some((cid, name, args)) = call_from_item(it) {
                                            let id = if cid.is_empty() { format!("muse-tool-{}", calls.len()) } else { cid };
                                            let input: serde_json::Value = serde_json::from_str(&args).unwrap_or(json!({}));
                                            on_event(StreamEvent::ToolUse { id: id.clone(), name: name.clone(), input });
                                            calls.push((id, name, args));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let fresh_in = u_in.saturating_sub(u_cr).saturating_sub(u_cw);
                    on_event(StreamEvent::Usage { input: fresh_in, output: u_out, cache_read: u_cr, cache_write: u_cw, cache_write_5m: 0, cache_write_1h: 0, context_window: 0 });
                    on_event(StreamEvent::Done { stop_reason: stop_reason.clone() });
                    saw_done = true;
                }
                Some("response.failed") | Some("response.error") | Some("error") => {
                    let msg = ev.get("response").and_then(|r| r.get("error")).and_then(|e| e.get("message")).and_then(|m| m.as_str())
                        .or_else(|| ev.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()))
                        .or_else(|| ev.get("message").and_then(|m| m.as_str()))
                        .unwrap_or("muse stream error");
                    on_event(StreamEvent::Error { text: msg.to_string() });
                    return Err(msg.to_string());
                }
                _ => {}
            }
        }
    }

    // Safety net: if the stream closed without a response.completed frame (proxy
    // cut, etc.), still resolve the UI spinner.
    if !saw_done { on_event(StreamEvent::Done { stop_reason: stop_reason.clone() }); }

    // Assemble the assistant message the agent loop stores.
    let mut tool_calls_json = Vec::new();
    for (id, name, args) in &calls {
        tool_calls_json.push(json!({ "id": id, "type": "function", "function": { "name": name, "arguments": args } }));
    }
    let mut assistant = json!({ "role": "assistant", "content": answer });
    if !tool_calls_json.is_empty() {
        assistant["tool_calls"] = json!(tool_calls_json);
        stop_reason = "tool_use".into();
    }
    Ok((assistant, stop_reason))
}
