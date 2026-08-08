// AYGENT — MCP manager (2026-08-06). The layer between the raw MCP client
// (mcp_client.rs = one stdio JSON-RPC server) and the rest of AYGENT: a registry
// of configured MCP servers (built-in catalog + user-added), which ones are
// enabled, how to START them (with a provisioned Node when needed), how to OFFER
// their tools to the agent, and how to ROUTE a tool call back to the right server.
//
// Storage: a small JSON file in app-data (<app_data>/mcp/servers.json) listing
// the user's configured servers. The built-in catalog (Premiere) is code; a
// catalog entry becomes a stored server when the user enables it.
//
// Tool namespacing: MCP tool names collide across servers (every server has
// `verify_*`, `get_*`, …). We expose each as `mcp__<key>__<tool>` to the agent,
// and split it back on dispatch. Underscores in <key> are avoided (keys are
// slugs). The double-underscore delimiter keeps the original tool name intact.

use std::path::PathBuf;
use tauri::AppHandle;

pub const TOOL_PREFIX: &str = "mcp__";

/// A configured MCP server (persisted). `key` is a stable slug (e.g. "premiere").
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct McpConfig {
    pub key: String,
    pub label: String,
    pub command: String,          // program to spawn (e.g. "premiere-pro-mcp" or "node")
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub enabled: bool,
    /// Uses the AYGENT-provisioned Node on PATH (true for node-based servers so a
    /// system Node isn't required).
    #[serde(default)]
    pub needs_node: bool,
    /// Uses the AYGENT-provisioned uv (Python toolchain) — true for Python MCP
    /// servers (e.g. Blender) so no system Python/uv is required.
    #[serde(default)]
    pub needs_uv: bool,
    /// Human note about what enabling installs / requires (shown in the UI).
    #[serde(default)]
    pub setup_note: String,
    /// Optional: a read-only tool the UI can call to verify readiness (e.g.
    /// Premiere's `verify_premiere_connection`).
    #[serde(default)]
    pub verify_tool: Option<String>,
    /// True for built-in catalog entries (can't be deleted, only disabled).
    #[serde(default)]
    pub builtin: bool,
}

fn mcp_dir(app: &AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?.join("mcp");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir mcp: {e}"))?;
    Ok(dir)
}
fn store_path(app: &AppHandle) -> Result<PathBuf, String> { Ok(mcp_dir(app)?.join("servers.json")) }

fn load_store(app: &AppHandle) -> Vec<McpConfig> {
    let Ok(p) = store_path(app) else { return vec![] };
    std::fs::read_to_string(&p).ok()
        .and_then(|t| serde_json::from_str::<Vec<McpConfig>>(&t).ok())
        .unwrap_or_default()
}
fn save_store(app: &AppHandle, list: &[McpConfig]) -> Result<(), String> {
    let p = store_path(app)?;
    std::fs::write(&p, serde_json::to_string_pretty(list).map_err(|e| format!("serialize: {e}"))?)
        .map_err(|e| format!("write mcp store: {e}"))
}

// ── Built-in catalog ─────────────────────────────────────────────────────────

/// The built-in MCP servers a user can one-click enable. Data, not code paths —
/// adding one = another entry here. Premiere is the first.
pub fn catalog() -> Vec<McpConfig> {
    vec![
        McpConfig {
            key: "premiere".into(),
            label: "Adobe Premiere Pro".into(),
            command: "premiere-pro-mcp".into(),
            args: vec![],
            env: vec![("PREMIERE_TEMP_DIR".into(), "/tmp/premiere-mcp-bridge".into())],
            enabled: false,
            needs_node: true,
            needs_uv: false,
            setup_note: "Installs the `adobe-premiere-pro-mcp` package + its CEP bridge into Premiere. \
                         After enabling, restart Premiere, open Window > Extensions > MCP Bridge (CEP), \
                         and click Start Bridge. AYGENT will verify the connection.".into(),
            verify_tool: Some("verify_premiere_connection".into()),
            builtin: true,
        },
        McpConfig {
            key: "blender".into(),
            label: "Blender".into(),
            // The official Blender MCP is a Python server launched via uv:
            // `uv run blender-mcp` (from manifest.json's mcp_config). needs_uv
            // provisions AYGENT's own uv + a managed Python — no system Python.
            command: "uv".into(),
            args: vec!["run".into(), "blender-mcp".into()],
            env: vec![],
            enabled: false,
            needs_node: false,
            needs_uv: true,
            setup_note: "Requires Blender 5.1+. Enabling installs the `blender-mcp` server into \
                         AYGENT's own space (portable uv + a managed Python — nothing touches your \
                         system). You must ALSO install the Blender add-on inside Blender: follow \
                         https://lab.blender.org/mcp-server/#addon (drag the Blender Lab repo + the \
                         add-on into Blender), then keep Blender open and click Verify. \
                         ⚠ SECURITY: the Blender MCP executes LLM-generated Python inside Blender \
                         with NO guardrails — it can modify or delete your data. Use a machine (or \
                         VM) without sensitive files.".into(),
            verify_tool: Some("get_objects_summary".into()),
            builtin: true,
        },
    ]
}

/// The full list the UI shows: built-in catalog merged with the stored state
/// (stored `enabled`/args/env override the catalog defaults) + any user-added
/// servers not in the catalog.
pub fn list(app: &AppHandle) -> Vec<McpConfig> {
    let stored = load_store(app);
    let mut out: Vec<McpConfig> = Vec::new();
    // Catalog first (with stored overrides applied).
    for cat in catalog() {
        if let Some(s) = stored.iter().find(|s| s.key == cat.key) {
            let mut merged = cat.clone();
            merged.enabled = s.enabled;
            if !s.args.is_empty() { merged.args = s.args.clone(); }
            if !s.env.is_empty() { merged.env = s.env.clone(); }
            out.push(merged);
        } else {
            out.push(cat);
        }
    }
    // User-added (non-catalog) servers.
    for s in stored {
        if !catalog().iter().any(|c| c.key == s.key) { out.push(s); }
    }
    out
}

pub fn get_config(app: &AppHandle, key: &str) -> Option<McpConfig> {
    list(app).into_iter().find(|c| c.key == key)
}

/// Persist a config (upsert into the store). Built-in catalog entries are stored
/// too once touched (so enabled/args/env persist).
pub fn upsert(app: &AppHandle, cfg: McpConfig) -> Result<(), String> {
    let mut stored = load_store(app);
    if let Some(existing) = stored.iter_mut().find(|s| s.key == cfg.key) { *existing = cfg; }
    else { stored.push(cfg); }
    save_store(app, &stored)
}

/// Add a user MCP server from the web (command + args + env). key is slugified
/// from the label; must not collide with a catalog key.
pub fn add_custom(app: &AppHandle, label: &str, command: &str, args: Vec<String>, env: Vec<(String, String)>, needs_node: bool) -> Result<String, String> {
    let key = slugify(label);
    if key.is_empty() { return Err("give the server a name".into()); }
    if catalog().iter().any(|c| c.key == key) { return Err("that name collides with a built-in — pick another".into()); }
    let cfg = McpConfig {
        key: key.clone(), label: label.trim().to_string(),
        command: command.trim().to_string(), args, env,
        enabled: false, needs_node, needs_uv: false,
        setup_note: "User-added MCP server.".into(),
        verify_tool: None, builtin: false,
    };
    upsert(app, cfg)?;
    Ok(key)
}

/// Remove a user server config (built-ins can only be disabled, not removed).
pub fn remove_custom(app: &AppHandle, key: &str) -> Result<(), String> {
    if catalog().iter().any(|c| c.key == key) { return Err("built-in servers can be disabled but not removed".into()); }
    mcp_client::stop(key);
    let mut stored = load_store(app);
    stored.retain(|s| s.key != key);
    save_store(app, &stored)
}

fn slugify(s: &str) -> String {
    s.trim().to_lowercase().chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-').to_string()
        .split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

use crate::mcp_client;

/// Resolve the launch command for a config: if it needs Node, run through the
/// provisioned node/npm bin (so no system Node is required). Returns
/// (program, args, env) ready for mcp_client::start. `command` is the global bin
/// name the npm package installs (e.g. "premiere-pro-mcp"); with provisioned
/// node we resolve it under runtime/node/bin.
fn launch_spec(app: &AppHandle, cfg: &McpConfig) -> Result<(String, Vec<String>, Vec<(String, String)>), String> {
    let mut env = cfg.env.clone();
    if cfg.needs_uv {
        // Python MCP servers (e.g. Blender) launch through OUR provisioned uv,
        // with uv's cache/data/managed-Python pinned inside our runtime tree so
        // nothing touches the user's home or system Python.
        let path = crate::provision::provisioned_uv_path(app);
        env.push(("PATH".into(), path));
        for kv in crate::provision::uv_env(app) { env.push(kv); }
        // Use the ABSOLUTE uv path as the program so we never depend on PATH
        // resolution inside the jailed spawn.
        if let Some(uv) = crate::provision::uv_bin(app) {
            return Ok((uv.to_string_lossy().to_string(), cfg.args.clone(), env));
        }
        // uv not provisioned yet — fall through to the bare command (will error
        // clearly if uv isn't on PATH), but this path is normally unreachable
        // because mcp_enable provisions uv before start.
    }
    if cfg.needs_node {
        // Ensure the provisioned node's bin is on PATH so a globally-installed
        // (into our node prefix) MCP CLI resolves.
        let path = crate::provision::provisioned_path(app);
        env.push(("PATH".into(), format!("{path}:/usr/bin:/bin")));
        // The global bin the npm pkg installs lives under our node prefix's bin.
        if let Some(nd) = crate::provision::node_bin(app).and_then(|p| p.parent().map(|d| d.to_path_buf())) {
            let candidate = nd.join(&cfg.command);
            if candidate.is_file() {
                return Ok((candidate.to_string_lossy().to_string(), cfg.args.clone(), env));
            }
        }
    }
    Ok((cfg.command.clone(), cfg.args.clone(), env))
}

/// START an enabled MCP server (idempotent). Blocks through the handshake.
pub fn start_server(app: &AppHandle, key: &str) -> Result<std::sync::Arc<mcp_client::McpServer>, String> {
    let cfg = get_config(app, key).ok_or_else(|| format!("unknown MCP server `{key}`"))?;
    let (program, args, env) = launch_spec(app, &cfg)?;
    mcp_client::start(key, &program, &args, &env, None)
}

/// Ensure every ENABLED server is running (called before assembling agent tools).
/// Best-effort: a server that fails to start is logged + skipped (its tools just
/// won't be offered) rather than breaking the turn.
pub fn ensure_enabled_running(app: &AppHandle) {
    for cfg in list(app) {
        if cfg.enabled && !mcp_client::is_running(&cfg.key) {
            if let Err(e) = start_server(app, &cfg.key) {
                eprintln!("[aygent][mcp] enabled server `{}` failed to start: {e}", cfg.key);
            }
        }
    }
}

// ── Agent-loop bridge: offer tools + route calls ─────────────────────────────

/// The AYGENT-format tool schemas contributed by all RUNNING enabled servers,
/// namespaced `mcp__<key>__<tool>`. Plus an instruction block naming them.
pub fn agent_tool_schemas(app: &AppHandle) -> (Vec<serde_json::Value>, String) {
    let mut schemas = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for cfg in list(app) {
        if !cfg.enabled { continue; }
        let Some(server) = mcp_client::get(&cfg.key) else { continue };
        let mut names: Vec<String> = Vec::new();
        for t in server.tools() {
            let Some(orig) = t.get("name").and_then(|n| n.as_str()) else { continue };
            let namespaced = format!("{TOOL_PREFIX}{}__{orig}", cfg.key);
            let desc = t.get("description").and_then(|d| d.as_str()).unwrap_or("");
            let input_schema = t.get("inputSchema").cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
            schemas.push(serde_json::json!({
                "name": namespaced,
                "description": desc,
                "input_schema": input_schema,
            }));
            names.push(orig.to_string());
        }
        if !names.is_empty() {
            let shown: Vec<String> = names.iter().take(12).cloned().collect();
            let more = if names.len() > shown.len() { format!(" (+{} more)", names.len() - shown.len()) } else { String::new() };
            lines.push(format!("{} ({} tools): {}{}", cfg.label, names.len(), shown.join(", "), more));
        }
    }
    let instr = if lines.is_empty() { String::new() } else {
        format!("\n\nMCP SERVERS — connected external tools you may call (names are prefixed \
                 `{TOOL_PREFIX}<server>__<tool>`):\n- {}\nInspect state before mutating; make one \
                 focused change, then read it back.", lines.join("\n- "))
    };
    (schemas, instr)
}

/// Is this tool name an MCP-routed tool?
pub fn is_mcp_tool(name: &str) -> bool { name.starts_with(TOOL_PREFIX) }

/// Route an MCP tool call. Splits `mcp__<key>__<tool>`, finds the running server,
/// calls it. Returns (text, is_error) for the agent loop.
pub fn exec(name: &str, input: &serde_json::Value) -> (String, bool) {
    let rest = &name[TOOL_PREFIX.len()..];
    let Some((key, tool)) = rest.split_once("__") else {
        return (format!("malformed MCP tool name `{name}`"), true);
    };
    let Some(server) = mcp_client::get(key) else {
        return (format!("MCP server `{key}` is not running — enable it in Connections."), true);
    };
    // MCP tool calls can be slow (Premiere ExtendScript round-trips); give them room.
    server.call_tool(tool, input, std::time::Duration::from_secs(120))
}

/// Verify a server via its declared verify_tool (read-only readiness check).
pub fn verify(app: &AppHandle, key: &str) -> Result<String, String> {
    let cfg = get_config(app, key).ok_or_else(|| format!("unknown MCP server `{key}`"))?;
    let tool = cfg.verify_tool.ok_or("this server has no verify tool")?;
    let server = start_server(app, key)?;
    let (text, is_err) = server.call_tool(&tool, &serde_json::json!({}), std::time::Duration::from_secs(30));
    if is_err { Err(text) } else { Ok(text) }
}

/// Which install commands does enabling this server run? Returned to the UI so it
/// can narrate before running (Mason's "user knows what's happening"). Each is
/// (label, program, args). Empty = nothing to install (already a bare command).
pub fn install_plan(cfg: &McpConfig) -> Vec<(String, String, Vec<String>)> {
    match cfg.key.as_str() {
        "premiere" => vec![
            ("Install the Premiere MCP package".into(), "npm".into(),
             vec!["install".into(), "-g".into(), "adobe-premiere-pro-mcp".into(), "--no-audit".into(), "--no-fund".into()]),
            ("Install the Premiere CEP bridge (into Premiere)".into(), "premiere-pro-mcp".into(),
             vec!["--install-cep".into()]),
        ],
        _ => vec![],
    }
}

/// The uninstall commands (mirror of install_plan) so removing an MCP removes
/// what it installed.
pub fn uninstall_plan(cfg: &McpConfig) -> Vec<(String, String, Vec<String>)> {
    match cfg.key.as_str() {
        "premiere" => vec![
            ("Remove the Premiere MCP package".into(), "npm".into(),
             vec!["uninstall".into(), "-g".into(), "adobe-premiere-pro-mcp".into()]),
        ],
        _ => vec![],
    }
}

/// Run an install/uninstall plan through the PROVISIONED node's npm (so no system
/// node needed), narrating each step on `channel`. Non-npm steps (e.g. the CEP
/// installer) run via the just-installed global bin. Returns a per-step report.
pub async fn run_plan(app: &AppHandle, channel: &str, plan: &[(String, String, Vec<String>)]) -> Result<Vec<serde_json::Value>, String> {
    use tauri::Emitter;
    let node = crate::provision::node_bin(app).ok_or("Node not provisioned — enable it first")?;
    let npm = crate::provision::npm_cli(app).ok_or("npm not provisioned")?;
    let node_prefix = node.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf())
        .ok_or("bad node layout")?;
    let path = crate::provision::provisioned_path(app);
    let mut report = Vec::new();
    for (label, program, args) in plan {
        let _ = app.emit(channel, &serde_json::json!({ "phase": "step", "note": label }));
        eprintln!("[aygent][mcp] {label}: {program} {}", args.join(" "));
        let mut cmd = if program == "npm" {
            let mut c = std::process::Command::new(&node);
            c.arg(&npm).args(args);
            c
        } else {
            // A global bin the previous step installed (resolve under node prefix/bin).
            let bin = node_prefix.join("bin").join(program);
            let prog = if bin.is_file() { bin } else { std::path::PathBuf::from(program) };
            let mut c = std::process::Command::new(prog);
            c.args(args);
            c
        };
        cmd.env("PATH", format!("{path}:/usr/bin:/bin"));
        cmd.env("npm_config_prefix", &node_prefix);
        let out = cmd.output().map_err(|e| format!("{label} failed to run: {e}"))?;
        let ok = out.status.success();
        let tail: String = String::from_utf8_lossy(&out.stderr).chars().rev().take(400).collect::<String>().chars().rev().collect();
        report.push(serde_json::json!({ "label": label, "ok": ok, "detail": tail }));
        let _ = app.emit(channel, &serde_json::json!({ "phase": "step-done", "note": label, "ok": ok }));
        if !ok { return Err(format!("{label} failed: …{tail}")); }
    }
    Ok(report)
}

