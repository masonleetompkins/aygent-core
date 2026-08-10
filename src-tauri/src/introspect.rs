// AYGENT — Agent self-introspection ("whoami").
//
// An agent should be able to answer "who am I, what am I running on, and what
// can I actually do?" from INSIDE a turn — even when none of it is written in
// its Soul. The model is infrastructure; the identity + configuration live in
// the SQLite spine, the connections tables, the tools registry, and the MCP
// store. This module gathers all of that into ONE read-only report the agent
// gets back from the `whoami` tool.
//
// READ-ONLY BY CONSTRUCTION: every call here is a SELECT / load. There is no
// setter, and the tool exposes no arguments — an agent can SEE its config but
// cannot change it. Changing configuration stays a human action in the UI
// (Settings, Connections, Tools), exactly as before.
//
// SINGLE SOURCE OF TRUTH: the capability section is assembled the SAME way the
// agent loop assembles the model's tool list (connections::enabled_providers_
// for_agent + disabled_tools + Connector::tools_granted, and tools_registry::
// load_enabled). If the real grant changes, this report changes with it — it
// can never drift into telling the agent it has a capability it wasn't given.

use crate::writer::Db;
use tauri::AppHandle;

/// The always-available core file tools (mirror of lib.rs CORE_TOOLS). These
/// are unconditional in the agent loop — not in any registry — so they must be
/// listed here or the answer is a lie.
const CORE_TOOLS: &[(&str, &str)] = &[
    ("read_file", "Read a UTF-8 text file inside the agent folder."),
    ("write_file", "Create or overwrite a text file inside the agent folder."),
    ("list_files", "List directory entries inside the agent folder."),
    ("rename_file", "Rename or move a file inside the agent folder."),
    ("delete_file", "Delete a file inside the agent folder."),
];

/// Build the whoami report for one agent as human-readable markdown. `agent_id`
/// is the resolved acting agent; `folder` is its folder path (for per-agent
/// tool enablement scope). Returns text the model reads back verbatim.
pub fn build_whoami(
    app: &AppHandle,
    db: &Db,
    agent_id: &str,
    folder: Option<&str>,
) -> String {
    let mut out = String::new();

    // ── IDENTITY + MODEL CONFIG ──────────────────────────────────────────
    // Straight from the agent profile — the single source of truth the UI's
    // Agents/Settings tabs write to. True even when the Soul says nothing.
    let agent = crate::repo::get_agent(db, agent_id).ok().flatten();
    out.push_str("## Identity\n");
    match &agent {
        Some(a) => {
            out.push_str(&format!("- Name: {}\n", non_empty(&a.name, "(unnamed)")));
            out.push_str(&format!("- Agent id: {}\n", a.id));
            if !a.icon.trim().is_empty() {
                out.push_str(&format!("- Icon: {}\n", a.icon));
            }
            out.push_str(&format!(
                "- Context mode: {}\n",
                non_empty(&a.context_mode, "isolated")
            ));
            if !a.folder_path.trim().is_empty() {
                out.push_str(&format!("- Folder (your jail root): {}\n", a.folder_path));
            }
        }
        None => {
            out.push_str(&format!("- Name: (profile not found for id {agent_id})\n"));
        }
    }

    out.push_str("\n## Model configuration\n");
    match &agent {
        Some(a) => {
            let provider = if a.provider.trim().is_empty() {
                "anthropic (default)".to_string()
            } else {
                a.provider.clone()
            };
            out.push_str(&format!("- Provider: {provider}\n"));
            let model = if a.model.trim().is_empty() {
                match a.provider.as_str() {
                    "" | "anthropic" => "auto (prefers a Haiku model, else the first available)".to_string(),
                    "local" => "(no local model file selected)".to_string(),
                    other => format!("(no {other} model selected)"),
                }
            } else if a.provider == "local" {
                // A local model's "id" is an absolute GGUF path; show just the file.
                let leaf = std::path::Path::new(&a.model)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&a.model);
                format!("{leaf} (local GGUF, runs in-process — no key, no network)")
            } else {
                a.model.clone()
            };
            out.push_str(&format!("- Model: {model}\n"));
        }
        None => out.push_str("- (unknown — no profile)\n"),
    }

    // ── CAPABILITIES (grouped by origin) ─────────────────────────────────
    out.push_str("\n## Tools & capabilities\n");

    // 1. CORE — jailed file tools, always on, cannot be disabled.
    out.push_str("\n**Core (always on):**\n");
    for (name, desc) in CORE_TOOLS {
        out.push_str(&format!("- `{name}` — {desc}\n"));
    }

    // 2. BUILT-IN REGISTRY TOOLS — pdf / whisper / fetch_url, per-agent toggle.
    if let Ok(ad) = crate::paths::state_dir(app) {
        let scope = crate::tools_registry::Scope::new(agent_id, folder);
        let enabled_map = crate::tools_registry::load_enabled(&ad, &scope);
        let mut builtin_lines = Vec::new();
        let mut skill_lines = Vec::new();
        for t in crate::tools_registry::load_registry(&ad) {
            let on = match enabled_map.get(&t.id) {
                Some(v) => *v,
                None => t.builtin, // builtins default on, composed (skills) default off
            };
            if t.kind == "composed" {
                // Skills = saved procedures (how-I-want-work-done), not a machine
                // capability. Listed separately so the two never blur together.
                if on {
                    skill_lines.push(format!("- `{}` — {}\n", t.name, t.display_name));
                }
            } else if on {
                builtin_lines.push(format!("- `{}` — {}\n", t.name, t.display_name));
            }
        }
        if !builtin_lines.is_empty() {
            out.push_str("\n**Built-in (enabled for you):**\n");
            for l in builtin_lines {
                out.push_str(&l);
            }
        }

        // 3. CONNECTION TOOLS — one row per enabled provider, listing the tools
        //    actually granted (same call the agent loop makes). Rendered as a
        //    GFM TABLE (Service | Tools | Access) — the separator row is on its
        //    OWN line so AYGENT's markdown renderer recognizes it as a table
        //    (Mason 08-10: a one-line table renders as a wall of raw pipes).
        let providers = crate::connections::enabled_providers_for_agent(db, agent_id);
        let mut conn_rows: Vec<(String, usize, String)> = Vec::new();
        for (provider, _mode) in providers {
            let Some(def) = crate::connectors::by_id(&provider) else { continue };
            let off = crate::connections::disabled_tools(db, agent_id, &provider);
            let granted: Vec<&crate::connectors::ConnectorTool> =
                def.tools_granted(true, &off).collect();
            if granted.is_empty() {
                continue;
            }
            // Access summary: "Read + write" if any granted tool mutates, else
            // "Read-only" — derived from the SAME grant list, so it can't lie.
            let can_write = granted
                .iter()
                .any(|t| t.access == crate::connectors::Access::Write);
            let access = if can_write { "Read + write" } else { "Read-only" };
            conn_rows.push((def.label.to_string(), granted.len(), access.to_string()));
        }
        if conn_rows.is_empty() {
            out.push_str("\n**Connected accounts:** none enabled for you.\n");
        } else {
            out.push_str("\n**Connected accounts:**\n\n");
            out.push_str("| Service | Tools | Access |\n");
            out.push_str("|---|--:|---|\n");
            for (label, n, access) in conn_rows {
                // Escape any pipe in a label so it can't break the row.
                let safe = label.replace('|', "\\|");
                out.push_str(&format!("| {safe} | {n} | {access} |\n"));
            }
        }

        // 4. MCP SERVERS — enabled + running servers contribute namespaced tools.
        let mcp_lines = mcp_summary(app);
        if mcp_lines.is_empty() {
            out.push_str("\n**MCP servers:** none enabled.\n");
        } else {
            out.push_str("\n**MCP servers (external tools):**\n");
            for l in mcp_lines {
                out.push_str(&l);
            }
        }

        // 5. SKILLS — saved procedures, listed last (they grant no new reach).
        if !skill_lines.is_empty() {
            out.push_str("\n**Skills (saved procedures enabled for you):**\n");
            for l in skill_lines {
                out.push_str(&l);
            }
        }
    }

    out.push_str(
        "\n_You can SEE this configuration but not change it — changing your model, \
         connections, or tools is done by the user in the app's Settings / Connections / \
         Tools screens._",
    );
    out
}

/// One summary line per enabled+running MCP server: "Label (N tools): a, b, …".
fn mcp_summary(app: &AppHandle) -> Vec<String> {
    let mut lines = Vec::new();
    for cfg in crate::mcp::list(app) {
        if !cfg.enabled {
            continue;
        }
        // A server can be enabled but not yet running/handshaked — report both.
        match crate::mcp_client::get(&cfg.key) {
            Some(server) => {
                let names: Vec<String> = server
                    .tools()
                    .iter()
                    .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect();
                if names.is_empty() {
                    lines.push(format!("- {} (enabled, no tools discovered yet)\n", cfg.label));
                } else {
                    let shown: Vec<String> = names.iter().take(12).cloned().collect();
                    let more = if names.len() > shown.len() {
                        format!(" (+{} more)", names.len() - shown.len())
                    } else {
                        String::new()
                    };
                    lines.push(format!(
                        "- {} ({} tools): {}{}\n",
                        cfg.label,
                        names.len(),
                        shown.join(", "),
                        more
                    ));
                }
            }
            None => lines.push(format!("- {} (enabled, not started)\n", cfg.label)),
        }
    }
    lines
}

fn non_empty<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    if s.trim().is_empty() { fallback } else { s }
}
