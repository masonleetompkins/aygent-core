// AYGENT — Tauri shell entry (the privileged side).
// Owns: window, tray, keychain broker, the PATH BROKER (trust boundary),
// and the daemon supervisor that launches the Node daemon (on macOS, UNDER a
// Seatbelt profile that denies file+exec — Atlas C1). Also mints the
// per-session WS token (C6) and hands it + the daemon port to the UI.

mod broker;
mod broker_ws;
mod keychain;
mod provider;
mod supervisor;

use std::sync::Arc;
use rand::Rng;
use supervisor::DaemonState;
use tauri_plugin_dialog::DialogExt;
use broker::Broker;

/// Mint a random per-session WS token (Atlas C6). Injected into the daemon via
/// env and handed to the WebView via the `daemon_info` command — the daemon
/// rejects any WS connection lacking it. localhost alone is NOT access control.
fn mint_ws_token() -> String {
    let mut rng = rand::thread_rng();
    (0..48)
        .map(|_| {
            let c: u8 = rng.gen_range(0..62);
            match c {
                0..=9 => (b'0' + c) as char,
                10..=35 => (b'a' + (c - 10)) as char,
                _ => (b'A' + (c - 36)) as char,
            }
        })
        .collect()
}

/// UI calls this to learn how to reach the daemon: the WS port + auth token.
#[tauri::command]
fn daemon_info(state: tauri::State<Arc<DaemonState>>) -> serde_json::Value {
    let port = *state.ws_port.lock().unwrap();
    serde_json::json!({ "port": port, "token": state.ws_token })
}

/// Open the native folder picker, canonicalize the choice, and register it as
/// the agent's scoped root in the broker (M0.2 (d)). This is how a user chooses
/// their Agent Folder — from here on the broker jails the agent to it.
///
/// ASYNC + non-blocking: `blocking_pick_folder()` on the main thread deadlocks
/// the UI (the window can't repaint while blocking). We use the async callback
/// picker and bridge it back with a oneshot channel on a spawned task.
#[tauri::command]
async fn pick_agent_folder(
    app: tauri::AppHandle,
    broker: tauri::State<'_, Arc<Broker>>,
) -> Result<Option<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |chosen| {
        let _ = tx.send(chosen);
    });
    // Wait for the user's choice off the main thread.
    let chosen = tokio::task::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| e.to_string())?;

    let Some(fp) = chosen else { return Ok(None) };
    let path = fp.into_path().map_err(|e| e.to_string())?;

    // Canonicalize against the data volume (resolves firmlinks/symlinks/case)
    // so the broker's containment checks compare against the real root.
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);

    // Register as the default agent's scope (multi-agent keying lands M1.4).
    broker.set_scope("default", canonical.clone(), false);
    eprintln!("[aygent] agent folder set: {}", canonical.display());
    Ok(Some(canonical.to_string_lossy().to_string()))
}

/// Probe the broker: ask it to resolve a path for the default agent and report
/// admit/refuse. Lets the UI demonstrate the jail live (M0.2 visible proof).
#[tauri::command]
fn broker_probe(
    broker: tauri::State<Arc<Broker>>,
    requested: String,
) -> serde_json::Value {
    match broker.resolve("default", &requested, broker::Mode::Read) {
        Ok(p) => serde_json::json!({ "ok": true, "resolved": p.to_string_lossy() }),
        Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:?}") }),
    }
}

// --- M0.3: provider key (Keychain) + Anthropic end-to-end -------------------

/// Store a provider API key in the macOS Keychain. Key never returns to JS.
#[tauri::command]
fn set_provider_key(provider: String, key: String) -> Result<(), String> {
    keychain::set_key(&provider, &key)
}

/// UI-safe check: does a key exist? Returns bool, never the secret.
#[tauri::command]
fn has_provider_key(provider: String) -> bool {
    keychain::has_key(&provider)
}

/// List the Anthropic models this key can actually use (robust vs guessing IDs;
/// also feeds the Phase-1 Settings model picker).
#[tauri::command]
async fn anthropic_models() -> Result<Vec<String>, String> {
    let key = keychain::get_key("anthropic")
        .map_err(|_| "no anthropic key set — add one first".to_string())?;
    provider::anthropic_list_models(&key).await
}

/// M0.3 end-to-end proof: fetch the Anthropic key from Keychain (Rust-side
/// only), ask the account which models it can use, call the first one, return
/// the text (prefixed with which model answered). Key NEVER enters JS/WebView.
#[tauri::command]
async fn anthropic_test(prompt: String) -> Result<String, String> {
    let key = keychain::get_key("anthropic")
        .map_err(|_| "no anthropic key set — add one first".to_string())?;
    // Ask the account what it can use instead of guessing a model ID.
    let models = provider::anthropic_list_models(&key).await?;
    // Prefer a haiku (cheap/fast) if present, else the first available model.
    let model = models
        .iter()
        .find(|m| m.contains("haiku"))
        .cloned()
        .or_else(|| models.first().cloned())
        .ok_or_else(|| "account returned no usable models".to_string())?;
    let answer = provider::anthropic_complete(&key, &model, &prompt).await?;
    Ok(format!("[{model}]\n{answer}"))
}

/// M0.3 AGENT LOOP: the model gets a jailed file tool (read_file/write_file) and
/// may call it; every call routes through the broker (jailed). This is where the
/// brain (model) + the jail (broker) fuse into an actual agent. Returns a
/// transcript string of what happened (tool calls + final answer).
#[tauri::command]
async fn agent_run(
    broker: tauri::State<'_, Arc<Broker>>,
    prompt: String,
) -> Result<String, String> {
    let key = keychain::get_key("anthropic")
        .map_err(|_| "no anthropic key set — add one first".to_string())?;
    let models = provider::anthropic_list_models(&key).await?;
    let model = models
        .iter()
        .find(|m| m.contains("haiku"))
        .cloned()
        .or_else(|| models.first().cloned())
        .ok_or_else(|| "account returned no usable models".to_string())?;

    // Tool schemas the model can call. Handlers route through the broker (jailed).
    let tools = serde_json::json!([
        {
            "name": "read_file",
            "description": "Read a UTF-8 text file inside the agent folder. Path is relative to the folder root.",
            "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        },
        {
            "name": "write_file",
            "description": "Write a UTF-8 text file inside the agent folder. Path is relative to the folder root.",
            "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] }
        },
        {
            "name": "list_files",
            "description": "List entries in a directory inside the agent folder. Path is relative to the folder root; use '.' for the root.",
            "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }
        }
    ]);

    let system = "You are AYGENT, an agent that can ONLY touch files inside the user's chosen \
        folder via your tools. You cannot run shell commands. Use read_file/write_file/list_files \
        to do file work. Be concise.";

    let mut messages = serde_json::json!([{ "role": "user", "content": prompt }]);
    let mut transcript = String::new();
    transcript.push_str(&format!("[{model}]\n"));

    // Agent loop: cap iterations so a misbehaving model can't spin forever.
    for _ in 0..8 {
        let resp = provider::anthropic_turn(&key, &model, system, &messages, &tools).await?;
        let content = resp.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
        let stop = resp.get("stop_reason").and_then(|s| s.as_str()).unwrap_or("");

        // Append the assistant turn to history verbatim (needed for tool_result).
        messages.as_array_mut().unwrap().push(serde_json::json!({
            "role": "assistant", "content": content.clone()
        }));

        // Collect any text + any tool_use blocks.
        let mut tool_results = Vec::new();
        for blk in &content {
            match blk.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = blk.get("text").and_then(|t| t.as_str()) {
                        transcript.push_str(t);
                        transcript.push('\n');
                    }
                }
                Some("tool_use") => {
                    let name = blk.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let id = blk.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let input = blk.get("input").cloned().unwrap_or(serde_json::json!({}));
                    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("");

                    // EXECUTE THROUGH THE BROKER (jailed).
                    let (result_text, is_err) = match name {
                        "read_file" => match broker.resolve_and_open("default", path, broker::Mode::Read) {
                            Ok(mut f) => {
                                use std::io::Read;
                                let mut s = String::new();
                                match f.read_to_string(&mut s) {
                                    Ok(_) => (s, false),
                                    Err(e) => (format!("io error: {e}"), true),
                                }
                            }
                            Err(e) => (format!("refused by jail: {e:?}"), true),
                        },
                        "write_file" => {
                            let cnt = input.get("content").and_then(|c| c.as_str()).unwrap_or("");
                            if let Ok(real) = broker.resolve("default", path, broker::Mode::Write) {
                                if let Some(parent) = real.parent() { let _ = std::fs::create_dir_all(parent); }
                            }
                            match broker.resolve_and_open("default", path, broker::Mode::Write) {
                                Ok(mut f) => {
                                    use std::io::Write as _;
                                    match f.write_all(cnt.as_bytes()) {
                                        Ok(_) => (format!("wrote {} bytes to {path}", cnt.len()), false),
                                        Err(e) => (format!("io error: {e}"), true),
                                    }
                                }
                                Err(e) => (format!("refused by jail: {e:?}"), true),
                            }
                        }
                        "list_files" => match broker.resolve("default", path, broker::Mode::Read) {
                            Ok(real) => match std::fs::read_dir(&real) {
                                Ok(rd) => {
                                    let names: Vec<String> = rd.filter_map(|e| e.ok())
                                        .map(|e| e.file_name().to_string_lossy().to_string()).collect();
                                    (names.join("\n"), false)
                                }
                                Err(e) => (format!("io error: {e}"), true),
                            },
                            Err(e) => (format!("refused by jail: {e:?}"), true),
                        },
                        other => (format!("unknown tool: {other}"), true),
                    };

                    transcript.push_str(&format!("  ⚙ {name}({path}) → {}\n",
                        if is_err { format!("✗ {result_text}") } else { "✓".to_string() }));

                    tool_results.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": result_text,
                        "is_error": is_err
                    }));
                }
                _ => {}
            }
        }

        if stop == "tool_use" && !tool_results.is_empty() {
            // Feed results back and loop for the model's next turn.
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "user", "content": tool_results
            }));
            continue;
        }
        break; // final answer reached
    }

    Ok(transcript)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(DaemonState {
        ws_token: mint_ws_token(),
        ..Default::default()
    });
    let broker = broker::Broker::new();
    // Separate per-session token for the broker WS (jail-boundary channel).
    let broker_token = mint_ws_token();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(broker.clone())
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            daemon_info, pick_agent_folder, broker_probe,
            set_provider_key, has_provider_key, anthropic_test, anthropic_models, agent_run
        ])
        .setup(move |_app| {
            let broker = broker.clone();
            let state = state.clone();
            let broker_token = broker_token.clone();
            // Start the Rust-hosted broker WS server (M0.2b), then spawn the
            // daemon, handing it the broker-WS {port, token} so it can connect
            // as an authed client. jailed=false in dev; Seatbelt (jailed=true)
            // is finalized later in M0.2.
            tauri::async_runtime::spawn(async move {
                match broker_ws::start(broker, broker_token.clone()).await {
                    Ok(broker_port) => {
                        // M0.2(e): jail ON by default on macOS (deny file+exec
                        // Seatbelt). Override with AYGENT_JAILED=0 for dev if a
                        // Seatbelt issue needs isolating.
                        let jailed = std::env::var("AYGENT_JAILED").as_deref() != Ok("0");
                        if let Err(e) = supervisor::spawn_daemon(
                            state.clone(), jailed, broker_port, &broker_token,
                        ) {
                            eprintln!("[aygent] daemon spawn failed: {e}");
                        }
                    }
                    Err(e) => eprintln!("[aygent] broker-ws failed to start: {e}"),
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AYGENT");
}
