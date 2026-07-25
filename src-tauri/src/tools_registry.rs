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
// per-folder (a tool can be on for one project, off for another).
//   <app_data>/tools/registry.json      -> [ToolDef, ...]
//   <app_data>/tools/enabled/<folderkey>.json -> { "<tool_id>": true/false }
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

/// FNV-1a folder key (same scheme as conversations/settings).
fn folder_key(folder: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in folder.as_bytes() { hash ^= *b as u64; hash = hash.wrapping_mul(0x100000001b3); }
    format!("{hash:016x}")
}

fn tools_dir(app_data: &Path) -> Result<PathBuf, String> {
    let d = app_data.join("tools");
    fs::create_dir_all(&d).map_err(|e| format!("mkdir tools: {e}"))?;
    Ok(d)
}
fn registry_path(app_data: &Path) -> Result<PathBuf, String> { Ok(tools_dir(app_data)?.join("registry.json")) }
fn enabled_path(app_data: &Path, folder: &str) -> Result<PathBuf, String> {
    let d = tools_dir(app_data)?.join("enabled");
    fs::create_dir_all(&d).map_err(|e| format!("mkdir enabled: {e}"))?;
    Ok(d.join(format!("{}.json", folder_key(folder))))
}

/// The built-in tools that always exist in the registry (seeded on first read).
fn builtins() -> Vec<ToolDef> {
    vec![ToolDef {
        id: "builtin.pdf".into(),
        name: "generate_pdf".into(),
        display_name: "PDF Generator".into(),
        description: "Create a PDF document from markdown/text content, saved into the agent folder. \
            Args: title (string), content (markdown string), output_path (e.g. \"report.pdf\").".into(),
        kind: "builtin".into(),
        builtin: true,
        instructions: String::new(),
        allowed_tools: vec![],
    }]
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

/// Per-folder enabled map. Missing entry => built-ins default ON, composed default OFF.
pub fn load_enabled(app_data: &Path, folder: &str) -> std::collections::HashMap<String, bool> {
    fs::read_to_string(enabled_path(app_data, folder).unwrap_or_default())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn set_enabled(app_data: &Path, folder: &str, id: &str, on: bool) -> Result<(), String> {
    let mut map = load_enabled(app_data, folder);
    map.insert(id.to_string(), on);
    let text = serde_json::to_string_pretty(&map).map_err(|e| format!("serialize: {e}"))?;
    fs::write(enabled_path(app_data, folder)?, text).map_err(|e| format!("write enabled: {e}"))
}

/// Resolve which tools are ENABLED for a folder (with defaults applied).
pub fn enabled_tools(app_data: &Path, folder: &str) -> Vec<ToolDef> {
    let all = load_registry(app_data);
    let map = load_enabled(app_data, folder);
    all.into_iter().filter(|t| {
        match map.get(&t.id) {
            Some(v) => *v,
            None => t.builtin, // builtins default on, composed default off
        }
    }).collect()
}
