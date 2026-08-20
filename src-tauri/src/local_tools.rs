// AYGENT — Local tool-use formatting + parsing, per native model family.
//
// Design (Mason's call): don't force every model through one fragile parser.
// We DETECT each model's format from its GGUF chat template (see gguf.rs), then
// use that family's NATIVE tool syntax to (a) tell the model what tools exist
// and how to call them, and (b) parse the tool calls it emits. Because our
// catalog is curated to 4 families, each gets its own correct handling — very
// little room for a parser to get confused.
//
// The file tools mirror the Anthropic path exactly (read_file/write_file/
// list_files) and execute through the SAME jailed broker (exec_tool in lib.rs).
// `whoami` is the one read-only, no-arg introspection tool (identity + model +
// capabilities) — parity with the cloud paths so local agents aren't a gap.

use serde_json::json;

/// A parsed tool call from model output.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
}

/// The tool descriptions injected into the system prompt. Kept plain + explicit
/// so even smaller models follow them; the CALL SYNTAX is family-specific below.
fn tools_description() -> &'static str {
    "You have these tools to work with files in the user's folder:\n\
     - read_file(path): read a UTF-8 text file (path relative to the folder root)\n\
     - write_file(path, content): create/overwrite a UTF-8 text file\n\
     - list_files(path): list a directory ('.' for the folder root)\n\
     - whoami(): report your own name, model, provider, and the tools you have \
     (read-only; takes no arguments) — use it when asked who or what you are\n\
     Only use a tool when the user's request needs it. After you receive a tool \
     result, continue and give the user a final answer."
}

/// Build the system prompt for a tool-capable local model, in its native style.
/// `format` comes from gguf::detect_tool_capability().
pub fn system_prompt_with_tools(base: &str, format: &str) -> String {
    let desc = tools_description();
    match format {
        "qwen" | "chatml" => format!(
            "{base}\n\n{desc}\n\n\
             To call a tool, output a line with ONLY this JSON wrapped in tags:\n\
             <tool_call>{{\"name\": \"read_file\", \"arguments\": {{\"path\": \"notes.md\"}}}}</tool_call>"
        ),
        "llama" => format!(
            "{base}\n\n{desc}\n\n\
             To call a tool, respond with ONLY a JSON object of the form:\n\
             {{\"name\": \"read_file\", \"parameters\": {{\"path\": \"notes.md\"}}}}"
        ),
        "mistral" => format!(
            "{base}\n\n{desc}\n\n\
             To call a tool, output: [TOOL_CALLS][{{\"name\": \"read_file\", \"arguments\": {{\"path\": \"notes.md\"}}}}]"
        ),
        "kimi" => format!(
            "{base}\n\n{desc}\n\n\
             To call a tool, output the JSON: {{\"name\": \"read_file\", \"arguments\": {{\"path\": \"notes.md\"}}}}"
        ),
        _ => format!("{base}\n\n{desc}"),
    }
}

/// Parse tool calls from a model's completed output, using the family's native
/// format. Robust: we scan for the family's signature, then extract the JSON
/// object(s). Falls back to a generic JSON-object scan so a model that strays
/// from its own format still works when the JSON is unambiguous.
pub fn parse_tool_calls(text: &str, format: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    // Family-native extraction first.
    match format {
        "qwen" | "chatml" => extract_tagged(text, "<tool_call>", "</tool_call>", &mut calls),
        "mistral" => extract_after_marker(text, "[TOOL_CALLS]", &mut calls),
        "llama" | "kimi" => extract_bare_json(text, &mut calls),
        _ => {}
    }

    // Generic fallback: if the native pass found nothing but the text clearly
    // contains a tool-shaped JSON object, try to recover it. This is bounded to
    // objects that have a "name" + args/arguments/parameters key, so ordinary
    // prose with braces won't false-positive.
    if calls.is_empty() {
        extract_bare_json(text, &mut calls);
    }
    calls
}

/// Extract JSON between explicit open/close tags (Qwen/ChatML <tool_call>).
fn extract_tagged(text: &str, open: &str, close: &str, out: &mut Vec<ToolCall>) {
    let mut rest = text;
    while let Some(s) = rest.find(open) {
        let after = &rest[s + open.len()..];
        let Some(e) = after.find(close) else { break };
        let body = after[..e].trim();
        if let Some(c) = call_from_json(body) { out.push(c); }
        rest = &after[e + close.len()..];
    }
}

/// Extract a JSON array/object following a marker (Mistral [TOOL_CALLS]).
fn extract_after_marker(text: &str, marker: &str, out: &mut Vec<ToolCall>) {
    if let Some(s) = text.find(marker) {
        let after = text[s + marker.len()..].trim_start();
        // The payload may be an array [ {...} ] or a bare object {...}.
        if after.starts_with('[') {
            if let Some(end) = matching_bracket(after, '[', ']') {
                if let Ok(arr) = serde_json::from_str::<serde_json::Value>(&after[..=end]) {
                    if let Some(items) = arr.as_array() {
                        for it in items { if let Some(c) = call_from_value(it) { out.push(c); } }
                    }
                }
            }
        } else if let Some(end) = matching_bracket(after, '{', '}') {
            if let Some(c) = call_from_json(&after[..=end]) { out.push(c); }
        }
    }
}

/// Scan for the first balanced JSON object that looks like a tool call.
fn extract_bare_json(text: &str, out: &mut Vec<ToolCall>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end_rel) = matching_bracket(&text[i..], '{', '}') {
                let slice = &text[i..=i + end_rel];
                if let Some(c) = call_from_json(slice) { out.push(c); return; }
                i += end_rel + 1;
                continue;
            }
        }
        i += 1;
    }
}

/// Find the index (into `s`) of the bracket matching the FIRST `open` at s[0].
fn matching_bracket(s: &str, open: char, close: char) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first().copied()? != open as u8 { return None; }
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        if in_str {
            if esc { esc = false; }
            else if c == '\\' { esc = true; }
            else if c == '"' { in_str = false; }
            continue;
        }
        match c {
            '"' => in_str = true,
            x if x == open => depth += 1,
            x if x == close => { depth -= 1; if depth == 0 { return Some(i); } }
            _ => {}
        }
    }
    None
}

/// Parse a JSON string into a ToolCall (accepting arguments/parameters/input).
fn call_from_json(s: &str) -> Option<ToolCall> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    call_from_value(&v)
}

fn call_from_value(v: &serde_json::Value) -> Option<ToolCall> {
    let name = v.get("name").and_then(|n| n.as_str())?.to_string();
    // Only accept our known tools \u2014 ignore hallucinated tool names.
    if !matches!(name.as_str(), "read_file" | "write_file" | "list_files" | "whoami") { return None; }
    let mut input = v.get("arguments")
        .or_else(|| v.get("parameters"))
        .or_else(|| v.get("input"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    // REPAIR a common small-model malformation (Mason 08-19, Gwen/Qwen3):
    // {"arguments": {"path": "...", "arguments": {"content": "..."}}} \u2014 the
    // model nests a second arguments object. Hoist its keys up (without
    // clobbering real top-level keys) so the write actually carries content.
    if let Some(obj) = input.as_object_mut() {
        let nested = obj.get("arguments").or_else(|| obj.get("parameters")).cloned();
        if let Some(serde_json::Value::Object(inner)) = nested {
            obj.remove("arguments");
            obj.remove("parameters");
            for (k, val) in inner { obj.entry(k).or_insert(val); }
        }
    }
    Some(ToolCall { name, input })
}

/// Format a tool result to feed back to the model, in the family's style.
pub fn format_tool_result(format: &str, name: &str, result: &str, is_error: bool) -> String {
    let status = if is_error { "ERROR" } else { "ok" };
    match format {
        "qwen" | "chatml" => format!("<tool_response>\n{{\"tool\":\"{name}\",\"status\":\"{status}\",\"result\":{}}}\n</tool_response>", json!(result)),
        _ => format!("[TOOL RESULT: {name} ({status})]\n{result}"),
    }
}

/// True if the text contains this family's tool-call SIGNATURE, even if no
/// call could be parsed out of it \u2014 used to give the model corrective
/// feedback instead of silently dropping a malformed call (Mason 08-19).
pub fn has_tool_marker(text: &str, format: &str) -> bool {
    match format {
        "qwen" | "chatml" => text.contains("<tool_call>") || text.contains("</tool_call>"),
        "mistral" => text.contains("[TOOL_CALLS]"),
        _ => text.contains("\"name\"") && (text.contains("\"arguments\"") || text.contains("\"parameters\"")),
    }
}

/// Remove <think>...</think> reasoning blocks before parsing tool calls, so a
/// hypothetical call the model MUSED about inside its thinking ("I could call
/// write_file like {...}") is never executed. An unclosed <think> (mid-stream
/// or runaway) strips to the end. Display-side collapsing is the UI's job \u2014
/// this is only the parse guard.
pub fn strip_think(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        match rest.find("<think>") {
            None => { out.push_str(rest); break; }
            Some(s) => {
                out.push_str(&rest[..s]);
                match rest[s..].find("</think>") {
                    None => break, // unclosed \u2014 drop the tail
                    Some(e_rel) => { rest = &rest[s + e_rel + "</think>".len()..]; }
                }
            }
        }
    }
    out
}

/// The tail of an UNCLOSED <think> block (text after the last `<think>` that
/// has no matching `</think>`), or None if every think block is closed.
pub fn unclosed_think_tail(text: &str) -> Option<&str> {
    let s = text.rfind("<think>")?;
    let tail = &text[s + "<think>".len()..];
    if tail.contains("</think>") { return None; }
    Some(tail)
}

/// RESCUE a tool call emitted inside an unclosed <think> (Mason 08-19, Gwen/
/// Qwen3): the model opens <think>, decides to act, and emits the call JSON —
/// often with only the closing </tool_call> tag — without ever closing the
/// think. strip_think() rightly drops unclosed thinks, but here it swallowed a
/// REAL call ("memory call stuffed in a thought"). Executing from a CLOSED
/// think stays forbidden — that is musing followed by a real answer. Recover
/// ONLY when:
///   1. the <think> is unclosed — there is no visible answer at all, and
///   2. the tail ENDS with the completed call (whitespace aside) — a model
///      that kept writing prose after the JSON was musing, not calling.
pub fn rescue_call_from_unclosed_think(text: &str, format: &str) -> Vec<ToolCall> {
    let Some(tail) = unclosed_think_tail(text) else { return Vec::new() };
    let t = tail.trim_end();
    if !(t.ends_with("</tool_call>") || t.ends_with('}') || t.ends_with(']')) {
        return Vec::new();
    }
    parse_tool_calls(t, format)
}
