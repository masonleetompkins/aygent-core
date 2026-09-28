// AYGENT — Tools registry (extensible agent capabilities).
//
// A "tool" here is a named, agent-callable capability shown in the Tools tab as
// an enable/disable/delete block. Two kinds:
//   - BUILTIN: native Rust (e.g. generate_pdf). Can be disabled, not deleted.
//   - COMPOSED: user-authored (v1) = a saved instruction + a curated subset of
//     the base file tools it may use. NO arbitrary code execution (that's a
//     scoped v2 behind a real sandbox) — keeps AYGENT's OS-enforced jail intact.
//
// STORAGE: tools live in app data (global across folders); enabled-state is
// per-AGENT (a tool can be on for one agent, off for another).
//   <app_data>/tools/registry.json          -> [ToolDef, ...]
//   <app_data>/tools/enabled/<scope>.json   -> { "<tool_id>": true/false }
//
// SCOPE KEY (shared-context fix): the key used to be FNV(folder_path), which
// meant two agents pointed at the same folder silently SHARED tool enablement
// and config — toggle Allow Shell Access on the cheap model and you toggled it on the
// other one. Identity is the AGENT, not the path (same class of bug as the
// 08-03 ghost folder: identity keyed by path instead of by agent). Callers now
// pass an agent id; a legacy folder-keyed file is read once and MIGRATED to the
// agent key on first access, so nobody loses their toggles.
//
// The base file tools (read_file/write_file/list_files) are always available to
// the agent and are NOT in this registry — this registry is the EXTRA layer.

use std::fs;
use std::path::{Path, PathBuf};

/// One registered tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDef {
    pub id: String,
    pub name: String,           // agent-facing tool name (snake_case), e.g. "generate_pdf"
    pub display_name: String,   // human label for the UI block
    pub description: String,    // what it does (also fed to the model)
    /// "builtin" | "composed"
    pub kind: String,
    #[serde(default)]
    pub builtin: bool,          // true => cannot be deleted (only disabled)
    /// COMPOSED tools: the instruction the agent follows when this tool is used,
    /// plus which base tools it may call. Empty for builtins.
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>, // subset of ["read_file","write_file","list_files","generate_pdf"]
}

/// WHO tool state belongs to. `agent_id` is the identity; `legacy_folder` is
/// only used to migrate a pre-existing folder-keyed file exactly once.
#[derive(Debug, Clone)]
pub struct Scope {
    pub agent_id: String,
    pub legacy_folder: Option<String>,
}

impl Scope {
    pub fn new(agent_id: &str, legacy_folder: Option<&str>) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            legacy_folder: legacy_folder.map(|s| s.to_string()),
        }
    }
}

/// FNV-1a hash — the legacy folder-key scheme, kept ONLY to find and migrate
/// pre-existing folder-keyed files.
fn fnv(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() { hash ^= *b as u64; hash = hash.wrapping_mul(0x100000001b3); }
    format!("{hash:016x}")
}

/// The storage key for a scope. Agent ids are app-generated (base36, no path
/// separators) so they're already filename-safe; we sanitize anyway — never
/// trust an id straight into a path.
fn scope_key(scope: &str) -> String {
    let clean: String = scope.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if clean.is_empty() { fnv(scope) } else { clean }
}

/// One-time migration: if `agent_path` doesn't exist but a legacy folder-keyed
/// file does, copy it across. Best-effort — a failure just means defaults.
fn migrate_legacy(agent_path: &Path, dir: &Path, legacy_folder: Option<&str>) {
    if agent_path.exists() { return; }
    let Some(folder) = legacy_folder else { return };
    if folder.is_empty() { return; }
    let legacy = dir.join(format!("{}.json", fnv(folder)));
    if legacy.is_file() {
        let _ = fs::copy(&legacy, agent_path);
    }
}

fn tools_dir(app_data: &Path) -> Result<PathBuf, String> {
    let d = app_data.join("tools");
    fs::create_dir_all(&d).map_err(|e| format!("mkdir tools: {e}"))?;
    Ok(d)
}
fn registry_path(app_data: &Path) -> Result<PathBuf, String> { Ok(tools_dir(app_data)?.join("registry.json")) }
fn enabled_path(app_data: &Path, scope: &Scope) -> Result<PathBuf, String> {
    let d = tools_dir(app_data)?.join("enabled");
    fs::create_dir_all(&d).map_err(|e| format!("mkdir enabled: {e}"))?;
    let p = d.join(format!("{}.json", scope_key(&scope.agent_id)));
    migrate_legacy(&p, &d, scope.legacy_folder.as_deref());
    Ok(p)
}
/// Per-AGENT config VALUES for tools: { "<tool_id>": { "<key>": <value>, ... } }
fn config_path(app_data: &Path, scope: &Scope) -> Result<PathBuf, String> {
    let d = tools_dir(app_data)?.join("config");
    fs::create_dir_all(&d).map_err(|e| format!("mkdir config: {e}"))?;
    let p = d.join(format!("{}.json", scope_key(&scope.agent_id)));
    migrate_legacy(&p, &d, scope.legacy_folder.as_deref());
    Ok(p)
}

/// Load all tool config values for a folder (tool_id -> {key: value}).
pub fn load_config(app_data: &Path, scope: &Scope) -> serde_json::Value {
    fs::read_to_string(config_path(app_data, scope).unwrap_or_default())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Config values for ONE tool in a folder ({} if unset).
pub fn tool_config(app_data: &Path, scope: &Scope, tool_id: &str) -> serde_json::Value {
    load_config(app_data, scope).get(tool_id).cloned().unwrap_or_else(|| serde_json::json!({}))
}

/// Save config values for one tool (merges into the folder's config file).
pub fn set_tool_config(app_data: &Path, scope: &Scope, tool_id: &str, values: serde_json::Value) -> Result<(), String> {
    let mut all = load_config(app_data, scope);
    if !all.is_object() { all = serde_json::json!({}); }
    all[tool_id] = values;
    let text = serde_json::to_string_pretty(&all).map_err(|e| format!("serialize config: {e}"))?;
    fs::write(config_path(app_data, scope)?, text).map_err(|e| format!("write config: {e}"))
}

/// The built-in tools that always exist in the registry (seeded on first read).
fn builtins() -> Vec<ToolDef> {
    vec![
        ToolDef {
            id: "builtin.pdf".into(),
            name: "generate_pdf".into(),
            display_name: "PDF Generator".into(),
            description: "Create a PDF document from markdown/text content, saved into the agent folder. \
                Args: title (string), content (markdown string), output_path (e.g. \"report.pdf\").".into(),
            kind: "builtin".into(),
            builtin: true,
            instructions: String::new(),
            allowed_tools: vec![],
        },
        ToolDef {
            id: "builtin.whisper".into(),
            name: "transcribe_audio".into(),
            display_name: "Whisper Transcription".into(),
            description: "Transcribe audio files to text with OpenAI Whisper (needs an OpenAI key in \
                Settings). Powers the mic button too. Args: path (audio file in the agent folder).".into(),
            kind: "builtin".into(),
            builtin: true,
            instructions: String::new(),
            allowed_tools: vec![],
        },
        ToolDef {
            id: "builtin.web".into(),
            name: "fetch_url".into(),
            display_name: "Web Fetch".into(),
            description: "Fetch a web page or API over HTTPS and return its readable text (HTML stripped \
                to prose). Lets an agent read current info from the internet. The daemon has no network; \
                the request is made on the privileged side and only extracted text is returned. \
                Args: url (http/https).".into(),
            kind: "builtin".into(),
            builtin: true,
            instructions: String::new(),
            allowed_tools: vec![],
        },
        ToolDef {
            id: "builtin.search".into(),
            name: "web_search".into(),
            display_name: "Web Search".into(),
            description: "Search the web (DuckDuckGo, no key needed) and return the top hits as \
                title + url + snippet. Use when the user asks what's current, or to find a page \
                to read with fetch_url. The daemon has no network; the search runs on the \
                privileged side and only titles/urls/snippets are returned. \
                Args: query (string).".into(),
            kind: "builtin".into(),
            builtin: true,
            instructions: String::new(),
            allowed_tools: vec![],
        },
    ]
}

/// Config SCHEMA for a tool (what settings it exposes). The UI renders controls
/// from this; the executor reads the saved values. Only built-ins declare a
/// schema for now. Returns [] for tools with no config.
pub fn config_schema(tool_id: &str) -> serde_json::Value {
    match tool_id {
        "builtin.pdf" => serde_json::json!([
            { "key": "font", "label": "Font", "type": "font", "default": "bundled",
              "help": "Bundled always works; system fonts fall back to bundled if they fail to load." },
            { "key": "text_color", "label": "Text color", "type": "color", "default": "#111111" },
            { "key": "heading_color", "label": "Heading color", "type": "color", "default": "#111111" },
            { "key": "page_size", "label": "Page size", "type": "select", "default": "Letter",
              "options": ["Letter", "A4"] },
            { "key": "font_size", "label": "Base font size", "type": "number", "default": 11, "min": 8, "max": 18 },
            { "key": "margin", "label": "Margin (mm)", "type": "number", "default": 18, "min": 8, "max": 40 }
        ]),
        _ => serde_json::json!([]),
    }
}

/// Load the full registry, seeding built-ins + merging any saved tools. Built-in
/// defs are always refreshed from code (so descriptions stay current); saved
/// user/composed tools are loaded as-is.
pub fn load_registry(app_data: &Path) -> Vec<ToolDef> {
    let mut out = builtins();
    if let Ok(text) = fs::read_to_string(registry_path(app_data).unwrap_or_default()) {
        if let Ok(saved) = serde_json::from_str::<Vec<ToolDef>>(&text) {
            for t in saved {
                if t.builtin { continue; } // builtins come from code, not disk
                out.push(t);
            }
        }
    }
    out
}

/// Persist only the NON-builtin tools (builtins are code-defined).
fn save_registry(app_data: &Path, all: &[ToolDef]) -> Result<(), String> {
    let user: Vec<&ToolDef> = all.iter().filter(|t| !t.builtin).collect();
    let text = serde_json::to_string_pretty(&user).map_err(|e| format!("serialize: {e}"))?;
    fs::write(registry_path(app_data)?, text).map_err(|e| format!("write registry: {e}"))
}

/// Create or update a composed (user) tool.
pub fn upsert_tool(app_data: &Path, tool: ToolDef) -> Result<(), String> {
    if tool.builtin { return Err("cannot modify a built-in tool".into()); }
    let mut all = load_registry(app_data);
    if let Some(existing) = all.iter_mut().find(|t| t.id == tool.id && !t.builtin) {
        *existing = tool;
    } else {
        all.push(tool);
    }
    save_registry(app_data, &all)
}

/// Delete a composed (user) tool. Built-ins cannot be deleted.
pub fn delete_tool(app_data: &Path, id: &str) -> Result<(), String> {
    let mut all = load_registry(app_data);
    let before = all.len();
    all.retain(|t| !(t.id == id && !t.builtin));
    if all.len() == before {
        return Err("tool not found or is a built-in (cannot delete)".into());
    }
    save_registry(app_data, &all)
}

/// Per-AGENT enabled map. Missing entry => built-ins default ON, composed default OFF.
pub fn load_enabled(app_data: &Path, scope: &Scope) -> std::collections::HashMap<String, bool> {
    fs::read_to_string(enabled_path(app_data, scope).unwrap_or_default())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn set_enabled(app_data: &Path, scope: &Scope, id: &str, on: bool) -> Result<(), String> {
    let mut map = load_enabled(app_data, scope);
    map.insert(id.to_string(), on);
    let text = serde_json::to_string_pretty(&map).map_err(|e| format!("serialize: {e}"))?;
    fs::write(enabled_path(app_data, scope)?, text).map_err(|e| format!("write enabled: {e}"))
}

/// Resolve which tools are ENABLED for an agent (with defaults applied).
pub fn enabled_tools(app_data: &Path, scope: &Scope) -> Vec<ToolDef> {
    let all = load_registry(app_data);
    let map = load_enabled(app_data, scope);
    all.into_iter().filter(|t| {
        match map.get(&t.id) {
            Some(v) => *v,
            None => t.builtin, // builtins default on, composed default off
        }
    }).collect()
}
