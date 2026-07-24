// AYGENT — Minimal GGUF metadata reader.
//
// We parse the GGUF file HEADER directly (documented binary format) to read the
// couple of metadata keys we need for tool-use detection — chiefly
// `tokenizer.chat_template`. This is "detect, don't guess": a model either
// embeds a chat template (and we can identify its native tool format) or it
// doesn't. We do NOT depend on llama-cpp-2 exposing a metadata getter.
//
// GGUF layout (v2/v3), all little-endian:
//   magic: u32 = 0x46554747 ("GGUF")
//   version: u32
//   tensor_count: u64
//   metadata_kv_count: u64
//   then metadata_kv_count entries, each:
//     key: gguf_string  (u64 len + bytes)
//     value_type: u32   (enum below)
//     value: depends on type
// We only need string values (chat_template) and can SKIP the rest, so we
// implement full value-skipping but only surface strings.

use std::fs::File;
use std::io::{BufReader, Read};

const GGUF_MAGIC: u32 = 0x4655_4747; // "GGUF" little-endian

// GGUF metadata value types.
const T_UINT8: u32 = 0;
const T_INT8: u32 = 1;
const T_UINT16: u32 = 2;
const T_INT16: u32 = 3;
const T_UINT32: u32 = 4;
const T_INT32: u32 = 5;
const T_FLOAT32: u32 = 6;
const T_BOOL: u32 = 7;
const T_STRING: u32 = 8;
const T_ARRAY: u32 = 9;
const T_UINT64: u32 = 10;
const T_INT64: u32 = 11;
const T_FLOAT64: u32 = 12;

/// What we learned about a model's tool-calling capability.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolCapability {
    /// True if the model embeds a chat template we recognize as tool-capable.
    pub tools_supported: bool,
    /// Native format family: "qwen" | "llama" | "mistral" | "kimi" | "chatml"
    /// | "unknown". Drives which emit-template + parser we use.
    pub format: String,
    /// Short human note for the UI ("works with file tools" / "chat only").
    pub note: String,
}

/// Read a model's tool capability by inspecting its GGUF chat template.
pub fn detect_tool_capability(path: &str) -> ToolCapability {
    let template = read_chat_template(path).unwrap_or_default();
    classify_template(&template)
}

/// Classify a chat-template string into a native tool format family. The
/// heuristics mirror how llama.cpp identifies known templates: we look for the
/// signature tokens each family uses. A template that mentions tools/tool_calls
/// AND matches a family we support => tool-capable.
fn classify_template(tmpl: &str) -> ToolCapability {
    let t = tmpl.to_lowercase();
    let mentions_tools = t.contains("tool_call") || t.contains("tools") || t.contains("tool_calls")
        || t.contains("function") && t.contains("call");

    // Family detection by signature tokens.
    let (format, family_toolable) = if t.contains("<|im_start|>") && (t.contains("qwen") || t.contains("tool_call")) {
        // Qwen / Hermes-style ChatML with <tool_call> blocks.
        ("qwen", true)
    } else if t.contains("<|python_tag|>") || t.contains("<|start_header_id|>") {
        // Llama 3.x native tool calling.
        ("llama", true)
    } else if t.contains("[tool_calls]") || t.contains("[available_tools]") {
        // Mistral / Mixtral tool format.
        ("mistral", true)
    } else if t.contains("kimi") || t.contains("<|tool_calls_section_begin|>") {
        ("kimi", true)
    } else if t.contains("<|im_start|>") {
        // Generic ChatML without an obvious tool block — treat as chat-only for
        // reliability (we don't force tools on a template that never mentions them).
        ("chatml", false)
    } else {
        ("unknown", false)
    };

    let tools_supported = family_toolable && mentions_tools;
    let note = if tools_supported {
        "works with file tools".to_string()
    } else {
        "chat only".to_string()
    };
    ToolCapability { tools_supported, format: format.to_string(), note }
}

/// Extract `tokenizer.chat_template` (and the `.tool_use` variant if present)
/// from a GGUF file. Returns the tool_use template if it exists, else the base
/// template, else None. Reads only the header — never loads weights.
pub fn read_chat_template(path: &str) -> Option<String> {
    let f = File::open(path).ok()?;
    let mut r = BufReader::new(f);

    if read_u32(&mut r)? != GGUF_MAGIC { return None; }
    let _version = read_u32(&mut r)?;
    let _tensor_count = read_u64(&mut r)?;
    let kv_count = read_u64(&mut r)?;

    let mut base: Option<String> = None;
    let mut tool_use: Option<String> = None;

    for _ in 0..kv_count {
        let key = read_gguf_string(&mut r)?;
        let vtype = read_u32(&mut r)?;
        // We only capture the string values we care about; everything else is skipped.
        if vtype == T_STRING {
            let val = read_gguf_string(&mut r)?;
            match key.as_str() {
                "tokenizer.chat_template" => base = Some(val),
                // Some models ship named template variants; the tool one wins.
                "tokenizer.chat_template.tool_use" => tool_use = Some(val),
                _ => {}
            }
        } else if !skip_value(&mut r, vtype)? {
            return None; // malformed/unsupported — bail safely
        }
        // Early out if we already have the tool_use variant.
        if tool_use.is_some() { break; }
    }
    tool_use.or(base)
}

// ---- primitive readers ----------------------------------------------------

fn read_u32<R: Read>(r: &mut R) -> Option<u32> {
    let mut b = [0u8; 4]; r.read_exact(&mut b).ok()?; Some(u32::from_le_bytes(b))
}
fn read_u64<R: Read>(r: &mut R) -> Option<u64> {
    let mut b = [0u8; 8]; r.read_exact(&mut b).ok()?; Some(u64::from_le_bytes(b))
}
fn read_gguf_string<R: Read>(r: &mut R) -> Option<String> {
    let len = read_u64(r)? as usize;
    // Guard against absurd lengths (corrupt file) — chat templates are < ~1MB.
    if len > 8_000_000 { return None; }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Skip a metadata value of the given type without keeping it. Returns Some(true)
/// on success. Handles scalars, strings, and (recursively) arrays.
fn skip_value<R: Read>(r: &mut R, vtype: u32) -> Option<bool> {
    let n = match vtype {
        T_UINT8 | T_INT8 | T_BOOL => 1,
        T_UINT16 | T_INT16 => 2,
        T_UINT32 | T_INT32 | T_FLOAT32 => 4,
        T_UINT64 | T_INT64 | T_FLOAT64 => 8,
        T_STRING => { let _ = read_gguf_string(r)?; return Some(true); }
        T_ARRAY => {
            let elem_type = read_u32(r)?;
            let count = read_u64(r)?;
            for _ in 0..count {
                if elem_type == T_STRING { let _ = read_gguf_string(r)?; }
                else if !skip_value(r, elem_type)? { return None; }
            }
            return Some(true);
        }
        _ => return None, // unknown type
    };
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).ok()?;
    Some(true)
}
