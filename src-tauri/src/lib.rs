// AYGENT — Tauri shell entry (the privileged side).
// Owns: window, tray, keychain broker, the PATH BROKER (trust boundary),
// and the daemon supervisor that launches the Node daemon (on macOS, UNDER a
// Seatbelt profile that denies file+exec — Atlas C1). Also mints the
// per-session WS token (C6) and hands it + the daemon port to the UI.

mod broker;
mod broker_ws;
mod catalog;
mod checkpoint;
mod conversations;
mod hardware;
mod keychain;
mod local_provider;
mod provider;
mod settings;
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

/// Reveal a file in the OS file manager (Finder on macOS). The path is resolved
/// THROUGH THE BROKER (jailed) first — so this can only ever reveal files that
/// live inside the agent folder. An out-of-scope or traversal path is refused by
/// the same jail the agent obeys; the reveal action gets no special privilege.
/// macOS: `open -R <file>` selects the item in Finder. (Windows/Linux branches
/// kept so my Windows authoring box + future Linux builds behave sanely.)
#[tauri::command]
fn reveal_in_finder(
    broker: tauri::State<Arc<Broker>>,
    path: String,
) -> Result<(), String> {
    // Read-mode resolution is the right check: revealing is a read-ish action,
    // and it proves the file is inside the jail before we hand it to the OS.
    let real = broker
        .resolve("default", &path, broker::Mode::Read)
        .map_err(|e| format!("refused by jail: {e:?}"))?;
    let real_os = real.as_os_str();

    #[cfg(target_os = "macos")]
    let mut cmd = { let mut c = std::process::Command::new("open"); c.arg("-R").arg(real_os); c };
    #[cfg(target_os = "windows")]
    let mut cmd = { let mut c = std::process::Command::new("explorer"); c.arg(format!("/select,{}", real.display())); c };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        // No universal "select" on Linux file managers; open the parent dir.
        let dir = real.parent().unwrap_or(&real);
        let mut c = std::process::Command::new("xdg-open"); c.arg(dir); c
    };

    cmd.spawn().map_err(|e| format!("could not open file manager: {e}"))?;
    Ok(())
}

// --- Conversation persistence (Phase 1) ------------------------------------
// Chat history lives in the APP data dir (not the agent folder) so it never
// pollutes the vault or gets swept into checkpoints. Keyed per agent folder.

use tauri::Manager;

/// Resolve the app data dir (created if missing). All conversation storage hangs
/// off this. Fails clearly if the platform dir can't be determined.
fn app_data(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| format!("app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir app_data: {e}"))?;
    Ok(dir)
}

/// List conversation metadata for the current agent folder, newest-first.
#[tauri::command]
fn conv_list(app: tauri::AppHandle, folder: String) -> Result<Vec<conversations::ConvMeta>, String> {
    conversations::list(&app_data(&app)?, &folder)
}

/// Load one full conversation (msgs + provider history).
#[tauri::command]
fn conv_load(app: tauri::AppHandle, folder: String, id: String) -> Result<conversations::Conversation, String> {
    conversations::load(&app_data(&app)?, &folder, &id)
}

/// Save (create or overwrite) a conversation.
#[tauri::command]
fn conv_save(app: tauri::AppHandle, folder: String, conv: conversations::Conversation) -> Result<(), String> {
    conversations::save(&app_data(&app)?, &folder, conv)
}

/// Delete a conversation.
#[tauri::command]
fn conv_delete(app: tauri::AppHandle, folder: String, id: String) -> Result<(), String> {
    conversations::delete(&app_data(&app)?, &folder, &id)
}

/// Update pin + manual order for a batch of conversations (drag-reorder / pin).
/// `updates` = [{ id, pinned, order }].
#[tauri::command]
fn conv_reorder(
    app: tauri::AppHandle,
    folder: String,
    updates: Vec<serde_json::Value>,
) -> Result<(), String> {
    let parsed: Vec<(String, bool, i64)> = updates
        .into_iter()
        .filter_map(|u| {
            let id = u.get("id")?.as_str()?.to_string();
            let pinned = u.get("pinned").and_then(|p| p.as_bool()).unwrap_or(false);
            let order = u.get("order").and_then(|o| o.as_i64()).unwrap_or(0);
            Some((id, pinned, order))
        })
        .collect();
    conversations::reorder(&app_data(&app)?, &folder, parsed)
}

// --- Checkpoints (Phase 1, Contract C4) ------------------------------------

/// Take a checkpoint of the agent folder. `label` is usually the user prompt.
/// Returns the new checkpoint sha, or null if nothing changed. Root comes from
/// the broker (never a UI-supplied path) so git only ever runs on the jail root.
#[tauri::command]
fn checkpoint_snapshot(
    broker: tauri::State<Arc<Broker>>,
    label: String,
) -> Result<Option<String>, String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::snapshot(&root, &label)
}

/// The full checkpoint timeline + undo/redo availability, newest first.
#[tauri::command]
fn checkpoint_timeline(
    broker: tauri::State<Arc<Broker>>,
) -> Result<checkpoint::Timeline, String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::timeline(&root)
}

/// Rewind (jump) the agent folder to a specific checkpoint. Snapshots current
/// state first, so the jump never loses uncommitted work.
#[tauri::command]
fn checkpoint_rewind(
    broker: tauri::State<Arc<Broker>>,
    target: String,
) -> Result<(), String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::rewind(&root, &target)
}

/// Undo: step the cursor one checkpoint back and restore that state.
#[tauri::command]
fn checkpoint_undo(broker: tauri::State<Arc<Broker>>) -> Result<Option<String>, String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::undo(&root)
}

/// Redo: step the cursor one checkpoint forward and restore that state.
#[tauri::command]
fn checkpoint_redo(broker: tauri::State<Arc<Broker>>) -> Result<Option<String>, String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::redo(&root)
}

/// Read the retention window (days, 1..=90).
#[tauri::command]
fn checkpoint_get_retention(broker: tauri::State<Arc<Broker>>) -> Result<i64, String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::get_retention(&root)
}

/// Set the retention window (days) + prune anything older immediately.
#[tauri::command]
fn checkpoint_set_retention(broker: tauri::State<Arc<Broker>>, days: i64) -> Result<(), String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::set_retention(&root, days)
}

/// Purge ALL checkpoint history for the folder (user's files untouched).
#[tauri::command]
fn checkpoint_purge(broker: tauri::State<Arc<Broker>>) -> Result<(), String> {
    let root = broker.root_for("default").map_err(|e| format!("{e:?}"))?;
    checkpoint::purge_all(&root)
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

/// Per-folder selected model. "" = auto (prefer haiku, else first available).
#[tauri::command]
fn get_selected_model(app: tauri::AppHandle, folder: String) -> Result<String, String> {
    use tauri::Manager;
    let app_data = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?;
    Ok(settings::load(&app_data, &folder).model)
}

#[tauri::command]
fn set_selected_model(app: tauri::AppHandle, folder: String, model: String) -> Result<(), String> {
    use tauri::Manager;
    let app_data = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?;
    let mut s = settings::load(&app_data, &folder);
    s.model = model;
    settings::save(&app_data, &folder, &s)
}

/// Full per-folder selection (provider + model). Empty provider = anthropic.
#[tauri::command]
fn get_selection(app: tauri::AppHandle, folder: String) -> Result<serde_json::Value, String> {
    use tauri::Manager;
    let app_data = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?;
    let s = settings::load(&app_data, &folder);
    Ok(serde_json::json!({ "provider": s.provider, "model": s.model }))
}

#[tauri::command]
fn set_selection(app: tauri::AppHandle, folder: String, provider: String, model: String) -> Result<(), String> {
    use tauri::Manager;
    let app_data = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?;
    let mut s = settings::load(&app_data, &folder);
    s.provider = provider;
    s.model = model;
    settings::save(&app_data, &folder, &s)
}

// --- LOCAL MODELS ----------------------------------------------------------

/// Detect the RUNTIME machine (CPU/RAM/GPU/accel) for perf prediction.
#[tauri::command]
fn detect_hardware() -> hardware::HardwareInfo {
    hardware::detect()
}

/// Live catalog of the latest curated GGUF models (Qwen/Mistral/Kimi/Llama),
/// each entry already scored for THIS machine's expected performance per quant.
#[tauri::command]
async fn local_catalog(per_family: Option<usize>) -> Result<serde_json::Value, String> {
    let hw = hardware::detect();
    let models = catalog::fetch(per_family.unwrap_or(4)).await?;
    // Attach a perf verdict to each quant so the UI can show fit + speed inline.
    let scored: Vec<serde_json::Value> = models.iter().map(|m| {
        let quants: Vec<serde_json::Value> = m.quants.iter().map(|q| {
            let v = hardware::predict(&hw, m.params_billions, q.size_gb);
            serde_json::json!({
                "quant": q.quant, "filename": q.filename, "size_gb": q.size_gb,
                "download_url": q.download_url, "perf": v,
            })
        }).collect();
        serde_json::json!({
            "family": m.family, "family_label": m.family_label, "repo": m.repo,
            "name": m.name, "params_billions": m.params_billions,
            "downloads": m.downloads, "updated": m.updated, "quants": quants,
        })
    }).collect();
    Ok(serde_json::json!({ "hardware": hw, "models": scored }))
}

/// Directory where downloaded GGUF models live (app data, not the user folder).
fn models_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir models: {e}"))?;
    Ok(dir)
}

/// List downloaded local models (filename + size + absolute path).
#[tauri::command]
fn local_downloaded(app: tauri::AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let dir = models_dir(&app)?;
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("gguf") {
                let size_gb = std::fs::metadata(&p).map(|m| (m.len() as f64 / 1_073_741_824.0) as f32).unwrap_or(0.0);
                out.push(serde_json::json!({
                    "filename": p.file_name().and_then(|n| n.to_str()).unwrap_or(""),
                    "path": p.to_string_lossy(), "size_gb": size_gb,
                }));
            }
        }
    }
    Ok(out)
}

/// Download a GGUF to the models dir, emitting progress events on `channel`.
/// Returns the absolute local path (which becomes the per-folder model id).
#[tauri::command]
async fn local_download(app: tauri::AppHandle, channel: String, url: String, filename: String) -> Result<String, String> {
    use tauri::Emitter;
    use futures_util::StreamExt;
    // Guard the filename (no traversal).
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("invalid filename".into());
    }
    let dir = models_dir(&app)?;
    let dest = dir.join(&filename);
    let tmp = dir.join(format!("{filename}.part"));

    let client = reqwest::Client::builder().user_agent("aygent/0.1").build().map_err(|e| format!("http: {e}"))?;
    let resp = client.get(&url).send().await.map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() { return Err(format!("download {}", resp.status())); }
    let total = resp.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create file: {e}"))?;
    let mut got: u64 = 0;
    let mut last_emit = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("stream: {e}"))?;
        use std::io::Write;
        file.write_all(&bytes).map_err(|e| format!("write: {e}"))?;
        got += bytes.len() as u64;
        // throttle progress events to ~every 8MB
        if got - last_emit > 8_000_000 {
            last_emit = got;
            let _ = app.emit(&channel, &serde_json::json!({ "got": got, "total": total }));
        }
    }
    drop(file);
    std::fs::rename(&tmp, &dest).map_err(|e| format!("finalize: {e}"))?;
    let _ = app.emit(&channel, &serde_json::json!({ "got": got, "total": total, "done": true }));
    Ok(dest.to_string_lossy().to_string())
}

/// Delete a downloaded local model by filename.
#[tauri::command]
fn local_delete(app: tauri::AppHandle, filename: String) -> Result<(), String> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("invalid filename".into());
    }
    let path = models_dir(&app)?.join(&filename);
    if path.exists() { std::fs::remove_file(&path).map_err(|e| format!("delete: {e}"))?; }
    Ok(())
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

    // M0.2b: capability model. In Folder Mode (the M0.3 default) the granted
    // caps are {fs.read, fs.write, net.http, mcp.net}. The file tools below need
    // only fs.read/fs.write, so they're allowed. shell.exec / mcp.local-exec /
    // hooks.script are NOT granted here — they belong to Pro Mode. The OS
    // Seatbelt jail is the authoritative backstop; this is the explicit early gate.
    // (Full registry-driven gating + MCP transport split lands with the MCP
    // client in Phase 1; the enum + rule are frozen now — see
    // daemon/src/core/capabilities.ts and docs/CONTRACTS.md §3.)
    let _mode = "folder"; // M1.4 makes this per-agent.

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

// --- STREAMING agent loop (Phase 1) ----------------------------------------
// Emits normalized StreamEvents to the UI via Tauri events as they arrive.
// Tool calls still route through the broker (jailed). Falls back to turn-based
// automatically if the provider can't stream (UI shows a thinking animation).

/// Execute one jailed tool call, returning (result_text, is_error).
fn exec_tool(
    broker: &Arc<Broker>,
    name: &str,
    input: &serde_json::Value,
) -> (String, bool) {
    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
    match name {
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
    }
}

fn agent_tools() -> serde_json::Value {
    serde_json::json!([
        { "name": "read_file", "description": "Read a UTF-8 text file inside the agent folder. Path relative to folder root.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } },
        { "name": "write_file", "description": "Write a UTF-8 text file inside the agent folder. Path relative to folder root.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] } },
        { "name": "list_files", "description": "List entries in a directory inside the agent folder. Path relative to root; '.' for root.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } }
    ])
}

const AGENT_SYSTEM: &str = "You are AYGENT, an agent that can ONLY touch files inside the user's \
    chosen folder via your tools. You cannot run shell commands. Use read_file/write_file/list_files \
    for file work. Be concise and friendly.";

// Local models are CHAT-only for now (tool-use is a fast-follow), so their
// system prompt doesn't promise file tools it can't yet use.
const AGENT_SYSTEM_LOCAL: &str = "You are AYGENT, a helpful local AI assistant running privately \
    on the user's own machine. You are running in chat mode. Be concise and friendly.";

/// STREAMING chat turn. `history` is the running conversation (array of
/// {role, content}); we append the new user prompt, run the agent loop with
/// streaming, emit events to the UI, and return the FULL updated history so the
/// UI can persist multi-turn memory. Events are emitted on channel `agent://<id>`.
#[tauri::command]
async fn agent_stream(
    app: tauri::AppHandle,
    broker: tauri::State<'_, Arc<Broker>>,
    channel: String,
    prompt: String,
    history: serde_json::Value,
    model: Option<String>,
    provider: Option<String>,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    let broker = broker.inner().clone();
    let provider_kind = provider.unwrap_or_default();

    // ---- LOCAL MODEL PATH (llama.cpp compiled in) --------------------------
    // Chat-first (Mason's call): a downloaded GGUF runs entirely in-process. No
    // key, no network. `model` here is the absolute path to the .gguf file.
    // Tool-use is a deliberate fast-follow, so the local loop is a SINGLE chat
    // turn (no agent_tools, no broker file loop yet).
    if provider_kind == "local" {
        let path = model.filter(|m| !m.trim().is_empty())
            .ok_or_else(|| "no local model selected — download one in Settings".to_string())?;
        let mut messages = if history.is_array() { history } else { serde_json::json!([]) };
        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": prompt }));
        let _ = app.emit(&channel, &provider::StreamEvent::Info {
            text: "local model · chat mode (file tools coming soon)".into(),
        });
        let (content, _stop) = local_provider::local_stream_turn(
            &path, AGENT_SYSTEM_LOCAL, &messages,
            |ev| { let _ = app.emit(&channel, &ev); },
        ).await?;
        messages.as_array_mut().unwrap().push(serde_json::json!({
            "role": "assistant", "content": content
        }));
        return Ok(messages);
    }

    // ---- ANTHROPIC PATH (default) ------------------------------------------
    let key = keychain::get_key("anthropic")
        .map_err(|_| "no anthropic key set — add one in Settings".to_string())?;
    // Model choice: explicit (from the per-folder Settings picker) wins; empty/
    // missing = auto (prefer haiku — cheap/fast — else first available).
    let model = match model.filter(|m| !m.trim().is_empty()) {
        Some(m) => m,
        None => {
            let models = provider::anthropic_list_models(&key).await?;
            models.iter().find(|m| m.contains("haiku")).cloned()
                .or_else(|| models.first().cloned())
                .ok_or_else(|| "account returned no usable models".to_string())?
        }
    };

    // CHECKPOINT (C4) part 1: ensure a BASELINE snapshot exists before the turn
    // runs. This captures the folder's pre-turn state (labeled "baseline") ONLY
    // if there are uncommitted changes / no history yet — so there's always an
    // anchor to rewind *back before* this turn's edits. It is NOT labeled with
    // the prompt: the prompt-labeled checkpoint is taken AFTER the turn (below),
    // so it correctly represents "the state produced by this prompt." This fixes
    // the bug where a turn's writes were absorbed (mislabeled) into the NEXT
    // turn's pre-snapshot, or lost entirely if they were the last edit.
    if let Ok(root) = broker.root_for("default") {
        let _ = checkpoint::snapshot(&root, "baseline");
    }

    let tools = agent_tools();
    let mut messages = if history.is_array() { history } else { serde_json::json!([]) };
    messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": prompt }));

    let emit = |ev: &provider::StreamEvent| { let _ = app.emit(&channel, ev); };
    emit(&provider::StreamEvent::Info { text: format!("model: {model}") });

    for _ in 0..8 {
        let (content, stop) = provider::anthropic_stream_turn(
            &key, &model, AGENT_SYSTEM, &messages, &tools,
            |ev| { let _ = app.emit(&channel, &ev); },
        ).await?;

        messages.as_array_mut().unwrap().push(serde_json::json!({
            "role": "assistant", "content": content.clone()
        }));

        // Execute any tool_use blocks through the broker; emit results live.
        let mut tool_results = Vec::new();
        if let Some(arr) = content.as_array() {
            for blk in arr {
                if blk.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let name = blk.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let id = blk.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let input = blk.get("input").cloned().unwrap_or(serde_json::json!({}));
                    let (result_text, is_err) = exec_tool(&broker, &name, &input);
                    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                    // tell the UI the tool's OUTCOME (the ToolUse start already fired)
                    let _ = app.emit(&channel, &serde_json::json!({
                        "kind": "ToolResult", "name": name, "path": path,
                        "ok": !is_err, "detail": if is_err { result_text.clone() } else { String::new() }
                    }));
                    tool_results.push(serde_json::json!({
                        "type": "tool_result", "tool_use_id": id,
                        "content": result_text, "is_error": is_err
                    }));
                }
            }
        }

        if stop == "tool_use" && !tool_results.is_empty() {
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "user", "content": tool_results
            }));
            continue;
        }
        break;
    }

    // CHECKPOINT (C4) part 2: snapshot the folder AFTER the turn's writes, labeled
    // with THIS turn's prompt. Now every turn that changed files gets its own
    // correctly-labeled checkpoint, and "Rewind here" restores the state produced
    // by that prompt — which is what a user intuitively expects. Skips silently if
    // nothing changed (no empty checkpoints). Best-effort: never blocks the reply.
    if let Ok(root) = broker.root_for("default") {
        match checkpoint::snapshot(&root, &prompt) {
            Ok(Some(sha)) => { let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("checkpoint {sha}") }); }
            Ok(None) => {}
            Err(e) => { let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("checkpoint skipped: {e}") }); }
        }
        // Auto-prune anything past the retention window (best-effort; never blocks).
        if let Ok(days) = checkpoint::get_retention(&root) {
            let _ = checkpoint::prune(&root, days);
        }
    }

    emit(&provider::StreamEvent::Done { stop_reason: "end_turn".into() });
    Ok(messages) // full history back for multi-turn persistence
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
            set_provider_key, has_provider_key, anthropic_test, anthropic_models, agent_run,
            agent_stream, reveal_in_finder, get_selected_model, set_selected_model,
            get_selection, set_selection, detect_hardware, local_catalog, local_downloaded,
            local_download, local_delete,
            checkpoint_snapshot, checkpoint_timeline, checkpoint_rewind,
            checkpoint_undo, checkpoint_redo,
            checkpoint_get_retention, checkpoint_set_retention, checkpoint_purge,
            conv_list, conv_load, conv_save, conv_delete, conv_reorder
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
