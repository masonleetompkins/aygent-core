// AYGENT — Tauri shell entry (the privileged side).
// Owns: window, tray, keychain broker, the PATH BROKER (trust boundary),
// and the daemon supervisor that launches the Node daemon (on macOS, UNDER a
// Seatbelt profile that denies file+exec — Atlas C1). Also mints the
// per-session WS token (C6) and hands it + the daemon port to the UI.

use rusqlite::params; // scheduler_list/scheduler_runs read-only queries

mod agents;
mod browser;
mod cancel;
// ENGINE-CEF (Phase 1): native Chromium visible-surface + punchout geometry.
// Both are #![cfg(all(target_os = "macos", feature = "engine-cef"))] internally,
// so declaring them unconditionally is inert unless the feature is on + macOS.
#[cfg(all(target_os = "macos", feature = "engine-cef"))]
mod cef_engine;
#[cfg(all(target_os = "macos", feature = "engine-cef"))]
mod cef_geometry;
#[cfg(all(target_os = "macos", feature = "engine-cef"))]
pub mod cef_app_mac;
mod broker;
mod dock_icon;
mod broker_ws;
mod exec;      // PRO MODE: the process-spawn broker (shell.exec). Only Rust spawns.
mod paths;     // CONFIG RELOCATION: root-folder pointer + state-dir seam + onboarding paths.
mod history;
mod introspect; // Agent self-introspection: the read-only `whoami` tool.
mod catalog;
mod connections;
mod connectors; // CONNECTOR REGISTRY: a provider is data (descriptor), not code.
mod connector_exec; // One generic HTTP executor for every registry connector.
pub mod savepoint; // pub for examples/savepoint_diag
mod context_docs;
mod dashboard_data; // DASHBOARDS M3: pull-only data resolution (bindings/http/exec).
mod dashboard; // DASHBOARDS: prompt-built, spec-driven, pull-only (never auto-runs a model).
mod db;
mod drainer;
mod lanes;
mod mailbox;
mod memory;
mod scheduler;
mod vault_write;
mod web;
mod whisper;
mod migrate_json;
pub mod remote_rt; // AYGENT REMOTE: Supabase Realtime client (Phoenix framing over wss).
pub mod remote_bridge; // AYGENT REMOTE: turn bridge — protocol, coalescing, dedupe, event translation.
pub mod remote_runtime; // AYGENT REMOTE: device runtime — rt events → dispatch → engine → sealed replies.
pub mod remote_cmds; // AYGENT REMOTE: Settings-card commands (pair/unpair/status/connect).
pub mod remote;   // AYGENT REMOTE: pairing + E2E envelope + Realtime client (masonlee.build/remote).
mod repo;
mod writer;
mod gguf;
mod google_auth; // GOOGLE service accounts: RS256 JWT -> access token (the one credential we must MINT, not paste).
mod hardware;
mod keychain;
mod local_provider;
mod local_tools;
mod openai_provider;
mod meta_provider; // Muse (Meta): api.meta.ai/v1 /responses (OpenAI Responses API shape) — own module.
mod mlx; // MLX local runner (Apple Silicon): mlx-community/* via uv-managed mlx_lm.server sidecar.
mod pdf_tool;
mod provider;
mod pricing; // CLOUD model context windows + $/Mtok (context meter + cost).
mod provision;
mod mcp_client;
mod mcp; // MCP manager: registry + catalog + agent-loop bridge + install/uninstall. // MCP client: spawn stdio JSON-RPC servers, discover + route their tools. // Level A: bundle portable node+ffmpeg+hyperframes into app-data (no system installs).
mod supervisor;
mod telegram;
mod tools_registry;
mod spark_state; // SPARKS: jailed KV persistence (Sparks/<slug>/state.json) for interactive Sparks.
mod video; // VIDEO v0.3: project store + hardlink import + probe/thumbs (Video/<project>/).
mod video_render; // VIDEO v0.3: composition.json -> ffmpeg filter graph -> MP4/frame.
mod video_media; // VIDEO v0.3: aygent-media:// jailed range-capable media serving for the editor.
mod video_tools; // VIDEO v0.3: video_* agent tools (frame-accurate edit helpers).
mod video_hyperframes; // VIDEO: Hyperframes transparent overlays — graphics + captions (T1/V3 clips).
mod continue_gate; // task_continue TIMER PICKER (Mason 09-08): human picks 1/3/5/10/15 min via chat modal.

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

// ---------------------------------------------------------------------------
// ONBOARDING / CONFIG RELOCATION (2026-07-31). The AYGENT root folder is the
// home; app-support holds only root.json. These commands drive the first-launch
// wizard: is onboarding needed? → pick/detect a folder → init or restore → create
// the first agent with a home subfolder.
// ---------------------------------------------------------------------------

/// Does the app need onboarding? True when no valid root is configured (missing
/// pointer OR the pointed-at folder is gone). The UI shows the wizard when true.
#[tauri::command]
fn onboarding_status(app: tauri::AppHandle) -> serde_json::Value {
    match paths::configured_root(&app) {
        Some(root) => serde_json::json!({
            "needsOnboarding": false,
            "root": root.to_string_lossy(),
        }),
        None => serde_json::json!({ "needsOnboarding": true, "root": serde_json::Value::Null }),
    }
}

/// Native folder picker for onboarding's "choose your AYGENT root" step. Returns
/// the chosen path + whether it's ALREADY an AYGENT root (has the manifest), so
/// the wizard can offer RESTORE vs fresh init. Also flags non-empty folders.
#[tauri::command]
async fn onboarding_pick_root(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |chosen| { let _ = tx.send(chosen); });
    let chosen = tokio::task::spawn_blocking(move || rx.recv().ok().flatten())
        .await.map_err(|e| e.to_string())?;
    let Some(fp) = chosen else { return Ok(serde_json::json!({ "cancelled": true })) };
    let path = fp.into_path().map_err(|e| e.to_string())?;
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let existing = paths::is_aygent_root(&canonical);
    let non_empty = std::fs::read_dir(&canonical)
        .map(|mut rd| rd.next().is_some())
        .unwrap_or(false);
    let (agent_count, chat_count) = if existing {
        count_restore_preview(&canonical).unwrap_or((0,0))
    } else { (0,0) };
    Ok(serde_json::json!({
        "cancelled": false,
        "path": canonical.to_string_lossy(),
        "existingRoot": existing,
        "nonEmpty": non_empty,
        "agentCount": agent_count,
        "chatCount": chat_count,
    }))
}

fn count_restore_preview(root: &std::path::Path) -> Result<(i64,i64), String> {
    let db_path = root.join(".aygent").join("aygent.db");
    if !db_path.is_file() { return Ok((0,0)); }
    let conn = rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open preview db: {e}"))?;
    let agents: i64 = conn.query_row("SELECT COUNT(*) FROM agent WHERE archived=0", [], |r| r.get(0)).unwrap_or(0);
    let chats: i64 = conn.query_row("SELECT COUNT(*) FROM conversation", [], |r| r.get(0)).unwrap_or(0);
    Ok((agents, chats))
}

/// Commit the chosen root: init the folder as an AYGENT root (manifest + .aygent
/// engine bay) if it isn't one already, then write the app-support pointer at it.
/// Idempotent — pointing at an existing root just RESTORES it (adopts its config).
/// After this, a restart boots into the root (SQLite loads from <root>/.aygent).
#[tauri::command]
fn onboarding_set_root(
    app: tauri::AppHandle,
    db: tauri::State<writer::Db>,
    folder: String,
) -> Result<serde_json::Value, String> {
    let root = std::path::PathBuf::from(&folder);
    if !root.is_dir() {
        return Err(format!("folder does not exist: {folder}"));
    }
    let restored = paths::is_aygent_root(&root);
    paths::init_root(&root)?;              // safe if already a root (won't overwrite manifest)
    paths::write_pointer(&app, &root)?;    // flip the pointer
    // RE-POINT THE LIVE DB to <root>/.aygent — no process restart (app.restart()
    // from inside a command future aborts; that was the SIGABRT crash). The
    // writer thread checkpoints the old WAL, opens the new DB, runs its
    // migrations, and the next write (agents_create) lands in the root's DB.
    let state_dir = root.join(".aygent");
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("mkdir state: {e}"))?;
    db.repoint(state_dir)?;
    eprintln!("[aygent] root set + db re-pointed: {} (restored={restored})", root.display());
    Ok(serde_json::json!({ "ok": true, "root": root.to_string_lossy(), "restored": restored }))
}

/// Create an agent's HOME skeleton under the root (<root>/<Name>/ + context/,
/// memory/, .aygent/). Called by onboarding's "first agent" step BEFORE
/// agents_create so the profile can point folder_path at the home. Returns the
/// absolute home path the UI passes as the agent's folder.
#[tauri::command]
fn onboarding_make_agent_home(app: tauri::AppHandle, name: String) -> Result<String, String> {
    let root = paths::configured_root(&app)
        .ok_or("no root configured — set the root folder first")?;
    let home = paths::init_agent_home(&root, &name)?;
    Ok(home.to_string_lossy().to_string())
}

/// Finish onboarding WITHOUT a process restart. The DB was already re-pointed to
/// <root>/.aygent by onboarding_set_root (live swap, no restart), so the agent
/// created in step 3 is in the root's DB. This just returns ok so the UI can
/// flip from the wizard into the app by re-checking onboarding_status.
///
/// (Replaces app_restart, which called app.restart() from inside a command
/// future — that aborts the process (SIGABRT) with live daemon/WS/writer threads.)
#[tauri::command]
fn onboarding_finish() -> Result<(), String> {
    Ok(())
}

/// IMPORT MEMORY (2026-07-31): one-click port of an existing memory bundle into
/// an agent's folder, then auto-ingest. This is the AYGENT-native "restore me /
/// bring my memory" flow — no terminal. The user picks a source folder (e.g. a
/// CleoPort bundle or an existing Obsidian vault); we copy its Memory/, Daily/,
/// and _index/ layers INTO the agent's jailed folder, then run the same
/// memory_ingest read-path so retrieval + graph expansion light up immediately.
///
/// Safety: copy is additive (never deletes the agent's existing notes); we only
/// bring the recognized memory layers, not arbitrary files. The destination is
/// the agent's own folder (already its jail).
#[tauri::command]
async fn import_memory(
    app: tauri::AppHandle,
    db: tauri::State<'_, writer::Db>,
    agent_id: String,
    agent_folder: String,
) -> Result<serde_json::Value, String> {
    // 1. Pick the SOURCE folder (native picker, off the main thread).
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |chosen| { let _ = tx.send(chosen); });
    let chosen = tokio::task::spawn_blocking(move || rx.recv().ok().flatten())
        .await.map_err(|e| e.to_string())?;
    let Some(fp) = chosen else { return Ok(serde_json::json!({ "cancelled": true })) };
    let src = fp.into_path().map_err(|e| e.to_string())?;
    let dst = std::path::PathBuf::from(&agent_folder);
    if !dst.is_dir() {
        return Err(format!("agent folder does not exist: {agent_folder}"));
    }

    // 2. Copy the recognized memory LAYERS (Memory/, Daily/) additively. If the
    //    source IS a layer root (has Memory/ or Daily/), copy those; otherwise
    //    treat the whole picked folder as a Memory/ drop-in.
    let mut copied = 0usize;
    let layers = ["Memory", "Daily"];
    let has_layers = layers.iter().any(|l| src.join(l).is_dir());
    if has_layers {
        for layer in layers {
            let s = src.join(layer);
            if s.is_dir() {
                copied += copy_dir_recursive(&s, &dst.join(layer))?;
            }
        }
    } else {
        // No layer structure — import the whole folder as the Memory layer.
        copied += copy_dir_recursive(&src, &dst.join("Memory"))?;
    }

    // 3. Auto-ingest so retrieval + graph expansion light up now.
    let embed_model = ensure_embed_model(&app).await?;
    let report = memory::ingest_vault(&db, "agent", &agent_id, &dst, &embed_model, "").await?;

    Ok(serde_json::json!({
        "cancelled": false,
        "files_copied": copied,
        "source": src.to_string_lossy(),
        "ingest": report,
    }))
}

/// Recursively copy a directory's .md files into `dst` (created if missing).
/// Additive: never deletes; overwrites same-named files (re-import = refresh).
/// Returns the count of files copied. Skips dotfiles + non-markdown noise.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<usize, String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    let mut n = 0;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') { continue; } // skip dotfiles/.aygent
        if path.is_dir() {
            n += copy_dir_recursive(&path, &dst.join(&name))?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            std::fs::copy(&path, dst.join(&name)).map_err(|e| format!("copy {}: {e}", path.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

/// PRO MODE: read whether shell.exec is enabled for a folder's agent. GUI-only
/// (no config files) — stored per-folder like browser-policy.
#[tauri::command]
fn pro_mode_get(app: tauri::AppHandle, folder: String) -> Result<bool, String> {
    Ok(pro_mode_enabled(&app, &folder))
}

/// PRO MODE: enable/disable shell.exec for a folder's agent. Called by the
/// scary-honest consent screen. Writes <app_data>/pro-mode/<folderkey>.json.
/// This is the UX gate; the Rust exec broker cap-gates authoritatively at the WS.
#[tauri::command]
fn pro_mode_set(app: tauri::AppHandle, folder: String, enabled: bool) -> Result<bool, String> {
    let ad = app_data(&app)?;
    let dir = ad.join("pro-mode");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {e}"))?;
    let path = dir.join(format!("{}.json", folder_key_fnv(&folder)));
    let body = serde_json::json!({ "enabled": enabled });
    std::fs::write(&path, serde_json::to_string_pretty(&body).unwrap_or_default())
        .map_err(|e| format!("write: {e}"))?;
    eprintln!("[aygent] pro-mode {} for folder {folder}", if enabled { "ENABLED" } else { "disabled" });
    Ok(enabled)
}

/// PRO MODE / SELF-HOSTED BUILD: seed macOS git credentials from a connected
/// GitHub PAT so `git push`/`git pull` authenticate WITHOUT the token ever
/// entering the repo OR the scrubbed child env.
///
/// HOW (Atlas §4, the env-scrub-safe path): we configure git's built-in
/// `osxkeychain` credential helper globally, then store the PAT into the LOGIN
/// KEYCHAIN under `https://github.com` via `git credential-osxkeychain store`.
/// From then on, ANY `git push` in ANY checkout (incl. a Pro-Mode-spawned one)
/// asks the osxkeychain helper, which reads the login keychain directly — the
/// token is NOT in .git/config, NOT in a remote URL, and NOT in AYGENT's env
/// whitelist, so the exec broker's child-env scrub can't leak it. This runs on
/// the PRIVILEGED Rust side (the daemon is jailed); the token comes from AYGENT's
/// own keychain connection store, decoded here only to hand to git's helper.
#[tauri::command]
async fn github_git_auth(
    db: tauri::State<'_, writer::Db>,
    agent_id: Option<String>,
) -> Result<String, String> {
    // Pull the PAT + login from the connected GitHub connection (any enabled one
    // for this agent, else the first connected github connection).
    let (token, login) = connections::resolve_github_push_token(&db, agent_id.as_deref())?;

    // 1. Set the credential helper globally to osxkeychain (idempotent).
    let set = std::process::Command::new("git")
        .args(["config", "--global", "credential.helper", "osxkeychain"])
        .output()
        .map_err(|e| format!("git config: {e}"))?;
    if !set.status.success() {
        return Err(format!("git config failed: {}", String::from_utf8_lossy(&set.stderr)));
    }

    // 2. Feed the credential to the osxkeychain helper's `store` on stdin. The
    //    protocol is a blank-line-terminated key=value block.
    use std::io::Write as _;
    let mut child = std::process::Command::new("git")
        .args(["credential-osxkeychain", "store"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn credential helper: {e}"))?;
    {
        let stdin = child.stdin.as_mut().ok_or("no stdin to credential helper")?;
        let block = format!(
            "protocol=https\nhost=github.com\nusername={login}\npassword={token}\n\n"
        );
        stdin.write_all(block.as_bytes()).map_err(|e| format!("write cred: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("cred helper: {e}"))?;
    if !out.status.success() {
        return Err(format!("credential store failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(format!("git push/pull authenticated as @{login} (stored in macOS Keychain)."))
}

/// TELEGRAM per-agent (Fix 7): status + token management + test.
/// Token is stored in the macOS Keychain under service telegram-bot-<agentId>.
#[tauri::command]
fn telegram_status(db: tauri::State<writer::Db>, agent_id: String) -> Result<serde_json::Value, String> {
    let ag = repo::get_agent(&db, &agent_id)?.ok_or("agent not found")?;
    let has_token = keychain::has_key(&telegram::keychain_service(&agent_id));
    Ok(serde_json::json!({
        "enabled": ag.telegram_enabled,
        "bot_username": ag.telegram_bot_username,
        "allowed_chats": ag.telegram_allowed_chats,
        "has_token": has_token,
    }))
}
#[tauri::command]
fn telegram_set_token(_app: tauri::AppHandle, db: tauri::State<writer::Db>, agent_id: String, token: String) -> Result<serde_json::Value, String> {
    let tok = token.trim().to_string();
    if tok.is_empty() {
        // Clear token
        let _ = keychain::set_key(&telegram::keychain_service(&agent_id), "");
        let mut ag = repo::get_agent(&db, &agent_id)?.ok_or("agent not found")?;
        ag.telegram_enabled = false;
        ag.telegram_bot_username = String::new();
        repo::update_agent(&db, ag)?;
        return Ok(serde_json::json!({ "ok": true, "cleared": true }));
    }
    if !telegram::looks_like_token(&tok) { return Err("that does not look like a Telegram Bot token (expected 123456:AA...)".into()); }
    keychain::set_key(&telegram::keychain_service(&agent_id), &tok)?;
    // Don't auto-enable here; the Agent Edit card will validate via getMe, then enable with the real @username.
    // This keeps the single source of truth (agents_update) and avoids spawning a worker with a bad token.
    Ok(serde_json::json!({ "ok": true, "saved": true }))
}
#[tauri::command]
async fn telegram_test_token(token: String) -> Result<serde_json::Value, String> {
    let username = telegram::validate_token(token.trim()).await?;
    Ok(serde_json::json!({ "ok": true, "bot_username": username }))
}

/// PRO MODE: list running/known shell processes (for the UI process panel).
#[tauri::command]
fn shell_procs() -> serde_json::Value {
    match exec::global() {
        Some(xb) => xb.list(),
        None => serde_json::json!({ "ok": true, "procs": [] }),
    }
}

/// PRO MODE: kill a running shell process from the UI panel (the always-visible
/// stop button). Handle comes from shell_procs.
#[tauri::command]
fn shell_kill_proc(handle: String, signal: Option<String>) -> Result<(), String> {
    let xb = exec::global().ok_or("exec broker not initialized")?;
    xb.kill(&handle, signal.as_deref().unwrap_or("TERM")).map_err(|e| e.to_string())?;
    Ok(())
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
    // PERSIST the choice so it survives restarts (folder persistence). We save
    // the canonical PATH — correct for a directly-distributed (non-sandboxed)
    // Mac app, which is how AYGENT ships today. A security-scoped bookmark blob
    // slots into the same record later, only needed once App Sandbox is on.
    if let Err(e) = save_agent_folder(&app, &canonical) {
        eprintln!("[aygent] warn: could not persist agent folder: {e}");
    }
    eprintln!("[aygent] agent folder set: {}", canonical.display());
    Ok(Some(canonical.to_string_lossy().to_string()))
}

/// Where the persisted agent-folder record lives (app data, not the user's
/// folder). Shape: { "path": "...", "bookmark": null }. The `bookmark` field is
/// reserved for the macOS security-scoped bookmark blob we add when we enable
/// App Sandbox; until then the canonical path is sufficient + correct.
fn agent_folder_record_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir app data: {e}"))?;
    Ok(dir.join("agent-folder.json"))
}

fn save_agent_folder(app: &tauri::AppHandle, path: &std::path::Path) -> Result<(), String> {
    let rec = serde_json::json!({ "path": path.to_string_lossy(), "bookmark": serde_json::Value::Null });
    let text = serde_json::to_string_pretty(&rec).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(agent_folder_record_path(app)?, text).map_err(|e| format!("write: {e}"))
}

/// Read the persisted agent folder path (if any). Returns None when unset.
fn load_agent_folder(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let p = agent_folder_record_path(app).ok()?;
    let text = std::fs::read_to_string(&p).ok()?;
    let rec: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = rec.get("path").and_then(|v| v.as_str())?;
    if path.is_empty() { return None; }
    Some(std::path::PathBuf::from(path))
}

/// UI calls this on boot to restore the saved Agent Folder. Re-registers the
/// broker scope (fail-closed if the folder vanished) and returns the path so the
/// UI can show it without a re-pick. Returns None if nothing was saved or the
/// saved folder no longer exists.
#[tauri::command]
fn restore_agent_folder(
    app: tauri::AppHandle,
    broker: tauri::State<'_, Arc<Broker>>,
    db: tauri::State<'_, writer::Db>,
) -> Result<Option<String>, String> {
    let Some(saved) = load_agent_folder(&app) else {
        // Even with no legacy single-folder record, agents created directly may
        // have folders — register their scopes so they're live on boot.
        register_all_agent_scopes(&db, &broker);
        return Ok(None);
    };
    // If the folder is gone (moved/deleted/external drive unplugged), don't
    // register a bogus scope — report None so the UI prompts a fresh pick.
    if !saved.is_dir() {
        eprintln!("[aygent] saved agent folder missing: {}", saved.display());
        return Ok(None);
    }
    let canonical = std::fs::canonicalize(&saved).unwrap_or(saved);
    // Keep "default" as a back-compat scope (legacy single-folder callers).
    broker.set_scope("default", canonical.clone(), false);
    eprintln!("[aygent] agent folder restored: {}", canonical.display());
    // BACK-COMPAT: if this user predates multi-agent (has a saved folder but no
    // agent profiles yet), synthesize "My Agent" pointing at it so the switcher
    // has something to show and existing per-folder state stays reachable.
    if let Ok(ad) = app_data(&app) {
        let _ = agents::ensure_migrated(&ad, Some(&canonical.to_string_lossy()));
    }
    // M1.4: register EVERY agent's folder as its own broker scope, keyed by the
    // real agentId (not just "default"). This is what enables TRUE CONCURRENT
    // runs — a scheduled Work agent can touch its folder while the user chats
    // with Personal, each jailed to its own root simultaneously. The security
    // kernel is unchanged: still path-scoped, still fail-closed per scope.
    register_all_agent_scopes(&db, &broker);
    Ok(Some(canonical.to_string_lossy().to_string()))
}

/// M1.4: register a broker scope for every agent that has a folder, keyed by its
/// real agentId. Called on boot (and after agent create/update) so all agents'
/// jails are live simultaneously — the foundation for concurrent runs. A missing
/// or vanished folder is skipped (fail-closed: no scope = the broker refuses).
fn register_all_agent_scopes(db: &writer::Db, broker: &Arc<Broker>) {
    let agents = match repo::list_agents(db) { Ok(a) => a, Err(_) => return };
    for a in &agents {
        if a.archived || a.folder_path.is_empty() { continue; }
        let path = std::path::PathBuf::from(&a.folder_path);
        if !path.is_dir() { continue; }
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        broker.set_scope(&a.id, canonical, false);
    }
    // Scopes must exist BEFORE mounts (set_mounts is a no-op without a scope).
    register_all_agent_mounts(db, broker);
}

/// SHARED CONTEXT: register every agent's READ-ONLY mounts. Run after scopes,
/// and again whenever mounts change. A mount whose folder vanished is skipped
/// (fail-closed — a missing mount reads as "no shared context", never as a
/// widened jail). When a mount names a SOURCE AGENT, that agent's CURRENT
/// folder wins over the stored path, so moving an agent's folder doesn't leave
/// stale mounts pointing at the old location (the 08-03 ghost-folder lesson:
/// never let two code paths disagree about where an agent lives).
fn register_all_agent_mounts(db: &writer::Db, broker: &Arc<Broker>) {
    let mounts = match repo::all_mounts(db) { Ok(m) => m, Err(_) => return };
    if mounts.is_empty() {
        // Still clear stale mounts from a previous registration.
        if let Ok(agents) = repo::list_agents(db) {
            for a in agents { broker.set_mounts(&a.id, vec![]); }
        }
        return;
    }
    let agents = repo::list_agents(db).unwrap_or_default();
    let folder_of = |id: &str| -> Option<String> {
        agents.iter().find(|a| a.id == id && !a.archived)
            .map(|a| a.folder_path.clone())
            .filter(|f| !f.is_empty())
    };

    let mut by_agent: std::collections::HashMap<String, Vec<broker::Mount>> =
        std::collections::HashMap::new();
    for m in mounts {
        // Prefer the source agent's live folder over the stored path.
        let raw = m.source_agent_id.as_deref()
            .and_then(folder_of)
            .unwrap_or_else(|| m.path.clone());
        let path = std::path::PathBuf::from(&raw);
        if !path.is_dir() { continue; }
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        by_agent.entry(m.agent_id.clone()).or_default().push(broker::Mount {
            root: canonical,
            label: if m.label.is_empty() { raw } else { m.label.clone() },
        });
    }
    // Apply to every agent (including those with zero mounts, to clear stale).
    for a in &agents {
        broker.set_mounts(&a.id, by_agent.remove(&a.id).unwrap_or_default());
    }
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
    db: tauri::State<writer::Db>,
    path: String,
) -> Result<(), String> {
    // Read-mode resolution is the right check: revealing is a read-ish action,
    // and it proves the file is inside the jail before we hand it to the OS.
    // Resolve against the ACTIVE agent's scope first (the "default" legacy scope
    // can point at a stale folder if the agent folder moved — Mason 08-03: file
    // tools and shell diverged for exactly this reason), fall back to "default".
    let scope: String = repo::get_active_agent(&db).ok().flatten()
        .map(|a| a.id).unwrap_or_else(|| "default".into());
    let real = broker
        .resolve(&scope, &path, broker::Mode::Read)
        .or_else(|_| broker.resolve("default", &path, broker::Mode::Read))
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
// pollutes the vault or gets swept into SAVE POINTs. Keyed per agent folder.

use tauri::Manager;

/// CACHE-BUST (Mason 08-08): WKWebView caches the frontend bundle on disk under
/// the OS cache dir; on macOS it can serve STALE JS across app updates, so a new
/// build appears to change nothing (this is what defeated the Sparks fixes). We
/// stamp the app version into <cache>/frontend-version.txt and, whenever the
/// running app's version differs from the stamp, delete the WebKit cache subtree
/// ONCE and rewrite the stamp. Result: every new build loads fresh frontend code
/// with zero manual steps. Best-effort + safe: only AYGENT's own WebKit cache is
/// removed (never user files); any error is logged and ignored (a stale cache is
/// a cosmetic nuisance, never a reason to fail boot).
/// Read the content-hashed frontend bundle id from the embedded index.html so we
/// can detect when the UI actually changed between builds (Vite hashes the asset
/// filenames). Returns something like "index-Zcg7olRg.js"; None if unreadable.
fn frontend_build_id(app: &tauri::AppHandle) -> Option<String> {
    use tauri::Manager;
    let res = app.path().resource_dir().ok()?;
    // Tauri bundles frontendDist under the resource dir; index.html references
    // the hashed asset. Try common layouts.
    for candidate in ["index.html", "dist/index.html", "../ui/dist/index.html"] {
        let p = res.join(candidate);
        if let Ok(html) = std::fs::read_to_string(&p) {
            // Grab the first hashed asset name (index-XXXX.js or .css).
            if let Some(start) = html.find("index-") {
                let tail = &html[start..];
                let end = tail.find(|c: char| c == '"' || c == '\'' || c == '?').unwrap_or(tail.len());
                let id = &tail[..end];
                if id.len() > 6 { return Some(id.to_string()); }
            }
        }
    }
    None
}

fn bust_webview_cache_on_version_change(app: &tauri::AppHandle) {
    use tauri::Manager;
    // Build id = the version + the content-hashed frontend bundle name (which
    // Vite regenerates on every real UI change). Read it from the embedded
    // index.html via the resource dir; fall back to just the version.
    let version = app.package_info().version.to_string();
    // Include the running BINARY's mtime in the build id: a rebuild at the SAME
    // version with the same asset hash (e.g. a CSP/config-only change) must
    // still bust the WKWebView cache — the injected meta-CSP lives in the
    // cached HTML. This was why the second 1.0.8 build never re-busted.
    let exe_stamp = std::env::current_exe().ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default();
    let build_id = format!("{}:{}:{}", version, frontend_build_id(app).unwrap_or_default(), exe_stamp);
    let Ok(cache_dir) = app.path().app_cache_dir() else {
        eprintln!("[aygent][cache] no app cache dir — skipping cache-bust");
        return;
    };
    let _ = std::fs::create_dir_all(&cache_dir);
    let stamp = cache_dir.join("frontend-version.txt");
    let prev = std::fs::read_to_string(&stamp).unwrap_or_default();
    if prev.trim() == build_id {
        return; // frontend unchanged since last launch — nothing to do
    }
    eprintln!("[aygent][cache] frontend changed ({} -> {}), clearing WebKit cache", if prev.trim().is_empty() { "none" } else { prev.trim() }, build_id);
    // WKWebView's on-disk cache lives in a `WebKit` subdir of the app cache dir.
    let webkit = cache_dir.join("WebKit");
    if webkit.is_dir() {
        match std::fs::remove_dir_all(&webkit) {
            Ok(_) => eprintln!("[aygent][cache] cleared {}", webkit.display()),
            Err(e) => eprintln!("[aygent][cache] could not clear WebKit cache: {e}"),
        }
    }
    // Also clear a generic Cache.db / Code Cache if present (belt + suspenders).
    for name in ["Cache.db", "Cache.db-shm", "Cache.db-wal", "Code Cache", "GPUCache"] {
        let p = cache_dir.join(name);
        if p.is_dir() { let _ = std::fs::remove_dir_all(&p); }
        else if p.is_file() { let _ = std::fs::remove_file(&p); }
    }
    if let Err(e) = std::fs::write(&stamp, &build_id) {
        eprintln!("[aygent][cache] could not write version stamp: {e}");
    }
}

/// Resolve the STATE DIR (created if missing) — where SQLite + JSON stores live.
/// CONFIG RELOCATION (2026-07-31): this now routes through paths::state_dir,
/// which returns <root>/.aygent when a root folder is configured (via the
/// pointer file), else falls back to the OS app-data dir (pre-onboarding). Every
/// existing `app_data(&app)` call site relocates automatically — this helper is
/// the single chokepoint. Name kept as `app_data` to avoid churning ~26 sites.
fn app_data(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    paths::state_dir(app)
}

/// Resolve the UI's `folder` arg to an agent_id. The frontend still keys chat
/// by folder path (its contract is unchanged); the SQLite spine keys by agent.
/// We map folder→agent via the agent that owns that folder_path, falling back
/// to the active agent. This keeps the UI stable while state moves to SQLite.
fn agent_for_folder(db: &writer::Db, folder: &str) -> Result<String, String> {
    if !folder.is_empty() {
        // Canonicalize both sides before comparing (Atlas #5 cause 1): a folder
        // stored with a trailing slash / symlink / case difference would fail a
        // raw string equality and the conversation would appear "missing".
        let want = std::fs::canonicalize(folder).unwrap_or_else(|_| std::path::PathBuf::from(folder));
        for a in repo::list_agents(db)? {
            if a.folder_path == folder { return Ok(a.id); }
            let have = std::fs::canonicalize(&a.folder_path).unwrap_or_else(|_| std::path::PathBuf::from(&a.folder_path));
            if have == want { return Ok(a.id); }
        }
    }
    // Fall back to the active agent (single-folder users, or a not-yet-mapped
    // folder). Empty string is tolerated downstream (no conversations found).
    repo::active_id(db)
}

/// List conversation metadata for the current agent folder, newest-first.
/// M1.1: resolves folder→agent, then reads from SQLite (WAL concurrent read).
#[tauri::command]
fn conv_list(db: tauri::State<writer::Db>, folder: String) -> Result<Vec<repo::ConvMeta>, String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    repo::list_conversations(&db, &agent_id)
}

// ---- M1.7 Slice 1: vault memory read path (ingest + retrieve) ----------
// Embeddings run IN-PROCESS through the compiled-in llama.cpp engine (NO Ollama,
// no external process, install nothing — Mason's hard stipulation). The embedder
// is a small GGUF that AYGENT auto-downloads to its own models dir on first use,
// exactly like the chat catalog. `MEM_EMBED_FILE` is the local filename;
// `MEM_EMBED_URL` is the public HF source (Q4_K_M, ~85MB).
const MEM_EMBED_FILE: &str = "nomic-embed-text-v1.5.Q4_K_M.gguf";
const MEM_EMBED_URL: &str =
    "https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q4_K_M.gguf?download=true";

/// Resolve the local embedding GGUF path, downloading it on first use. This is
/// what keeps "install nothing" true: the model lives in AYGENT's own app-data
/// models dir, fetched by AYGENT itself, never by the user in a terminal.
async fn ensure_embed_model(app: &tauri::AppHandle) -> Result<String, String> {
    let dir = models_dir(app)?;
    let dest = dir.join(MEM_EMBED_FILE);
    if dest.is_file() {
        return Ok(dest.to_string_lossy().to_string());
    }
    // Download to a .part then rename (same pattern as local_download). Public
    // GGUF repo = no auth token needed.
    let tmp = dir.join(format!("{MEM_EMBED_FILE}.part"));
    let client = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client.get(MEM_EMBED_URL).header("Accept", "*/*").send().await
        .map_err(|e| format!("embed model download request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("embed model download failed: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("embed model download body: {e}"))?;
    std::fs::write(&tmp, &bytes).map_err(|e| format!("write embed model: {e}"))?;
    std::fs::rename(&tmp, &dest).map_err(|e| format!("finalize embed model: {e}"))?;
    Ok(dest.to_string_lossy().to_string())
}

/// Ingest a vault folder into the derived memory index for an agent (isolated).
/// owner = ('agent', agent_id). Returns a report the test UI/CLI can print.
#[tauri::command]
async fn memory_ingest(
    app: tauri::AppHandle,
    db: tauri::State<'_, writer::Db>,
    agent_id: String,
    vault_path: String,
) -> Result<memory::IngestReport, String> {
    let root = std::path::PathBuf::from(&vault_path);
    if !root.is_dir() {
        return Err(format!("vault path is not a folder: {vault_path}"));
    }
    let embed_model = ensure_embed_model(&app).await?;
    memory::ingest_vault(&db, "agent", &agent_id, &root, &embed_model, "").await
}

/// Retrieve memory for a query: semantic top-K + graph expansion. Read-only.
#[tauri::command]
async fn memory_retrieve(
    app: tauri::AppHandle,
    db: tauri::State<'_, writer::Db>,
    agent_id: String,
    query: String,
    top_k: Option<usize>,
    expand_hops: Option<usize>,
) -> Result<Vec<memory::RetrievedNote>, String> {
    let embed_model = ensure_embed_model(&app).await?;
    memory::retrieve(
        &db, "agent", &agent_id, &query,
        top_k.unwrap_or(4), expand_hops.unwrap_or(1),
        &embed_model, "",
    ).await
}

// ---- M1.7 Slice 2: L1 EPISODIC WRITE (append to daily note) --------------
// The FIRST sanctioned mutation of a real vault. Guarded by the byte-stability
// gate (vault_write.rs tests) + the prefix invariant + the jail broker. We
// resolve the daily-note path THROUGH THE BROKER in WRITE mode so an append can
// only ever land inside the agent's folder scope — same jail the agent obeys.
#[tauri::command]
fn memory_append_daily(
    broker: tauri::State<Arc<Broker>>,
    agent_id: String,
    date: String,      // "YYYY-MM-DD"
    entry: String,     // the episodic line(s) to append (no leading dash needed)
) -> Result<vault_write::AppendReceipt, String> {
    // Daily notes live under /Daily by convention (Atlas L1). Resolve the
    // RELATIVE path through the broker for this agent's scope.
    let rel = format!("Daily/{}", vault_write::daily_note_name_from_str(&date)?);
    let abs = broker
        .resolve(&agent_id, &rel, broker::Mode::Write)
        .map_err(|e| format!("path refused by jail: {e:?}"))?;
    let header = vault_write::daily_header(&date);
    let block = if entry.trim_start().starts_with('-') {
        entry
    } else {
        format!("- {entry}")
    };
    vault_write::append_to_note(&abs, &header, &block)
}

/// Report whether the byte-stability gate is compiled/available. The REAL proof
/// is `cargo test` (the gate tests) — this is a lightweight UI affordance that
/// runs the identity + append checks in-process on the bundled corpus so the
/// panel can show a green/red without a terminal.
#[tauri::command]
fn memory_gate_check() -> Result<serde_json::Value, String> {
    vault_write::run_gate_in_process()
}

// ---- M1.7 Slice 3: explicit L2 write ("remember this") + novelty dedup ----
// Create a durable atomic Memory note, OR reinforce a near-duplicate. The
// Memory/ dir is resolved THROUGH THE JAIL BROKER in Write mode so an atom can
// only ever land inside the agent's folder scope.
#[tauri::command]
async fn memory_remember(
    app: tauri::AppHandle,
    broker: tauri::State<'_, Arc<Broker>>,
    db: tauri::State<'_, writer::Db>,
    agent_id: String,
    text: String,
    ntype: Option<String>,   // decision | preference | fact | person | project | note
    source: Option<String>,  // optional provenance block-ref, e.g. "[[2026-07-28^s3]]"
) -> Result<memory::RememberResult, String> {
    // Resolve Memory/ through the jail (Write mode). We resolve a sentinel path
    // inside it, then take its parent as the dir — the broker validates the
    // whole path is in-scope.
    let abs_sentinel = broker
        .resolve(&agent_id, "Memory/.aygent-scope", broker::Mode::Write)
        .map_err(|e| format!("Memory path refused by jail: {e:?}"))?;
    let abs_memory_dir = abs_sentinel
        .parent()
        .ok_or("could not resolve Memory dir")?
        .to_path_buf();
    let embed_model = ensure_embed_model(&app).await?;
    memory::remember(
        &db, "agent", &agent_id,
        &abs_memory_dir, "Memory",
        &text,
        ntype.as_deref().unwrap_or("note"),
        source.as_deref().unwrap_or(""),
        &embed_model, "",
    ).await
}

// ---- M1.7 Slice 4: AUTO-CAPTURE + salience (Self-Gardening killer loop) ----
// Extract durable facts from a turn WITHOUT being told to, salience-gate them,
// and route each through the novelty-deduped remember(). Conservative by
// default (bias to under-remember). Same jail-broker Memory/ resolution.
#[tauri::command]
async fn memory_auto_capture(
    app: tauri::AppHandle,
    broker: tauri::State<'_, Arc<Broker>>,
    db: tauri::State<'_, writer::Db>,
    agent_id: String,
    user_text: String,
    threshold: Option<f32>,  // salience cutoff; default 0.65 (conservative)
) -> Result<memory::AutoCaptureReport, String> {
    let abs_sentinel = broker
        .resolve(&agent_id, "Memory/.aygent-scope", broker::Mode::Write)
        .map_err(|e| format!("Memory path refused by jail: {e:?}"))?;
    let abs_memory_dir = abs_sentinel
        .parent()
        .ok_or("could not resolve Memory dir")?
        .to_path_buf();
    let embed_model = ensure_embed_model(&app).await?;
    memory::auto_capture(
        &db, "agent", &agent_id,
        &abs_memory_dir, "Memory",
        &user_text,
        threshold.unwrap_or(0.65),
        "",
        &embed_model, "",
    ).await
}

// ---- M1.7 auto-capture gate (per-agent toggle) ---------------------------
/// Whether this agent auto-remembers durable facts from conversation. Default
/// ON (capture is salience+novelty gated = conservative). Thin wrapper so the
/// turn loop reads it cheaply.
fn memory_auto_remember_enabled(db: &writer::Db, agent_id: &str) -> bool {
    repo::get_auto_remember(db, agent_id)
}

/// UI: read the per-agent auto-remember toggle.
#[tauri::command]
fn memory_get_auto_remember(db: tauri::State<writer::Db>, agent_id: String) -> Result<bool, String> {
    Ok(repo::get_auto_remember(&db, &agent_id))
}

/// UI: set the per-agent auto-remember toggle.
#[tauri::command]
fn memory_set_auto_remember(db: tauri::State<writer::Db>, agent_id: String, enabled: bool) -> Result<(), String> {
    repo::set_auto_remember(&db, &agent_id, enabled)
}

// ---- M1.9 Connections: GitHub (Slice 1) ----------------------------------
// A Connection = keychain-backed bearer credential + non-secret metadata,
// exposed to an agent as Rust-side token-attached tools (connections.rs).

/// Connect GitHub via a fine-grained/classic PAT. Validates GET /user, stores
/// the token in the keychain, upserts the connection row. Returns the login.
#[tauri::command]
async fn github_connect(db: tauri::State<'_, writer::Db>, token: String) -> Result<serde_json::Value, String> {
    let (id, login) = connections::connect_github_pat(&db, &token).await?;
    Ok(serde_json::json!({ "id": id, "login": login }))
}

/// List all connections (non-secret metadata) for the Connections catalog.
#[tauri::command]
fn connections_list(db: tauri::State<writer::Db>) -> Result<Vec<connections::ConnectionRow>, String> {
    connections::list(&db)
}

/// Disconnect (delete the row + wipe keychain slots).
#[tauri::command]
fn connection_disconnect(db: tauri::State<writer::Db>, id: i64) -> Result<(), String> {
    connections::disconnect(&db, id)
}

/// Enable/disable a connection for a specific agent (per-agent toggle).
#[tauri::command]
fn connection_set_agent_enabled(db: tauri::State<writer::Db>, agent_id: String, connection_id: i64, enabled: bool) -> Result<(), String> {
    connections::set_agent_enabled(&db, &agent_id, connection_id, enabled)
}

/// Which connection ids are enabled for an agent (per-agent UI state).
#[tauri::command]
fn connection_enabled_for_agent(db: tauri::State<writer::Db>, agent_id: String) -> Result<Vec<i64>, String> {
    connections::enabled_ids_for_agent(&db, &agent_id)
}

/// The connector CATALOG (all descriptors, non-secret by construction). Drives
/// the Connections screen: cards, setup steps, auth fields, per-tool access.
#[tauri::command]
fn connectors_catalog() -> Vec<&'static connectors::Connector> {
    connectors::catalog().iter().collect()
}

/// Connect ANY registry connector. `values` maps auth-field key -> value.
/// Validates against the live API (so a bad credential fails at paste time, not
/// mid-turn an hour later) and captures which ACCOUNT it belongs to.
#[tauri::command]
async fn connector_connect(
    db: tauri::State<'_, writer::Db>,
    provider: String,
    nickname: String,
    values: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let map = values.as_object().cloned().unwrap_or_default();
    let (id, account) = connections::connect_connector(&db, &provider, &nickname, &map).await?;
    Ok(serde_json::json!({ "id": id, "account": account }))
}

/// Per-agent connection state for the UI: which are enabled and in what mode.
#[tauri::command]
fn connection_agent_state(
    db: tauri::State<writer::Db>,
    agent_id: String,
) -> Result<serde_json::Value, String> {
    let enabled = connections::enabled_ids_for_agent(&db, &agent_id)?;
    let modes: Vec<serde_json::Value> = connections::enabled_providers_for_agent(&db, &agent_id)
        .into_iter()
        .map(|(p, m)| serde_json::json!({ "provider": p, "access_mode": m }))
        .collect();
    Ok(serde_json::json!({ "enabled_ids": enabled, "providers": modes }))
}

/// Switch ONE connector tool on or off for an (agent, connection). This is the
/// primary control surface: connecting an account grants full capability, and the
/// user removes individual tools from here.
#[tauri::command]
fn connection_set_tool_enabled(
    db: tauri::State<writer::Db>,
    agent_id: String,
    connection_id: i64,
    tool_name: String,
    on: bool,
) -> Result<(), String> {
    connections::set_tool_enabled(&db, &agent_id, connection_id, &tool_name, on)
}

/// Every tool a connected provider COULD offer, with its current on/off state —
/// the switch list. Must include OFF tools, or there is no way to switch one back
/// on.
#[tauri::command]
fn connection_tool_states(
    db: tauri::State<writer::Db>,
    agent_id: String,
) -> Result<serde_json::Value, String> {
    let off: std::collections::HashSet<(i64, String)> =
        connections::disabled_tools_by_connection(&db, &agent_id).into_iter().collect();
    let mut out = Vec::new();
    for row in connections::list(&db)? {
        let Some(def) = connectors::by_id(&row.provider) else { continue };
        let tools: Vec<serde_json::Value> = def.all_tools().map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "access": if t.access == connectors::Access::Write { "Write" } else { "Read" },
                "danger": t.danger,
                "enabled": !off.contains(&(row.id, t.name.to_string())),
            })
        }).collect();
        out.push(serde_json::json!({
            "connection_id": row.id,
            "provider": row.provider,
            "tools": tools,
        }));
    }
    Ok(serde_json::json!(out))
}

/// Bulk switch: turn every WRITE tool of a connection off (read-only) or back on.
/// This replaces the old access_mode toggle — expressed in the same per-tool
/// storage everything else reads, so it cannot disagree with the switch panel.
#[tauri::command]
fn connection_set_read_only(
    db: tauri::State<writer::Db>,
    agent_id: String,
    connection_id: i64,
    read_only: bool,
) -> Result<(), String> {
    let provider: String = {
        let conn = db.reader()?;
        conn.query_row(
            "SELECT provider FROM connection WHERE id = ?1",
            rusqlite::params![connection_id],
            |r| r.get(0),
        ).map_err(|e| format!("unknown connection: {e}"))?
    };
    let def = connectors::by_id(&provider)
        .ok_or_else(|| format!("unknown connector `{provider}`"))?;
    for t in def.all_tools() {
        if t.access != connectors::Access::Write { continue; }
        connections::set_tool_enabled(&db, &agent_id, connection_id, t.name, !read_only)?;
    }
    Ok(())
}

/// Turn WRITE access on/off for one (agent, connection). Read is the default and
/// write tools are not even added to the agent's tool list until this is on.
#[tauri::command]
fn connection_set_write(
    db: tauri::State<writer::Db>,
    agent_id: String,
    connection_id: i64,
    write: bool,
) -> Result<(), String> {
    connections::set_access_mode(&db, &agent_id, connection_id, write)
}

// ---- M1.8 Scheduler: CRUD (Slice 2) --------------------------------------
// Real schedules come from the UI now (the Slice-1 auto-seed is retired). Two
// kinds the user builds: a HEARTBEAT (interval + "keep conversation context")
// and a SCHEDULED JOB (daily/weekly at a wall-clock time, fresh context).

/// Create a schedule. `when` = {type:"interval",every_secs} | {type:"daily_at",
/// hh,mm} | {type:"weekly_at",days:[0..6],hh,mm}. `action` = {type:"agent_turn",
/// prompt,context:"fresh"|"continue"} | {type:"system_job",job}. tz is an IANA
/// name (e.g. "America/Los_Angeles") or "local".
#[tauri::command]
fn scheduler_create(
    db: tauri::State<writer::Db>,
    sched: tauri::State<scheduler::SchedSignal>,
    agent_id: String,
    name: String,
    when: serde_json::Value,
    action: serde_json::Value,
    tz: Option<String>,
    max_fires_per_day: Option<i64>,
    max_cost_units_per_day: Option<i64>,
) -> Result<i64, String> {
    // Parse the UI's JSON into the typed enums (tolerant of the UI's field names).
    let spec: scheduler::ScheduleSpec = serde_json::from_value(when)
        .map_err(|e| format!("bad `when` spec: {e}"))?;
    let act: scheduler::ScheduleAction = serde_json::from_value(action)
        .map_err(|e| format!("bad `action`: {e}"))?;
    // kind label for the row (observability/UI grouping): heartbeat if an
    // interval-continue turn, cron if daily/weekly, else interval.
    let kind = match (&spec, &act) {
        (scheduler::ScheduleSpec::Interval { .. }, scheduler::ScheduleAction::AgentTurn { context, .. }) if context == "continue" => "heartbeat",
        (scheduler::ScheduleSpec::Interval { .. }, _) => "interval",
        _ => "cron",
    };
    // Sensible default fire cap BY KIND if the UI didn't set one: a daily/weekly
    // job fires ~once, so 2/day is plenty; an interval/heartbeat may fire many
    // times, so cap by frequency (fires that fit in a day, ceiling 240). 2/day
    // for an every-1-min schedule was the confusing 'skipped' Mason hit.
    let default_cap: i64 = match &spec {
        scheduler::ScheduleSpec::Interval { every_secs } => {
            let per_day = 86_400 / (*every_secs).max(1) as i64;
            per_day.clamp(2, 240)
        }
        _ => 2,
    };
    let id = scheduler::create(
        &db, &agent_id, &name, kind, &spec,
        tz.as_deref().unwrap_or("local"), &act,
        max_fires_per_day.unwrap_or(default_cap),
        max_cost_units_per_day,
    )?;
    sched.nudge(); // hot: ticker recomputes wake_at immediately
    Ok(id)
}

/// Reset a schedule's daily fire/cost counters NOW (so testing isn't blocked by
/// the cap). Also re-enables it if a cost ceiling auto-paused it.
#[tauri::command]
fn scheduler_reset_counters(
    db: tauri::State<writer::Db>,
    sched: tauri::State<scheduler::SchedSignal>,
    id: i64,
) -> Result<(), String> {
    scheduler::reset_counters(&db, id)?;
    sched.nudge();
    Ok(())
}

/// Enable/disable one schedule (pause a single one). Hot.
#[tauri::command]
fn scheduler_set_enabled(
    db: tauri::State<writer::Db>,
    sched: tauri::State<scheduler::SchedSignal>,
    id: i64,
    enabled: bool,
) -> Result<(), String> {
    scheduler::set_enabled(&db, id, enabled)?;
    sched.nudge();
    Ok(())
}

/// Delete a schedule (cascades its run log). Hot.
#[tauri::command]
fn scheduler_delete(
    db: tauri::State<writer::Db>,
    sched: tauri::State<scheduler::SchedSignal>,
    id: i64,
) -> Result<(), String> {
    scheduler::delete(&db, id)?;
    sched.nudge();
    Ok(())
}

/// Global pause-all kill switch (Slice 4). Durable + hot. `paused=true` stops
/// ALL scheduled fires instantly (survives relaunch — schedules never silently
/// resume). Returns the new state.
#[tauri::command]
fn scheduler_set_paused(
    db: tauri::State<writer::Db>,
    sched: tauri::State<scheduler::SchedSignal>,
    paused: bool,
) -> Result<bool, String> {
    scheduler::set_paused(&db, paused)?;
    sched.nudge();
    Ok(paused)
}

/// Read the global pause state (for the UI toggle).
#[tauri::command]
fn scheduler_get_paused(db: tauri::State<writer::Db>) -> Result<bool, String> {
    Ok(scheduler::get_paused(&db))
}

/// FIRE NOW: run one schedule immediately (bypasses timing, keeps guardrails).
/// A real UI action AND the fastest way to prove firing works without waiting.
/// Async because a SystemJob (distill) runs in-process with local embeddings.
#[tauri::command]
async fn scheduler_run_now(
    app: tauri::AppHandle,
    db: tauri::State<'_, writer::Db>,
    broker: tauri::State<'_, Arc<Broker>>,
    drain: tauri::State<'_, drainer::DrainSignal>,
    id: i64,
) -> Result<bool, String> {
    scheduler::run_now(&app, &db, &broker, &drain, id).await
}

// ---- M1.8 Scheduler: read-only inspection (Slice 1 observability) --------
/// List schedules (optionally for one agent) with their timing + last-run state,
/// so the UI/panel can show next/last/status without a terminal.
#[tauri::command]
fn scheduler_list(db: tauri::State<writer::Db>, agent_id: Option<String>) -> Result<Vec<serde_json::Value>, String> {
    let conn = db.reader()?;
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT s.id, s.agent_id, s.name, s.kind, s.spec_json, s.action_json, s.enabled,
                s.next_fire_at, s.last_fired_at, s.daily_fire_count,
                (SELECT state FROM schedule_run r WHERE r.schedule_id=s.id ORDER BY r.fired_at DESC LIMIT 1),
                (SELECT result_snippet FROM schedule_run r WHERE r.schedule_id=s.id ORDER BY r.fired_at DESC LIMIT 1)
         FROM schedule s
         WHERE (?1 IS NULL OR s.agent_id = ?1)
         ORDER BY s.next_fire_at ASC",
    ).map_err(|e| format!("prep list: {e}"))?;
    let rows = stmt.query_map(params![agent_id], |r| {
        Ok(serde_json::json!({
            "id": r.get::<_, i64>(0)?,
            "agent_id": r.get::<_, String>(1)?,
            "name": r.get::<_, String>(2)?,
            "kind": r.get::<_, String>(3)?,
            "spec_json": r.get::<_, String>(4)?,
            "action_json": r.get::<_, String>(5)?,
            "enabled": r.get::<_, i64>(6)? != 0,
            "next_fire_at": r.get::<_, Option<i64>>(7)?,
            "last_fired_at": r.get::<_, Option<i64>>(8)?,
            "daily_fire_count": r.get::<_, i64>(9)?,
            "last_status": r.get::<_, Option<String>>(10)?,
            "last_result": r.get::<_, Option<String>>(11)?,
        }))
    }).map_err(|e| format!("query list: {e}"))?;
    for row in rows.flatten() { out.push(row); }
    Ok(out)
}

/// Recent run history for a schedule (observability drill-in).
#[tauri::command]
fn scheduler_runs(db: tauri::State<writer::Db>, schedule_id: i64, limit: Option<i64>) -> Result<Vec<serde_json::Value>, String> {
    let conn = db.reader()?;
    let lim = limit.unwrap_or(20).clamp(1, 200);
    let mut stmt = conn.prepare(
        "SELECT id, fired_at, started_at, finished_at, state, skip_reason, result_snippet
         FROM schedule_run WHERE schedule_id = ?1 ORDER BY fired_at DESC LIMIT ?2",
    ).map_err(|e| format!("prep runs: {e}"))?;
    let rows = stmt.query_map(params![schedule_id, lim], |r| {
        Ok(serde_json::json!({
            "id": r.get::<_, i64>(0)?,
            "fired_at": r.get::<_, i64>(1)?,
            "started_at": r.get::<_, Option<i64>>(2)?,
            "finished_at": r.get::<_, Option<i64>>(3)?,
            "state": r.get::<_, String>(4)?,
            "skip_reason": r.get::<_, Option<String>>(5)?,
            "result_snippet": r.get::<_, Option<String>>(6)?,
        }))
    }).map_err(|e| format!("query runs: {e}"))?;
    let mut out = Vec::new();
    for row in rows.flatten() { out.push(row); }
    Ok(out)
}

/// Quick counts (notes/links/vecs) for a scope — sanity read after ingest.
#[tauri::command]
fn memory_stats(db: tauri::State<writer::Db>, agent_id: String) -> Result<serde_json::Value, String> {
    let (notes, links, vecs) = memory::stats(&db, "agent", &agent_id)?;
    Ok(serde_json::json!({ "notes": notes, "links": links, "vecs": vecs }))
}

/// Load one full conversation (msgs + provider history). `folder` is accepted
/// (unchanged UI contract) but unused — a conversation id is globally unique.
#[tauri::command]
fn conv_load(db: tauri::State<writer::Db>, folder: Option<String>, id: String) -> Result<repo::Conversation, String> {
    let _ = folder;
    repo::load_conversation(&db, &id)
}

/// Save (create or overwrite) a conversation. The conversation is attached to
/// the agent that owns `folder` (resolved here) so multi-agent stays correct.
#[tauri::command]
fn conv_save(db: tauri::State<writer::Db>, folder: String, conv: repo::Conversation) -> Result<(), String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    let mut conv = conv;
    if conv.agent_id.is_empty() { conv.agent_id = agent_id; }
    repo::save_conversation(&db, conv)
}

/// Delete a conversation (and forget its execution lane).
#[tauri::command]
fn conv_rename(db: tauri::State<writer::Db>, id: String, title: String) -> Result<(), String> {
    let t = title.trim();
    if t.is_empty() { return Err("title cannot be empty".into()); }
    repo::rename_conversation(&db, &id, t)
}

#[tauri::command]
fn conv_delete(db: tauri::State<writer::Db>, lanes: tauri::State<lanes::Lanes>, id: String) -> Result<(), String> {
    repo::delete_conversation(&db, &id)?;
    lanes.forget(&id);
    Ok(())
}

/// Update pin + manual order for a batch of conversations (drag-reorder / pin).
/// `updates` = [{ id, pinned, order }].
#[tauri::command]
fn conv_reorder(
    db: tauri::State<writer::Db>,
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
    repo::reorder_conversations(&db, parsed)
}

// --- SAVE POINTs (Phase 1, Contract C4) ------------------------------------

/// Take a SAVE POINT of the agent folder. `label` is usually the user prompt.
/// Returns the new SAVE POINT sha, or null if nothing changed. Root comes from
/// the broker (never a UI-supplied path) so git only ever runs on the jail root.
#[tauri::command]
fn savepoint_snapshot(
    broker: tauri::State<Arc<Broker>>,
    db: tauri::State<writer::Db>,
    label: String,
) -> Result<Option<String>, String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::snapshot(&root, &label)
}

/// The full save-point timeline + undo/redo availability, newest first.
#[tauri::command]
fn savepoint_timeline(
    broker: tauri::State<Arc<Broker>>,
    db: tauri::State<writer::Db>,
) -> Result<savepoint::Timeline, String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::timeline(&root)
}

/// Rewind (jump) the agent folder to a specific save point. Snapshots current
/// state first, so the jump never loses uncommitted work.
#[tauri::command]
fn savepoint_rewind(
    broker: tauri::State<Arc<Broker>>,
    db: tauri::State<writer::Db>,
    target: String,
) -> Result<(), String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::rewind(&root, &target)
}

/// Undo: step the cursor one save point back and restore that state.
#[tauri::command]
fn savepoint_undo(broker: tauri::State<Arc<Broker>>, db: tauri::State<writer::Db>) -> Result<Option<String>, String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::undo(&root)
}

/// Redo: step the cursor one save point forward and restore that state.
#[tauri::command]
fn savepoint_redo(broker: tauri::State<Arc<Broker>>, db: tauri::State<writer::Db>) -> Result<Option<String>, String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::redo(&root)
}

/// Resolve the save-point root for the ACTIVE agent's folder. Bug (Mason 07-28):
/// get/set retention both used broker.root_for("default") — the legacy hardcoded
/// scope — so a value set on the real agent folder was never read back and the
/// slider snapped to 30. Resolve the active agent's own root so read == write.
fn active_savepoint_root(broker: &Arc<Broker>, db: &writer::Db) -> Result<std::path::PathBuf, String> {
    if let Ok(Some(a)) = repo::get_active_agent(db) {
        if !a.folder_path.is_empty() {
            if let Ok(root) = broker.root_for(&a.id) { return Ok(root); }
        }
    }
    broker.root_for("default").map_err(|e| format!("{e:?}"))
}

/// Read the retention window (days, 1..=90) for the active agent's folder.
#[tauri::command]
fn savepoint_get_retention(broker: tauri::State<Arc<Broker>>, db: tauri::State<writer::Db>) -> Result<i64, String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::get_retention(&root)
}

/// Set the retention window (days) + prune anything older immediately.
#[tauri::command]
fn savepoint_set_retention(broker: tauri::State<Arc<Broker>>, db: tauri::State<writer::Db>, days: i64) -> Result<(), String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::set_retention(&root, days)
}

/// Purge ALL save-point history for the folder (user's files untouched).
#[tauri::command]
fn savepoint_purge(broker: tauri::State<Arc<Broker>>, db: tauri::State<writer::Db>) -> Result<(), String> {
    let root = active_savepoint_root(&broker, &db)?;
    savepoint::purge_all(&root)
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
    // Surface the RAW keyring error (see keychain::get_key) instead of a
    // hardcoded "no key set" — otherwise a present-but-unreadable Keychain
    // entry looks identical to a genuinely-absent one in the UI.
    let key = keychain::get_key("anthropic")?;
    provider::anthropic_list_models(&key).await
}

/// List models for an OpenAI-compatible provider ("openai" | "openrouter") via
/// its live /models endpoint — no hardcoded list, so new models appear without
/// an app update.
#[tauri::command]
async fn openai_models(provider: String) -> Result<Vec<String>, String> {
    let key = keychain::get_key(&provider)?;
    // Meta (Llama API) has its own module (different response shape); route it there.
    if provider == "meta" { return meta_provider::list_models(&key).await; }
    openai_provider::list_models(&provider, &key).await
}

/// REAL connectivity check (Mason 08-02): openai_models() for OpenRouter hits a
/// PUBLIC endpoint that returns success with no key — the "Test" button lied.
/// This one actually round-trips auth.
#[tauri::command]
async fn provider_verify_key(provider: String) -> Result<(), String> {
    let key = keychain::get_key(&provider)?;
    if key.trim().is_empty() { return Err("stored key is empty".into()); }
    if provider == "anthropic" { return anthropic_models().await.map(|_| ()); }
    if provider == "meta" { return meta_provider::verify_key(&key).await; }
    openai_provider::verify_key(&provider, &key).await
}

/// Per-agent selected model. "" = auto (prefer haiku, else first available).
/// M1.1: folder→agent then read from SQLite agent_settings.
#[tauri::command]
fn get_selected_model(db: tauri::State<writer::Db>, folder: String) -> Result<String, String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    Ok(repo::load_settings(&db, &agent_id)?.model)
}

#[tauri::command]
fn set_selected_model(db: tauri::State<writer::Db>, folder: String, model: String) -> Result<(), String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    let mut s = repo::load_settings(&db, &agent_id)?;
    s.model = model;
    repo::save_settings(&db, &agent_id, s)
}

/// Full per-agent selection (provider + model). Empty provider = anthropic.
/// BUG FIX (Mason 08-02): this read the VESTIGIAL agent_settings table, but the
/// Agents tab (the only editing surface) writes agent.model/agent.provider on
/// the AGENT row — so switching a model in the UI never changed what Chat used.
/// The agent profile is the single source of truth now; agent_settings remains
/// only for non-selection knobs (auto_remember).
#[tauri::command]
fn get_selection(db: tauri::State<writer::Db>, folder: String) -> Result<serde_json::Value, String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    let a = repo::get_agent(&db, &agent_id)?.ok_or("agent not found")?;
    Ok(serde_json::json!({ "provider": a.provider, "model": a.model, "model_variant": a.model_variant }))
}

#[tauri::command]
fn set_selection(db: tauri::State<writer::Db>, folder: String, provider: String, model: String, model_variant: Option<String>) -> Result<(), String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    let mut a = repo::get_agent(&db, &agent_id)?.ok_or("agent not found")?;
    a.provider = provider;
    a.model = model;
    if let Some(v) = model_variant { a.model_variant = v; }
    repo::update_agent(&db, a)
}

/// CONTEXT METER: the context window (tokens) + $/Mtok price for a model id, so
/// the chat header can render "62% of context used" + a running $ cost. Cloud
/// providers (Anthropic/OpenAI/OpenRouter/Meta); local GGUF models report their
/// window via the catalog, price 0. Pure lookup, no network.
/// Process-global cache for model metadata so the meter is instant and we don't
/// refetch the provider catalog on every render. Keyed by (provider, model);
/// entries live for CTX_TTL. Model specs rarely change within a session, but a
/// TTL keeps us honest if a model is updated while the app is open.
static MODEL_INFO_CACHE: once_cell::sync::Lazy<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, serde_json::Value)>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
const CTX_TTL: std::time::Duration = std::time::Duration::from_secs(1800); // 30 min

#[tauri::command]
async fn chat_model_info(provider: Option<String>, model: String) -> serde_json::Value {
    let provider = provider.unwrap_or_default();
    let cache_key = format!("{provider}::{model}");
    // Serve a fresh cached entry.
    if let Ok(cache) = MODEL_INFO_CACHE.lock() {
        if let Some((at, val)) = cache.get(&cache_key) {
            if at.elapsed() < CTX_TTL { return val.clone(); }
        }
    }

    // Curated pricing table is the fallback denominator + the price source (no
    // provider API exposes price for Anthropic/OpenAI; OpenRouter does).
    let table = pricing::lookup(&model);
    let mut context_tokens = table.context_tokens;
    let mut known = table.known;
    let (mut p_in, mut p_out) = (table.price.input, table.price.output);
    let (mut p_cr, mut p_cw, mut p_cw5, mut p_cw1) = (table.price.cache_read, table.price.cache_write, table.price.cache_write_5m, table.price.cache_write_1h);
    let mut display_name = model.clone();

    // DYNAMIC window (the fix): fetch the REAL context window from the provider
    // instead of a hardcoded substring guess. Falls back to the table on any
    // error (offline, older account, unknown model).
    match provider.as_str() {
        "anthropic" | "" => {
            if let Ok(key) = keychain::get_key("anthropic") {
                if let Ok((ctx, _max_out, name)) = provider::anthropic_model_info(&key, &model).await {
                    if ctx > 0 { context_tokens = ctx; known = true; }
                    if !name.is_empty() { display_name = name; }
                }
            }
        }
        "openrouter" => {
            if let Ok(key) = keychain::get_key("openrouter") {
                if let Ok((ctx, pin, pout)) = openai_provider::openrouter_model_info(&key, &model).await {
                    if ctx > 0 { context_tokens = ctx; known = true; }
                    // OpenRouter publishes real price; use it over the table when present.
                    if pin > 0.0 { p_in = pin; p_cr = pin * 0.1; p_cw = pin * 1.25; p_cw5 = pin * 1.25; p_cw1 = pin * 2.5; }
                    if pout > 0.0 { p_out = pout; }
                }
            }
        }
        "meta" => {
            // Muse: try the dynamic /models window first (if the deployment
            // publishes one), else keep the curated fallback (Spark = 1M).
            if let Ok(key) = keychain::get_key("meta") {
                if let Ok(ctx) = meta_provider::muse_model_info(&key, &model).await {
                    if ctx > 0 { context_tokens = ctx; known = true; }
                }
            }
        }
        // openai / local: no dynamic window endpoint. openai keeps the curated
        // table (documented as the ONLY hardcoded numbers); local reports its
        // real window through the Usage stream, so the UI overrides this anyway.
        _ => {}
    }

    let val = serde_json::json!({
        "context_tokens": context_tokens,
        "known": known,
        "display_name": display_name,
        "price": { "input": p_in, "output": p_out, "cache_read": p_cr, "cache_write": p_cw, "cache_write_5m": p_cw5, "cache_write_1h": p_cw1 }
    });
    if let Ok(mut cache) = MODEL_INFO_CACHE.lock() {
        cache.insert(cache_key, (std::time::Instant::now(), val.clone()));
    }
    val
}

/// COMPACT CONTEXT: shrink a conversation's provider-format history so a long
/// chat can keep going without blowing the model context window. The VISIBLE
/// transcript (msgs) is untouched — only the model-facing `history` is replaced
/// with a compact summary seed, so the agent keeps the gist while the token load
/// resets. The model authors the summary using the agent own provider/model.
/// Returns the new (small) history array; the UI persists it onto the conv.
#[tauri::command]
async fn conv_compact(
    db: tauri::State<'_, writer::Db>,
    id: String,
) -> Result<serde_json::Value, String> {
    let conv = repo::load_conversation(&db, &id)?;
    let history = conv.history.as_array().cloned().unwrap_or_default();
    compact_history(&db, &conv.agent_id, history).await
}

/// VIDEO DOCK COMPACT: the editor's per-project conversation is NOT a repo
/// conversation (it lives in Video/<project>/chat.json), so the UI hands us the
/// provider-format history directly. Same summarizer, same seed shape.
#[tauri::command]
async fn history_compact(
    db: tauri::State<'_, writer::Db>,
    agent_id: String,
    history: Vec<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    compact_history(&db, &agent_id, history).await
}

async fn compact_history(db: &writer::Db, agent_id: &str, history: Vec<serde_json::Value>) -> Result<serde_json::Value, String> {
    if history.len() < 4 {
        return Err("not enough conversation to compact yet".into());
    }
    // Resolve the agent own provider/model (fallback: anthropic auto/haiku).
    let agent = repo::get_agent(db, agent_id)?.ok_or("agent not found")?;
    let provider = if agent.provider.is_empty() { "anthropic".to_string() } else { agent.provider.clone() };

    // Flatten the history into a readable transcript for the summarizer. We keep
    // it bounded (head + tail) so the summarize call itself never overflows.
    let transcript = flatten_history_for_summary(&history);
    let ask = format!(
        "Summarize this conversation so it can CONTINUE with full context but far \
         fewer tokens. Capture: what the user is doing, decisions made, key facts, \
         open threads, and any state the assistant must remember (file paths, names, \
         numbers). Write it as a dense factual briefing in the SECOND person addressed \
         to the assistant (\"You are helping the user with...\"). Do NOT add pleasantries. \
         Conversation:\n\n{transcript}"
    );

    let summary = match provider.as_str() {
        "anthropic" => {
            let key = keychain::get_key("anthropic").map_err(|_| "no anthropic key set".to_string())?;
            let model = if agent.model.trim().is_empty() {
                let models = provider::anthropic_list_models(&key).await?;
                models.iter().find(|m| m.contains("haiku")).cloned().or_else(|| models.first().cloned()).ok_or("no model")?
            } else { agent.model.clone() };
            provider::anthropic_complete(&key, &model, &ask).await?
        }
        "openai" | "openrouter" | "meta" => {
            let key = keychain::get_key(&provider).map_err(|_| format!("no {provider} key set"))?;
            if agent.model.trim().is_empty() { return Err("pick a model for this agent first".into()); }
            if provider == "meta" { meta_provider::complete(&key, &agent.model, &ask).await? }
            else { openai_provider::complete(&provider, &key, &agent.model, &ask).await? }
        }
        "local" => {
            // LOCAL MODELS COMPACT TOO (Mason 08-19): Gwen authors her own
            // summary in-process — no cloud key needed, which matters MOST here
            // because small context windows are exactly where compaction is
            // needed. `agent.model` is the absolute GGUF path on this provider.
            if agent.model.trim().is_empty() { return Err("no local model selected for this agent".into()); }
            let path = agent.model.clone();
            let ctx_tokens = local_context_budget(&path);
            let msgs = serde_json::json!([{ "role": "user", "content": ask }]);
            let (content, _stop) = local_provider::local_stream_turn(
                &path,
                "You are a precise summarizer. Output only the summary — no preamble.",
                &msgs, ctx_tokens, |_| {},
            ).await?;
            let raw = content.as_array()
                .and_then(|a| a.first())
                .and_then(|b| b.get("text")).and_then(|t| t.as_str())
                .unwrap_or("").to_string();
            // Reasoning models (Qwen3) may wrap musing in <think> blocks — the
            // briefing must be the visible answer only.
            let cleaned = local_tools::strip_think(&raw).trim().to_string();
            if cleaned.is_empty() { return Err("local model produced an empty summary — try again".into()); }
            cleaned
        }
        _ => return Err("compaction needs a cloud provider (Anthropic/OpenAI/OpenRouter) or a local model".into()),
    };

    // The new history is a SINGLE user turn carrying the briefing, so the next
    // real turn appends after it. Small, self-contained, resets the token load.
    let seed = serde_json::json!([
        { "role": "user", "content": format!("[Context summary of the earlier conversation, compacted to save tokens]\n\n{summary}") },
        { "role": "assistant", "content": "Understood — I have the summarized context and we can continue." }
    ]);
    Ok(seed)
}

/// Flatten a provider-format history array into a plain transcript for the
/// summarizer. Bounded (head + tail) so the summarize request never itself
/// overflows the window on a huge chat. Text + tool intent only (skips raw
/// tool bytes and images).
fn flatten_history_for_summary(history: &[serde_json::Value]) -> String {
    fn content_text(v: &serde_json::Value) -> String {
        match v.get("content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(blocks)) => blocks.iter().filter_map(|b| {
                match b.get("type").and_then(|t| t.as_str()) {
                    Some("text") | Some("input_text") | Some("output_text") =>
                        b.get("text").and_then(|t| t.as_str()).map(String::from),
                    Some("tool_use") => b.get("name").and_then(|n| n.as_str()).map(|n| format!("[called tool: {n}]")),
                    Some("tool_result") => Some("[tool result]".to_string()),
                    _ => None,
                }
            }).collect::<Vec<_>>().join(" "),
            _ => String::new(),
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for m in history {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("?");
        let text = content_text(m);
        let text = text.trim();
        if text.is_empty() { continue; }
        let capped: String = text.chars().take(2000).collect();
        lines.push(format!("{}: {}", role, capped));
    }
    // Bound: keep the first 12 + last 40 turns if very long.
    if lines.len() > 60 {
        let head = lines[..12].join("\n");
        let tail = lines[lines.len()-40..].join("\n");
        format!("{head}\n\n[...older turns omitted...]\n\n{tail}")
    } else {
        lines.join("\n")
    }
}

// --- AGENTS (multi-agent profiles) -----------------------------------------

/// List all agent profiles + the active id. UI renders the switcher from this.
/// M1.1: reads from the SQLite spine (single-writer actor for writes, WAL
/// concurrent reads) instead of agents/index.json.
#[tauri::command]
fn agents_list(db: tauri::State<writer::Db>) -> Result<serde_json::Value, String> {
    let agents = repo::list_agents(&db)?;
    let active = repo::active_id(&db)?;
    Ok(serde_json::json!({ "agents": agents, "activeId": active }))
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn agents_create(
    db: tauri::State<writer::Db>,
    broker: tauri::State<'_, Arc<Broker>>,
    name: String,
    icon: Option<String>,
    color: Option<String>,
    folder_path: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    model_variant: Option<String>,
    context_mode: Option<String>,
    system_prompt: Option<String>,
) -> Result<repo::AgentProfile, String> {
    let created = repo::create_agent(
        &db, &name,
        &icon.unwrap_or_default(),
        &color.unwrap_or_default(),
        &folder_path.unwrap_or_default(),
        &model.unwrap_or_default(),
        &provider.unwrap_or_default(),
        &model_variant.unwrap_or_default(),
        &context_mode.unwrap_or_default(),
        &system_prompt.unwrap_or_default(),
    )?;
    // M1.4: register the new agent's scope immediately so it's runnable this
    // session (concurrent with every other agent) — no restart needed.
    register_all_agent_scopes(&db, &broker);
    Ok(created)
}

#[tauri::command]
fn agents_update(app: tauri::AppHandle, db: tauri::State<writer::Db>, broker: tauri::State<'_, Arc<Broker>>, profile: repo::AgentProfile) -> Result<(), String> {
    let enabled = profile.telegram_enabled;
    let agent_id = profile.id.clone();
    repo::update_agent(&db, profile)?;
    // Folder may have changed — re-register all scopes so the jail tracks it.
    register_all_agent_scopes(&db, &broker);
    // If Telegram was just enabled, start polling immediately (boot only starts workers for already-enabled agents).
    if enabled {
        telegram::spawn_one(app, db.inner().clone(), agent_id);
    }
    Ok(())
}

/// AGENT REORDER (Mason 2026-08-06): persist a new display order for the agents
/// (Agents tab drag / the rail). `ids` is the full ordered list, top-first.
#[tauri::command]
fn agents_reorder(db: tauri::State<writer::Db>, ids: Vec<String>) -> Result<(), String> {
    repo::reorder_agents(&db, ids)
}

/// DYNAMIC APP ICON: install a PNG (base64 from the UI's canvas render) as the
/// running app's Dock icon. Called whenever the theme (light/dark + accent)
/// changes, so the icon always matches the app's look.
#[tauri::command]
fn set_app_icon(png_b64: String) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_b64.trim())
        .map_err(|e| format!("bad icon base64: {e}"))?;
    dock_icon::set_dock_icon_png(&bytes)
}

// ── Shared context (read-only mounts) ──────────────────────────────────────

/// List an agent's read-only mounts, annotated with whether the folder is
/// currently reachable (so the UI can show a broken mount honestly).
#[tauri::command]
fn agent_mounts_list(
    db: tauri::State<writer::Db>,
    agent_id: String,
) -> Result<serde_json::Value, String> {
    let mounts = repo::list_mounts(&db, &agent_id)?;
    let agents = repo::list_agents(&db).unwrap_or_default();
    let out: Vec<serde_json::Value> = mounts.into_iter().map(|m| {
        let live = m.source_agent_id.as_deref()
            .and_then(|sid| agents.iter().find(|a| a.id == sid))
            .map(|a| a.folder_path.clone())
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| m.path.clone());
        serde_json::json!({
            "id": m.id,
            "path": live,
            "label": m.label,
            "source_agent_id": m.source_agent_id,
            "ok": std::path::Path::new(&live).is_dir(),
        })
    }).collect();
    Ok(serde_json::json!(out))
}

/// Mount another folder (or another agent's folder) READ-ONLY for this agent.
/// Refuses self-mounts and non-directories. The broker enforces read-only —
/// this only records the intent.
#[tauri::command]
fn agent_mount_add(
    db: tauri::State<writer::Db>,
    broker: tauri::State<Arc<Broker>>,
    agent_id: String,
    path: String,
    label: String,
    source_agent_id: Option<String>,
) -> Result<(), String> {
    if agent_id.is_empty() { return Err("no agent".into()); }
    if source_agent_id.as_deref() == Some(agent_id.as_str()) {
        return Err("an agent can't mount itself".into());
    }
    let p = std::path::Path::new(&path);
    if !p.is_dir() { return Err(format!("not a folder: {path}")); }
    let canonical = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    // Mounting your own root is a no-op; say so instead of silently dropping it.
    if let Ok(own) = broker.root_for(&agent_id) {
        if own == canonical { return Err("that's this agent's own folder".into()); }
    }
    repo::add_mount(&db, &agent_id, &canonical.to_string_lossy(), &label, source_agent_id)?;
    register_all_agent_mounts(&db, &broker);
    Ok(())
}

#[tauri::command]
fn agent_mount_remove(
    db: tauri::State<writer::Db>,
    broker: tauri::State<Arc<Broker>>,
    id: i64,
) -> Result<(), String> {
    repo::remove_mount(&db, id)?;
    register_all_agent_mounts(&db, &broker);
    Ok(())
}

#[tauri::command]
fn agents_delete(db: tauri::State<writer::Db>, lanes: tauri::State<lanes::Lanes>, id: String) -> Result<(), String> {
    // Drop any conversation lanes the UI won't reference again is handled per
    // conversation on delete; agent delete cascades conversations in SQL.
    let _ = &lanes;
    repo::delete_agent(&db, &id)
}

/// Set the active agent AND re-point the broker jail at that agent's folder, so
/// switching agents switches the security scope in one atomic step.
#[tauri::command]
fn agents_set_active(
    db: tauri::State<writer::Db>,
    broker: tauri::State<'_, Arc<Broker>>,
    id: String,
) -> Result<Option<repo::AgentProfile>, String> {
    repo::set_active(&db, &id)?;
    let profile = repo::get_agent(&db, &id)?;
    if let Some(p) = &profile {
        if !p.folder_path.is_empty() {
            let path = std::path::PathBuf::from(&p.folder_path);
            if path.is_dir() {
                let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                broker.set_scope("default", canonical, false);
            }
        }
    }
    Ok(profile)
}

/// Persist the active agent id to app-data so the browser can pick the right
/// per-agent profile (Slice 6). Called by the UI alongside agents_set_active.
#[tauri::command]
fn set_active_agent_marker(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let ad = app_data(&app)?;
    std::fs::write(ad.join("active_agent.txt"), id.trim()).map_err(|e| format!("write active agent: {e}"))
}

#[tauri::command]
fn agents_get_active(db: tauri::State<writer::Db>) -> Result<Option<repo::AgentProfile>, String> {
    repo::get_active_agent(&db)
}

/// M1.4 (#2): the other non-archived agents sharing `folder_path` with `agent_id`
/// (pass "" as agent_id to include all). Agents on the SAME folder share
/// SAVE POINT history + the folder write-lock (CONTRACTS §4). The UI uses this
/// to show a plain "shares history with X, Y" line so the user understands what
/// pointing two agents at one folder means.
#[tauri::command]
fn agents_sharing_folder(
    db: tauri::State<writer::Db>,
    folder_path: String,
    agent_id: Option<String>,
) -> Result<Vec<repo::AgentProfile>, String> {
    repo::agents_sharing_folder(&db, &folder_path, &agent_id.unwrap_or_default())
}

// --- CONTEXT DOCUMENTS (M1.4 #4) -------------------------------------------
// Per-agent reference docs stored app-data-side (out of the jail), chunked into
// mem_chunk for M1.7 retrieval, prepended (capped) into the agent's prompt now.

/// Upload a context doc for an agent. `bytes` is base64 from the UI (file read).
#[tauri::command]
fn agent_context_add(
    app: tauri::AppHandle,
    db: tauri::State<writer::Db>,
    agent_id: String,
    filename: String,
    bytes_b64: String,
) -> Result<context_docs::ContextDoc, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes_b64.as_bytes())
        .map_err(|e| format!("decode upload: {e}"))?;
    let ad = app_data(&app)?;
    context_docs::add(&db, &ad, &agent_id, &filename, &bytes)
}

/// CHAT ATTACHMENTS (Mason 08-01): save an attached file into the agent's
/// jail under .attachments/ and return its relative path. The UI passes these
/// paths to agent_stream, which feeds the CONTENT to the model (images as
/// image blocks — the model can SEE them — text inline, PDFs as documents).
#[tauri::command]
fn chat_attach_file(
    broker: tauri::State<'_, Arc<Broker>>,
    agent_id: String,
    filename: String,
    bytes_b64: String,
) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes_b64.as_bytes())
        .map_err(|e| format!("decode upload: {e}"))?;
    let leaf: String = filename.chars().map(|c| if c == '/' || c == '\\' { '_' } else { c }).collect();
    let rel = format!(".attachments/{}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0), leaf);
    let abs = broker.resolve(&agent_id, &rel, broker::Mode::Write).map_err(|e| format!("jail refused: {e:?}"))?;
    if let Some(dir) = abs.parent() { std::fs::create_dir_all(dir).map_err(|e| format!("mkdir attachments: {e}"))?; }
    std::fs::write(&abs, &bytes).map_err(|e| format!("write attachment: {e}"))?;
    Ok(rel)
}
/// DIFF VIEW (Mason 09-09): snapshot a file's CURRENT content (jailed read)
/// BEFORE the agent overwrites it, so the chat can render a streaming
/// side-by-side diff (old on the left vs the new content arriving live in
/// ToolUseDelta). Capped at 32k chars — `truncated` says so. Missing files
/// (brand-new writes) and unreadable/binary files report `exists: false` /
/// `binary: true` so the UI falls back to the plain code view instead of a
/// bogus all-green diff.
#[tauri::command]
fn tool_file_before(
    broker: tauri::State<'_, Arc<Broker>>,
    agent_id: String,
    path: String,
) -> Result<serde_json::Value, String> {
    use std::io::Read;
    // Same scope-key rule as every other file tool (per-agent scope,
    // "default" only if unregistered — see exec_tool_cfg).
    let agent_id: &str = if broker.root_for(&agent_id).is_ok() { &agent_id } else { "default" };
    match broker.resolve_and_open(agent_id, &path, broker::Mode::Read) {
        Ok(mut f) => {
            let mut s = String::new();
            match f.read_to_string(&mut s) {
                Ok(_) => {
                    let truncated = s.len() > 32_768;
                    if truncated { s.truncate(32_768); }
                    Ok(serde_json::json!({ "exists": true, "content": s, "truncated": truncated, "binary": false }))
                }
                Err(_) => Ok(serde_json::json!({ "exists": true, "content": "", "truncated": false, "binary": true })),
            }
        }
        Err(_) => Ok(serde_json::json!({ "exists": false, "content": "", "truncated": false, "binary": false })),
    }
}

/// STOP BUTTON: the UI calls this when the user clicks Stop mid-turn. `channel`
/// is the SAME per-conversation event channel id the Chat pane already passes
/// to agent_stream (myConvId in turns.ts) -- the cancel registry is keyed by
/// it, so this reaches the exact turn the button belongs to. Returns whether a
/// live turn was actually found and signaled (false = nothing to stop, e.g. it
/// had already finished -- not an error, just a no-op).
#[tauri::command]
fn agent_stop(cancel_reg: tauri::State<'_, cancel::CancelRegistry>, channel: String) -> bool {
    cancel_reg.request_stop(&channel)
}

/// Build Anthropic content BLOCKS for attached files. Images become image
/// blocks (model vision), PDFs document blocks, small text files inline text,
/// anything else a descriptive note (the agent can still use file tools on it).
fn attachment_blocks(broker: &Arc<Broker>, agent_id: &str, rels: &[String]) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    for rel in rels {
        let abs = match broker.resolve(agent_id, rel, broker::Mode::Read) { Ok(a) => a, Err(_) => continue };
        let bytes = match std::fs::read(&abs) { Ok(b) => b, Err(_) => continue };
        let name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
        let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        use base64::Engine;
        let media = match ext.as_str() {
            "png" => Some("image/png"), "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"), "webp" => Some("image/webp"), _ => None,
        };
        if let Some(m) = media {
            if bytes.len() > 4_800_000 {
                blocks.push(serde_json::json!({ "type": "text", "text": format!("[attachment '{name}' is an image over the vision size limit — use file tools on {rel} instead]") }));
            } else {
                blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached image: {name}]") }));
                blocks.push(serde_json::json!({ "type": "image", "source": { "type": "base64", "media_type": m, "data": base64::engine::general_purpose::STANDARD.encode(&bytes) } }));
            }
        } else if ext == "pdf" {
            if bytes.len() > 4_800_000 {
                blocks.push(serde_json::json!({ "type": "text", "text": format!("[attachment '{name}' is a PDF over the inline size limit — stored at {rel}]") }));
            } else {
                blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached PDF: {name}]") }));
                blocks.push(serde_json::json!({ "type": "document", "source": { "type": "base64", "media_type": "application/pdf", "data": base64::engine::general_purpose::STANDARD.encode(&bytes) } }));
            }
        } else if let Ok(text) = String::from_utf8(bytes.clone()) {
            let capped: String = text.chars().take(30_000).collect();
            blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached file: {name} ({rel})]\n\n{capped}") }));
        } else {
            blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached binary file: {name} — stored at {rel} ({} bytes); use your file/shell tools on it as needed]", bytes.len()) }));
        }
    }
    blocks
}

/// Build OpenAI/OpenRouter content PARTS for attached files (Chat Completions
/// vision format: {type:"image_url", image_url:{url:"data:<mime>;base64,..."}}).
/// Mirrors attachment_blocks() but in the wire shape OpenAI-compatible APIs
/// expect — this was MISSING entirely (attachments silently vanished on
/// OpenAI/OpenRouter/DeepSeek: the file saved into the jail fine via
/// chat_attach_file, but the turn never read it back in). PDFs are NOT inlined
/// (chat/completions has no standard inline-PDF content part, unlike Anthropic's
/// document block) — we say so plainly instead of guessing at an unsupported
/// shape; the model still has file tools to read it if needed.
fn attachment_blocks_openai(broker: &Arc<Broker>, agent_id: &str, rels: &[String]) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    for rel in rels {
        let abs = match broker.resolve(agent_id, rel, broker::Mode::Read) { Ok(a) => a, Err(_) => continue };
        let bytes = match std::fs::read(&abs) { Ok(b) => b, Err(_) => continue };
        let name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
        let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        use base64::Engine;
        let media = match ext.as_str() {
            "png" => Some("image/png"), "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"), "webp" => Some("image/webp"), _ => None,
        };
        if let Some(m) = media {
            if bytes.len() > 4_800_000 {
                blocks.push(serde_json::json!({ "type": "text", "text": format!("[attachment '{name}' is an image over the vision size limit — use file tools on {rel} instead]") }));
            } else {
                blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached image: {name}]") }));
                let data_url = format!("data:{m};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
                blocks.push(serde_json::json!({ "type": "image_url", "image_url": { "url": data_url } }));
            }
        } else if ext == "pdf" {
            blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached PDF '{name}' — stored at {rel}; this provider's chat API has no inline-PDF vision format, use read_file/shell tools on it if you need its contents]") }));
        } else if let Ok(text) = String::from_utf8(bytes.clone()) {
            let capped: String = text.chars().take(30_000).collect();
            blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached file: {name} ({rel})]\n\n{capped}") }));
        } else {
            blocks.push(serde_json::json!({ "type": "text", "text": format!("[attached binary file: {name} — stored at {rel} ({} bytes); use your file/shell tools on it as needed]", bytes.len()) }));
        }
    }
    blocks
}

/// VISION (video_look): read rendered canvas frames through the jail so the model
/// SEES them inline. Frames are 854px JPEGs (bounded count + size); stale state is
/// impossible — exec_full drains per call, take_pending_vision drains after the push.
fn vision_bytes(broker: &Arc<Broker>, agent_id: &str, frames: &[video_tools::VisionFrame]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for f in frames.iter().take(4) {
        let Ok(abs) = broker.resolve(agent_id, &f.rel, broker::Mode::Read) else { continue };
        let Ok(bytes) = std::fs::read(&abs) else { continue };
        if bytes.is_empty() || bytes.len() > 4_800_000 { continue; }
        out.push((f.label.clone(), bytes));
    }
    out
}
/// Anthropic content blocks: [text, image, image, ...].
fn vision_blocks_anthropic(vb: &[(String, Vec<u8>)]) -> Vec<serde_json::Value> {
    use base64::Engine;
    let mut blocks = Vec::new();
    for (label, bytes) in vb {
        blocks.push(serde_json::json!({ "type": "text", "text": format!("[canvas frame at {} — the Video tab preview shows the first frame]", label) }));
        blocks.push(serde_json::json!({ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(bytes) } }));
    }
    blocks
}
/// OpenAI/Muse user-message blocks: [text, image_url, ...]. OpenAI passes them
/// through verbatim; the Meta translator already maps image_url -> input_image.
fn vision_blocks_openai(vb: &[(String, Vec<u8>)]) -> Vec<serde_json::Value> {
    use base64::Engine;
    let mut blocks = Vec::new();
    for (label, bytes) in vb {
        blocks.push(serde_json::json!({ "type": "text", "text": format!("[canvas frame at {} — delete spent frames with delete_file]", label) }));
        blocks.push(serde_json::json!({ "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)) } }));
    }
    blocks
}

#[tauri::command]
fn agent_context_list(db: tauri::State<writer::Db>, agent_id: String) -> Result<Vec<context_docs::ContextDoc>, String> {
    context_docs::list(&db, &agent_id)
}

#[tauri::command]
fn agent_context_remove(app: tauri::AppHandle, db: tauri::State<writer::Db>, agent_id: String, id: i64) -> Result<(), String> {
    let ad = app_data(&app)?;
    context_docs::remove(&db, &ad, &agent_id, id)
}

/// M1.4 #7: pending inbound inter-agent message counts per agent (switcher badge).
#[tauri::command]
fn mailbox_pending_counts(db: tauri::State<writer::Db>) -> Result<Vec<(String, i64)>, String> {
    mailbox::pending_counts(&db)
}

/// M1.4 app knobs: inter-agent budget (turns/chain) + max headless concurrency.
#[tauri::command]
fn get_app_knobs(db: tauri::State<writer::Db>) -> Result<repo::AppKnobs, String> {
    repo::get_knobs(&db)
}

#[tauri::command]
fn set_app_knobs(db: tauri::State<writer::Db>, budget: i64, concurrency: i64) -> Result<(), String> {
    repo::set_knobs(&db, budget, concurrency)
}

/// M1.4 #7: the roster of OTHER agents a given agent can message.
#[tauri::command]
fn mailbox_roster(db: tauri::State<writer::Db>, agent_id: String) -> Result<Vec<(String, String)>, String> {
    mailbox::roster(&db, &agent_id)
}

/// M1.4 #7: DELIVERY DRAIN. Pull the next pending inter-agent message for an
/// agent and return it (marks delivered + charges the tree budget). The UI/
/// caller then runs it as a turn on that agent's session via agent_stream. This
/// is the async, non-blocking delivery Atlas specified — the recipient processes
/// on its own lane; user turns always preempt (see Lanes::acquire_low). Returns
/// null when the agent's inbox is empty.
#[tauri::command]
fn mailbox_take_next(db: tauri::State<writer::Db>, agent_id: String) -> Result<Option<mailbox::Message>, String> {
    mailbox::take_next_for(&db, &agent_id)
}

/// M1.4 #5: GENERATE A SOUL.md for an agent — the model authors its own
/// personality/values doc from a short brief, and we save it as the agent's
/// system_prompt (its "soul"). Uses the agent's own provider/model. Returns the
/// generated text so the UI can show + let the user edit before it sticks.
#[tauri::command]
async fn agent_generate_soul(
    db: tauri::State<'_, writer::Db>,
    agent_id: String,
    brief: String,
) -> Result<String, String> {
    let agent = repo::get_agent(&db, &agent_id)?.ok_or("agent not found")?;
    let name = if agent.name.trim().is_empty() { "this agent".to_string() } else { agent.name.clone() };
    let meta = format!(
        "Write a SOUL.md for an AI agent named \"{name}\". {}\n\nThe SOUL.md defines WHO this agent \
         is: its personality, voice, values, and how it should behave. Write it in the SECOND person \
         (\"You are...\") as durable instructions the agent will live by. Be vivid and specific, not \
         generic. Include: a one-line identity, 3-5 core values, tone/voice, and what it cares about. \
         Output ONLY the markdown, no preamble.",
        if brief.trim().is_empty() { "Infer a fitting personality from the name.".to_string() } else { format!("The user's brief: {}", brief.trim()) }
    );

    // Route to the agent's own provider/model (fallback: anthropic auto/haiku).
    let provider = if agent.provider.is_empty() { "anthropic".to_string() } else { agent.provider.clone() };
    let soul = match provider.as_str() {
        "anthropic" => {
            let key = keychain::get_key("anthropic").map_err(|_| "no anthropic key set".to_string())?;
            let model = if agent.model.trim().is_empty() {
                let models = provider::anthropic_list_models(&key).await?;
                models.iter().find(|m| m.contains("sonnet")).cloned()
                    .or_else(|| models.first().cloned())
                    .ok_or("no usable model")?
            } else { agent.model.clone() };
            provider::anthropic_complete(&key, &model, &meta).await?
        }
        "openai" | "openrouter" | "meta" => {
            let key = keychain::get_key(&provider).map_err(|_| format!("no {provider} key set"))?;
            let model = agent.model.clone();
            if model.trim().is_empty() { return Err("pick a model for this agent first".into()); }
            if provider == "meta" { meta_provider::complete(&key, &model, &meta).await? }
            else { openai_provider::complete(&provider, &key, &model, &meta).await? }
        }
        _ => return Err("Soul generation needs a cloud provider (Anthropic/OpenAI/OpenRouter) \
                         — set one for this agent.".into()),
    };
    Ok(soul)
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
    let models = catalog::fetch(per_family.unwrap_or(12)).await?;
    // Attach a perf verdict to each quant so the UI can show fit + speed inline.
    let scored: Vec<serde_json::Value> = models.iter().map(|m| {
        let quants: Vec<serde_json::Value> = m.quants.iter().map(|q| {
            let v = hardware::predict_with_ctx(&hw, m.params_billions, q.size_gb, m.context_tokens);
            serde_json::json!({
                "quant": q.quant, "filename": q.filename, "size_gb": q.size_gb,
                "download_url": q.download_url, "perf": v,
            })
        }).collect();
        serde_json::json!({
            "family": m.family, "family_label": m.family_label, "repo": m.repo,
            "name": m.name, "params_billions": m.params_billions,
            "context_tokens": m.context_tokens,
            "downloads": m.downloads, "updated": m.updated, "quants": quants,
        })
    }).collect();
    Ok(serde_json::json!({ "hardware": hw, "models": scored }))
}

/// Search Hugging Face for any GGUF repo matching a free-text query (power-user path).
/// Unlike the curated catalog, this does not restrict to trusted authors, so a DeepSeek
/// or Gemma GGUF pack can be found. Still filters junk + requires a usable quant.
#[tauri::command]
async fn local_search(query: String, limit: Option<usize>) -> Result<serde_json::Value, String> {
    let hw = hardware::detect();
    let models = catalog::search(query, limit.unwrap_or(12)).await?;
    let scored: Vec<serde_json::Value> = models.iter().map(|m| {
        let quants: Vec<serde_json::Value> = m.quants.iter().map(|q| {
            let v = hardware::predict_with_ctx(&hw, m.params_billions, q.size_gb, m.context_tokens);
            serde_json::json!({
                "quant": q.quant, "filename": q.filename, "size_gb": q.size_gb,
                "download_url": q.download_url, "perf": v,
            })
        }).collect();
        serde_json::json!({
            "family": m.family, "family_label": m.family_label, "repo": m.repo,
            "name": m.name, "params_billions": m.params_billions,
            "context_tokens": m.context_tokens,
            "downloads": m.downloads, "updated": m.updated, "quants": quants,
        })
    }).collect();
    Ok(serde_json::json!({ "hardware": hw, "models": scored }))
}

/// Lookup one exact HF repo by id (e.g. "bartowski/Qwen3-14B-GGUF") and return its
/// catalog entry. Used for the paste-a-repo-ID power-user path.
#[tauri::command]
async fn local_lookup(repo_id: String) -> Result<serde_json::Value, String> {
    let hw = hardware::detect();
    let m = catalog::lookup(repo_id).await?;
    let quants: Vec<serde_json::Value> = m.quants.iter().map(|q| {
        let v = hardware::predict_with_ctx(&hw, m.params_billions, q.size_gb, m.context_tokens);
        serde_json::json!({
            "quant": q.quant, "filename": q.filename, "size_gb": q.size_gb,
            "download_url": q.download_url, "perf": v,
        })
    }).collect();
    Ok(serde_json::json!({
        "family": m.family, "family_label": m.family_label, "repo": m.repo,
        "name": m.name, "params_billions": m.params_billions,
        "context_tokens": m.context_tokens,
        "downloads": m.downloads, "updated": m.updated, "quants": quants,
    }))
}

/// Choose the context window (tokens) for a local model: the model's real
/// advertised window, capped to a memory-safe budget for THIS machine. The KV
/// cache scales ~linearly with context, so we cap by usable accelerator memory.
/// Heuristic: budget ~ half of usable memory for context, at a rough
/// ~0.5 MB/token for a small model's KV cache. Clamped to sane bounds. This is
/// intentionally conservative so it "just works" without OOMing a user's Mac.
fn local_context_budget(gguf_path: &str) -> u32 {
    let fname = std::path::Path::new(gguf_path)
        .file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let lower = fname.to_lowercase();
    let model_ctx = catalog::context_window(&lower);
    let hw = hardware::detect();
    let params_b = catalog::parse_params(&lower);
    if let Ok(meta) = std::fs::metadata(gguf_path) {
        let file_gb = meta.len() as f64 / 1_073_741_824.0;
        if file_gb > 0.5 {
            return hardware::recommended_context(&hw, params_b, file_gb as f32, model_ctx);
        }
    }
    let mem_tokens = ((hw.accel_mem_gb as f64) * 0.40 / 0.0005) as u32;
    let chosen = match (model_ctx, mem_tokens) {
        (0, 0) => 4096,
        (0, m) => m.min(8192),
        (c, 0) => c.min(8192),
        (c, m) => c.min(m),
    };
    chosen.clamp(2048, 131072)
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

    // HF serves GGUF files via a 302 redirect to a CDN (cdn-lfs/Cloudflare).
    // Follow redirects explicitly and send an Accept header so the CDN handoff
    // doesn't 401. (Public GGUF repos need no auth token.)
    let client = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client
        .get(&url)
        .header("Accept", "*/*")
        .send()
        .await
        .map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {} (the model file may have moved — try Refresh catalog)", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);

    // Emit an immediate "starting" event (0 of total) so the UI shows a live bar
    // right away instead of a frozen 0% during the first chunk.
    let _ = app.emit(&channel, &serde_json::json!({ "got": 0u64, "total": total }));

    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create file: {e}"))?;
    let mut got: u64 = 0;
    let mut last_emit = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("stream: {e}"))?;
        use std::io::Write;
        file.write_all(&bytes).map_err(|e| format!("write: {e}"))?;
        got += bytes.len() as u64;
        // Throttle progress events to ~every 2MB so the bar moves smoothly from
        // the very first chunk (was 8MB — which looked frozen at 0% for ages).
        if got - last_emit > 2_000_000 {
            last_emit = got;
            let _ = app.emit(&channel, &serde_json::json!({ "got": got, "total": total }));
        }
    }
    drop(file);
    std::fs::rename(&tmp, &dest).map_err(|e| format!("finalize: {e}"))?;
    let _ = app.emit(&channel, &serde_json::json!({ "got": got, "total": total, "done": true }));
    Ok(dest.to_string_lossy().to_string())
}

/// Report a downloaded model's tool capability (detected from its GGUF chat
/// template). Used by the UI to tag installed models "works with file tools" vs
/// "chat only" — honest, per-model, no guessing.
#[tauri::command]
fn local_tool_capability(path: String) -> gguf::ToolCapability {
    gguf::detect_tool_capability(&path)
}

// --- CAPABILITY INVENTORY -------------------------------------------------
// ONE answer to "what can my agent actually do?", assembled from all origins:
//
//   built-in    native Rust (files, pdf, fetch_url, whisper) + always-on core
//   connection  contributed by an enabled Connection (github_list_prs, …)
//   mcp         discovered from an MCP server (not yet implemented — the
//               inventory is built to carry it so the UI doesn't change later)
//
// SKILLS ARE NOT HERE. A skill is saved instructions ("how I want work done"),
// not a machine capability, and it holds no credential. Mixing them was the
// confusing part of the old Tools tab: `kind='composed'` entries looked like
// tools but behaved like procedures. They get their own list.
//
// This is DERIVED state — it never stores anything. The truth lives in the tools
// registry, the connection tables, and the connector descriptors.

/// The always-available core tools. These aren't in the tools registry (they're
/// unconditional in the agent loop), but a user asking "what can my agent do?"
/// must see them or the answer is a lie.
const CORE_TOOLS: &[(&str, &str, &str)] = &[
    ("read_file", "Read files", "Read a UTF-8 text file inside the agent folder."),
    ("write_file", "Write files", "Create or overwrite a text file inside the agent folder."),
    ("append_file", "Append to files", "Add text to the end of a file without rewriting it (for content too large for one write)."),
    ("list_files", "List files", "List directory entries inside the agent folder."),
    ("rename_file", "Rename / move files", "Rename or move a file inside the agent folder."),
    ("delete_file", "Delete files", "Delete a file inside the agent folder."),
];

/// The full capability inventory for one agent, grouped by ORIGIN.
#[tauri::command]
fn capabilities_list(
    app: tauri::AppHandle,
    db: tauri::State<writer::Db>,
    agent_id: Option<String>,
    folder: Option<String>,
) -> Result<serde_json::Value, String> {
    let ad = app_data(&app)?;
    let mut items: Vec<serde_json::Value> = Vec::new();

    // 1. CORE — jailed file tools, always on, cannot be disabled.
    for (name, label, desc) in CORE_TOOLS {
        items.push(serde_json::json!({
            "name": name, "display_name": label, "description": desc,
            "origin": "built-in", "source": "Core", "enabled": true,
            "toggleable": false, "access": "Write", "id": format!("core.{name}"),
        }));
    }

    // 2. BUILT-IN REGISTRY TOOLS — pdf, whisper, fetch_url. Toggleable per agent.
    let enabled_map = agent_id.as_ref()
        .map(|a| tools_registry::load_enabled(&ad, &tools_registry::Scope::new(a, folder.as_deref())))
        .unwrap_or_default();
    for t in tools_registry::load_registry(&ad) {
        // Composed entries are SKILLS now — excluded from the tool inventory.
        if t.kind == "composed" { continue; }
        let on = match enabled_map.get(&t.id) { Some(v) => *v, None => t.builtin };
        items.push(serde_json::json!({
            "name": t.name, "display_name": t.display_name, "description": t.description,
            "origin": "built-in", "source": "Built-in", "enabled": on,
            "toggleable": true, "access": "Write", "id": t.id,
            "has_config": !tools_registry::config_schema(&t.id).as_array().map(|a| a.is_empty()).unwrap_or(true),
        }));
    }

    // 3. CONNECTION TOOLS — contributed by whatever is enabled for THIS agent.
    // Read the same way the agent loop does, so the inventory can't drift from
    // what the model is actually given.
    if let Some(aid) = agent_id.as_deref() {
        for (provider, _legacy_access) in connections::enabled_providers_for_agent(&db, aid) {
            let Some(def) = connectors::by_id(&provider) else { continue };
            let off = connections::disabled_tools(&db, aid, &provider);
            for t in def.tools_granted(true, &off) {
                items.push(serde_json::json!({
                    "name": t.name, "display_name": t.name, "description": t.description,
                    "origin": "connection", "source": def.label, "enabled": true,
                    "toggleable": false, "id": format!("conn.{}.{}", def.id, t.name),
                    "access": if t.access == connectors::Access::Write { "Write" } else { "Read" },
                }));
            }
        }
    }

    // 4. MCP — placeholder shape, deliberately empty until the client lands.

    Ok(serde_json::json!(items))
}

// --- SPARKS (interactive AI-built mini-apps) -------------------------------
// A Spark is a SELF-CONTAINED mini-app the agent authors as a single
// `Sparks/<slug>/index.html` inside the agent folder (jailed). It runs in a
// SANDBOXED iframe (sandbox="allow-scripts", NO same-origin) in the Sparks tab
// — so it can run its own JS + any data the agent EMBEDDED at build time, but
// can NOT reach the file system, the network to this machine, or the agent.
// The agent gives it data; the Spark never calls back. These commands are the
// read/list/delete surface for the Sparks library; the agent BUILDS a Spark
// with ordinary jailed write_file (taught by the built-in "sparks" skill).

/// One Spark's metadata (from its spark.json, with sane fallbacks).
fn spark_slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 80
        && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// List every Spark in the agent folder: scans Sparks/<slug>/ for an index.html
/// (+ optional spark.json for title/description/created). Jailed via the broker.
#[tauri::command]
fn sparks_list(broker: tauri::State<Arc<Broker>>, agent_id: String) -> Result<serde_json::Value, String> {
    let dir = match broker.resolve(&agent_id, "Sparks", broker::Mode::Read) {
        Ok(p) => p,
        Err(_) => return Ok(serde_json::json!([])), // no Sparks/ yet → empty library
    };
    let mut out: Vec<serde_json::Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() { continue; }
            let slug = e.file_name().to_string_lossy().to_string();
            if !spark_slug_ok(&slug) { continue; }
            if !p.join("index.html").is_file() { continue; }
            // Optional manifest.
            let (mut title, mut description, mut created) = (slug.clone(), String::new(), 0i64);
            if let Ok(txt) = std::fs::read_to_string(p.join("spark.json")) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                    if let Some(t) = v.get("title").and_then(|x| x.as_str()) { title = t.to_string(); }
                    if let Some(d) = v.get("description").and_then(|x| x.as_str()) { description = d.to_string(); }
                    if let Some(c) = v.get("created").and_then(|x| x.as_i64()) { created = c; }
                }
            }
            let modified = std::fs::metadata(p.join("index.html")).ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            out.push(serde_json::json!({
                "slug": slug, "title": title, "description": description,
                "created": created, "modified": modified,
            }));
        }
    }
    // Newest activity first.
    out.sort_by(|a, b| b.get("modified").and_then(|x| x.as_i64()).unwrap_or(0)
        .cmp(&a.get("modified").and_then(|x| x.as_i64()).unwrap_or(0)));
    Ok(serde_json::json!(out))
}

/// Read one Spark's index.html (jailed). Returned as a STRING the UI injects as
/// an iframe srcdoc under sandbox="allow-scripts" (no same-origin) — the Spark
/// runs isolated; it can never read this file path or reach the agent.
#[tauri::command]
fn sparks_read(broker: tauri::State<Arc<Broker>>, agent_id: String, slug: String) -> Result<serde_json::Value, String> {
    if !spark_slug_ok(&slug) { return Err("invalid spark name".into()); }
    let rel = format!("Sparks/{slug}/index.html");
    let html = match broker.resolve_and_open(&agent_id, &rel, broker::Mode::Read) {
        Ok(mut f) => { use std::io::Read; let mut s = String::new(); f.read_to_string(&mut s).map_err(|e| format!("read: {e}"))?; s }
        Err(e) => return Err(format!("refused by jail: {e:?}")),
    };
    Ok(serde_json::json!({ "slug": slug, "html": html }))
}

/// Delete a Spark (its whole Sparks/<slug>/ folder). Jailed: resolves a sentinel
/// inside the folder through the broker (Write) to prove it's in-scope, then
/// removes the directory.
#[tauri::command]
fn sparks_delete(broker: tauri::State<Arc<Broker>>, agent_id: String, slug: String) -> Result<(), String> {
    if !spark_slug_ok(&slug) { return Err("invalid spark name".into()); }
    let sentinel = format!("Sparks/{slug}/index.html");
    let abs = broker.resolve(&agent_id, &sentinel, broker::Mode::Write)
        .map_err(|e| format!("refused by jail: {e:?}"))?;
    let spark_dir = abs.parent().ok_or("could not resolve spark dir")?.to_path_buf();
    // Paranoia: the resolved dir must actually be .../Sparks/<slug>.
    if spark_dir.file_name().and_then(|n| n.to_str()) != Some(slug.as_str()) {
        return Err("spark path mismatch".into());
    }
    if spark_dir.is_dir() {
        std::fs::remove_dir_all(&spark_dir).map_err(|e| format!("delete: {e}"))?;
    }
    Ok(())
}

/// SAVE a Spark from the chat preview into the library: writes
/// Sparks/<slug>/index.html + spark.json (jailed via the broker). Called by the
/// inline preview card's "Save to Library" button.
#[tauri::command]
fn spark_save(
    broker: tauri::State<Arc<Broker>>,
    agent_id: String,
    slug: String,
    title: String,
    html: String,
    description: Option<String>,
) -> Result<(), String> {
    if !spark_slug_ok(&slug) { return Err("invalid spark name".into()); }
    if html.trim().is_empty() { return Err("nothing to save".into()); }
    // index.html
    let html_rel = format!("Sparks/{slug}/index.html");
    let html_abs = broker.resolve(&agent_id, &html_rel, broker::Mode::Write)
        .map_err(|e| format!("refused by jail: {e:?}"))?;
    if let Some(parent) = html_abs.parent() { std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?; }
    std::fs::write(&html_abs, html.as_bytes()).map_err(|e| format!("write html: {e}"))?;
    // spark.json manifest
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    let manifest = serde_json::json!({
        "title": if title.trim().is_empty() { slug.clone() } else { title },
        "description": description.unwrap_or_default(),
        "created": created,
    });
    let man_rel = format!("Sparks/{slug}/spark.json");
    let man_abs = broker.resolve(&agent_id, &man_rel, broker::Mode::Write)
        .map_err(|e| format!("refused by jail: {e:?}"))?;
    std::fs::write(&man_abs, serde_json::to_string_pretty(&manifest).unwrap_or_default())
        .map_err(|e| format!("write manifest: {e}"))?;
    Ok(())
}

// SPARK STATE (jailed KV) — the persistence layer that makes Sparks real apps.
// The Spark's injected runtime marshals localStorage + window.spark over
// postMessage to the host; the host lands here, jailed to Sparks/<slug>/.

/// Read a Spark's whole KV blob (jailed). `{}` when it has none yet.
#[tauri::command]
fn spark_state_get(broker: tauri::State<Arc<Broker>>, agent_id: String, slug: String) -> Result<serde_json::Value, String> {
    spark_state::read(&broker, &agent_id, &slug)
}

/// Overwrite a Spark's whole KV blob (jailed). `values` must be a JSON object.
#[tauri::command]
fn spark_state_set(broker: tauri::State<Arc<Broker>>, agent_id: String, slug: String, values: serde_json::Value) -> Result<(), String> {
    spark_state::write(&broker, &agent_id, &slug, &values)
}

/// Set OR remove ONE key in a Spark's KV blob (jailed). `value: null` removes.
#[tauri::command]
fn spark_state_set_key(broker: tauri::State<Arc<Broker>>, agent_id: String, slug: String, key: String, value: serde_json::Value) -> Result<(), String> {
    if value.is_null() {
        spark_state::remove_key(&broker, &agent_id, &slug, &key)
    } else {
        spark_state::set_key(&broker, &agent_id, &slug, &key, value)
    }
}

/// SKILLS — saved procedures (instructions + an allowed subset of real tools).
/// Same storage as before (`kind: "composed"` in the tools registry); this is a
/// clearer name and a separate list, not a migration.
#[tauri::command]
fn skills_list(
    app: tauri::AppHandle,
    agent_id: Option<String>,
    folder: Option<String>,
) -> Result<serde_json::Value, String> {
    let ad = app_data(&app)?;
    let enabled = agent_id.as_ref()
        .map(|a| tools_registry::load_enabled(&ad, &tools_registry::Scope::new(a, folder.as_deref())))
        .unwrap_or_default();
    let out: Vec<serde_json::Value> = tools_registry::load_registry(&ad).iter()
        .filter(|t| t.kind == "composed")
        .map(|t| {
            let on = enabled.get(&t.id).copied().unwrap_or(false);
            serde_json::json!({
                "id": t.id, "name": t.name, "display_name": t.display_name,
                "description": t.description, "instructions": t.instructions,
                "allowed_tools": t.allowed_tools, "enabled": on, "builtin": t.builtin,
            })
        })
        .collect();
    Ok(serde_json::json!(out))
}

// --- TOOLS registry (extensible agent capabilities) ------------------------
// (uses the existing `app_data` helper defined earlier)

/// Full registry (builtins + user tools) with each tool's enabled-state for a
/// folder folded in.
#[tauri::command]
fn tools_list(app: tauri::AppHandle, agent_id: Option<String>, folder: Option<String>) -> Result<serde_json::Value, String> {
    let ad = app_data(&app)?;
    let all = tools_registry::load_registry(&ad);
    let enabled = agent_id.as_ref()
        .map(|a| tools_registry::load_enabled(&ad, &tools_registry::Scope::new(a, folder.as_deref())))
        .unwrap_or_default();
    let out: Vec<serde_json::Value> = all.iter().map(|t| {
        let on = match enabled.get(&t.id) { Some(v) => *v, None => t.builtin };
        serde_json::json!({
            "id": t.id, "name": t.name, "display_name": t.display_name,
            "description": t.description, "kind": t.kind, "builtin": t.builtin,
            "instructions": t.instructions, "allowed_tools": t.allowed_tools,
            "enabled": on,
        })
    }).collect();
    Ok(serde_json::json!(out))
}

/// Create/update a composed (user) tool.
#[tauri::command]
fn tools_upsert(app: tauri::AppHandle, tool: tools_registry::ToolDef) -> Result<(), String> {
    tools_registry::upsert_tool(&app_data(&app)?, tool)
}

#[tauri::command]
fn tools_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    tools_registry::delete_tool(&app_data(&app)?, &id)
}

#[tauri::command]
fn tools_set_enabled(app: tauri::AppHandle, agent_id: String, folder: Option<String>, id: String, on: bool) -> Result<(), String> {
    let scope = tools_registry::Scope::new(&agent_id, folder.as_deref());
    tools_registry::set_enabled(&app_data(&app)?, &scope, &id, on)
}

/// A tool's config SCHEMA (what settings it exposes) + the folder's saved VALUES.
#[tauri::command]
fn tools_config(app: tauri::AppHandle, agent_id: String, folder: Option<String>, id: String) -> Result<serde_json::Value, String> {
    let ad = app_data(&app)?;
    let scope = tools_registry::Scope::new(&agent_id, folder.as_deref());
    Ok(serde_json::json!({
        "schema": tools_registry::config_schema(&id),
        "values": tools_registry::tool_config(&ad, &scope, &id),
        "fonts": pdf_tool::system_fonts(),
    }))
}

/// Save a tool's config values for a folder.
#[tauri::command]
fn tools_set_config(app: tauri::AppHandle, agent_id: String, folder: Option<String>, id: String, values: serde_json::Value) -> Result<(), String> {
    let scope = tools_registry::Scope::new(&agent_id, folder.as_deref());
    tools_registry::set_tool_config(&app_data(&app)?, &scope, &id, values)
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
    app: tauri::AppHandle,
    broker: tauri::State<'_, Arc<Broker>>,
    browser_state: tauri::State<'_, browser::BrowserProc>,
    db: tauri::State<'_, writer::Db>,
    folder: Option<String>,
    prompt: String,
    // Per-call event channel the Browser panel listens on for the live CHECKLIST
    // (browser:agent-plan / browser:agent-step / browser:agent-done). Optional so
    // older callers still work; when absent we just skip the UI emits.
    channel: Option<String>,
) -> Result<String, String> {
    use tauri::Emitter;
    // Resolve the ACTIVE agent's actual provider + model (the rail selection).
    // The Browser panel passes folder:null, so we resolve via the active agent id.
    // Hardcoding Anthropic here meant Muse in the sidebar still ran Anthropic.
    let resolved_agent_id = agent_for_folder(&db, folder.as_deref().unwrap_or("")).ok();
    let (provider, configured_model): (String, Option<String>) = match resolved_agent_id.as_deref().and_then(|aid| repo::get_agent(&db, aid).ok()).flatten() {
        Some(a) => (if a.provider.is_empty() { "anthropic".into() } else { a.provider.clone() }, if a.model.trim().is_empty() { None } else { Some(a.model.clone()) }),
        None => ("anthropic".into(), None),
    };
    let provider = provider.as_str();
    let (key, model): (String, String) = match provider {
        "openai" | "openrouter" => {
            let k = keychain::get_key(provider).map_err(|_| format!("no {provider} key set — add one in Settings"))?;
            let m = match configured_model.clone() {
                Some(m) if !m.trim().is_empty() => m,
                _ => {
                    // auto-pick: prefer a cheap/default model from live list
                    let models = openai_provider::list_models(provider, &k).await.unwrap_or_default();
                    models.first().cloned().unwrap_or_else(|| if provider=="openai" { "gpt-4o-mini".into() } else { "openai/gpt-4o-mini".into() })
                }
            };
            (k, m)
        },
        "meta" => {
            let k = keychain::get_key("meta").map_err(|_| "no meta key set — add one in Settings".to_string())?;
            let m = configured_model.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "muse-spark-1.2".into());
            (k, m)
        },
        _ => {
            // anthropic + default
            let k = keychain::get_key("anthropic").map_err(|_| "no anthropic key set — add one first".to_string())?;
            let models = provider::anthropic_list_models(&k).await?;
            let m = match configured_model.clone() {
                Some(m) if !m.trim().is_empty() => m,
                _ => models.iter().find(|m| m.contains("sonnet")).or_else(|| models.iter().find(|m| !m.contains("haiku"))).cloned().or_else(|| models.first().cloned()).ok_or_else(|| "account returned no usable models".to_string())?,
            };
            (k, m)
        }
    };
    eprintln!("[aygent][browser][AGENT] provider={provider} model={model} (resolved={:?})", resolved_agent_id);

    // Helper: emit a checklist event to the FE if a channel was provided.
    let emit_ev = |kind: &str, payload: serde_json::Value| {
        if let Some(ch) = channel.as_deref() {
            let mut p = payload;
            if let Some(obj) = p.as_object_mut() { obj.insert("kind".into(), serde_json::json!(kind)); }
            let _ = app.emit(ch, &p);
        }
    };

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
    // PLUS the browser tools (Slice 4/5) so the hand-off panel's agent can act in
    // the VISIBLE tab — that's the whole point of this path being called from
    // the Browser screen.
    let mut tools = serde_json::json!([
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
        },
        {
            "name": "step_done",
            "description": "Call this the INSTANT you finish one step of your plan. Pass the step's number (1-based). This checks the step off in the human's live checklist. Calling step_done for the FINAL step ENDS your turn. Advance the plan one step at a time — do not skip or repeat.",
            "input_schema": { "type": "object", "properties": { "step": { "type": "integer", "description": "1-based number of the step you just completed" } }, "required": ["step"] }
        },
        {
            "name": "task_complete",
            "description": "Call this to STOP EARLY if the task is fully done before all plan steps are needed. Provide a one-sentence summary. Normally you finish by calling step_done on the last step instead.",
            "input_schema": { "type": "object", "properties": { "summary": { "type": "string", "description": "one-sentence summary of what you did" } }, "required": ["summary"] }
        }
    ]);
    if let Some(arr) = tools.as_array_mut() {
        for schema in browser::agent_tool_schemas() { arr.push(schema); }
    }

    let system = "You are AYGENT, driving the in-app browser in the tab the human is watching. \
        Use the browser tools to ACT in that page: browser_open (navigate to a URL), browser_read \
        (see the current page's text), browser_click_text (click a link/button by its visible text), \
        browser_type_text (type into a field, optionally submit), browser_click_first_result \
        (click the FIRST organic search result — takes no arguments). You also have \
        read_file/write_file/list_files for the user's folder.\n\n\
        HOW TO WORK:\n\
        - To search Google: the page is already google.com. Call browser_type_text with the query \
        and submit:true. Then call browser_read ONCE to see the results.\n\
        - To open the FIRST search result: call browser_click_first_result (NO arguments) — it \
        deterministically clicks the first real result and skips ads. NEVER use browser_click_text \
        with a guessed title for the first result.\n\
        - To open a SPECIFIC named link: call browser_click_text with distinctive text from that \
        link. The human approves each click.\n\
        - After EACH tool call, the tool returns the current page title + URL. TRUST IT. If the URL \
        changed to the destination you intended, the action SUCCEEDED.\n\n\
        THE PLAN IS THE LAW (CRITICAL — do EXACTLY this, nothing else):\n\
        - You were given an ordered PLAN. It is the ONLY authority for what to do. Execute it ONE STEP \
        AT A TIME, strictly in order. Each turn you will be told the CURRENT step — do ONLY that step.\n\
        - You MUST NOT take any action outside the current step. Do NOT explore, do NOT open extra \
        pages, do NOT 'double-check' by re-searching. If it isn't the current step, don't do it.\n\
        - The INSTANT the current step's goal is met, call `step_done` with that step's number. This \
        checks it off in the human's live checklist. Only then move on.\n\
        - Steps already checked off are FINISHED FOREVER. NEVER redo a completed step. In particular, \
        once you have navigated OFF Google onto a destination, you may NOT go back to Google or \
        re-open/re-search it — that step is done.\n\
        - Reads are AUTHORITATIVE: after each tool call the result shows the current page title + URL. \
        TRUST IT. If the URL is the destination you intended, that step SUCCEEDED — mark it done.\n\
        - 'Click the first result' is DONE the instant the page navigates off the results page onto \
        the destination. Mark it done and STOP touching Google.\n\
        - If a step genuinely CANNOT be completed (element missing, page won't navigate, permission \
        denied), say so plainly in text and call task_complete with a one-sentence reason — do NOT \
        keep retrying or wander to a different approach.\n\
        - Calling step_done on the LAST step ENDS your turn. That is how you finish. You do not need \
        task_complete unless you're stopping early.\n\n\
        Be concise. Prefer the FEWEST tool calls. Advance the checklist in order; never loop, never wander.";

    // =====================================================================
    // PLAN-FIRST (Problem 2 — the visible checklist Mason has asked for 4×).
    // Before ANY action, make ONE model call that decomposes the task into an
    // ordered list of concrete, checkable steps. We render it live in the Agent
    // panel and check each off as it completes. The checklist STRUCTURALLY
    // prevents the old re-search loop: if the plan is ["type query + submit",
    // "click first result"] and step 2 is checked, there is no "search again"
    // step to fall into — when every step is checked the turn ENDS.
    // =====================================================================
    let plan_system = "You are a browsing task planner. Decompose the user's task into the SHORTEST \
        ordered list of concrete browser steps needed to finish it. Each step is one short imperative \
        phrase (e.g. \"type 'claude' in the search box and submit\", \"click the first result\", \"read \
        the page\"). For opening the top search hit, phrase the step as \"click the first result\" \
        (the runner has a dedicated deterministic tool for it). Do NOT include steps for opening the \
        browser (it's already open) or for reporting \
        back. Prefer 1-4 steps. Reply with ONLY a JSON array of strings, nothing else.";
    let plan_msgs = serde_json::json!([{ "role": "user", "content": prompt }]);
    let no_tools = serde_json::json!([]);
    let mut steps: Vec<String> = Vec::new();
    // Planner uses the SAME provider/model as the agent (Muse in sidebar -> Muse planner).
    let plan_resp: Result<serde_json::Value, String> = match provider {
        "openai" | "openrouter" => {
            let _msgs = serde_json::json!([{ "role": "user", "content": prompt }]);
            // Use the provider's raw turn via openai_provider but we need a text-only call; use complete helper via a synthetic turn
            // Instead do a minimal stream-equivalent: reuse anthropic planner prompt via openai complete
            // For now, call openai_provider::openai_stream_turn with no tools and collect text
            let _ = &key; // keeps borrow
            // Fallback: try anthropic as planner even when browsing on another provider? No — use the right provider.
            // Simplest: call openai's complete-style turn by using provider::anthropic_turn only as fallback? Let's dispatch properly:
            // We will attempt to use the selected provider's turn; on error fall back to anthropic if available.
            // To avoid async borrow issues, just call the provider's one-shot helper.
            // For openai/openrouter: use openai_provider::complete with the plan prompt
            match openai_provider::complete(&provider.to_string(), &key, &model, &format!("{plan_system}\n\nTask: {prompt}")).await {
                Ok(text) => Ok(serde_json::json!({"content": [{"type":"text","text": text}]})),
                Err(e) => Err(e),
            }
        },
        "meta" => {
            match meta_provider::complete(&key, &model, &format!("{plan_system}\n\nTask: {prompt}")).await {
                Ok(text) => Ok(serde_json::json!({"content": [{"type":"text","text": text}]})),
                Err(e) => Err(e),
            }
        },
        _ => provider::anthropic_turn(&key, &model, plan_system, &plan_msgs, &no_tools).await,
    };
    if let Ok(resp) = plan_resp {
        let text = resp.get("content").and_then(|c| c.as_array())
            .map(|arr| arr.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""))
            .unwrap_or_default();
        steps = parse_plan_steps(&text);
    }
    if steps.is_empty() {
        // Fallback plan so the checklist always renders + the loop still ends.
        steps = vec!["Do the task on the current page".to_string(), "Confirm it's done".to_string()];
    }
    eprintln!("[aygent][browser][PLAN] {} steps: {:?}", steps.len(), steps);
    emit_ev("plan", serde_json::json!({ "steps": steps.clone() }));

    // Tell the model its own plan + that it must advance it explicitly.
    let plan_note = {
        let listed = steps.iter().enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s)).collect::<Vec<_>>().join("\n");
        format!("Your plan (execute in order, one at a time):\n{listed}\n\nAfter you FINISH each step, \
            call `step_done` with its number. When the LAST step is done, calling step_done for it \
            ends the turn — you do NOT also need task_complete (but you may call task_complete to stop early).")
    };

    let mut messages = serde_json::json!([{ "role": "user", "content": format!("{prompt}\n\n{plan_note}") }]);
    let mut transcript = String::new();
    transcript.push_str(&format!("[{model}]\n"));

    // Checklist progress. `next_step` is the 0-based index of the next unchecked
    // step; when it reaches steps.len() every step is checked => DONE.
    let mut next_step: usize = 0;
    let mut browser_actions = 0u32;
    // PER-STEP RETRY CAP (spec #3 — honest failure, no infinite loop). Count
    // consecutive FAILED browser actions while the plan is parked on the same
    // step. If a step can't complete after MAX_STEP_RETRIES attempts, surface a
    // clear message to the user and STOP (don't loop forever). Resets whenever
    // the checklist advances (next_step changes).
    const MAX_STEP_RETRIES: u32 = 2;
    let mut step_fail_streak: u32 = 0;
    let mut streak_step: usize = 0;
    // STEP-OVERRUN GUARD (Mason's Part-1 fix). Once a NAV-step's navigation
    // actually lands on a real (non-search) destination, we record the step
    // index here. While `satisfied_step == Some(next_step)`:
    //   - the per-turn CURRENT-STATE tells the model the step's goal is MET and
    //     to advance (step_done) or task_complete — NOT click/open/type again;
    //   - a further browser_click_text/browser_open/browser_type_text on that
    //     SAME step is REJECTED (never executed) with "this step is already
    //     complete — advance", so a wandering second click can't happen.
    // Keyed off the ACTUAL plan step + ACTUAL navigation, not hardcoded google
    // logic. Cleared whenever the checklist advances (next_step changes).
    let mut satisfied_step: Option<usize> = None;
    // Iteration cap scales with plan size (each step may need a couple tool
    // calls) but stays bounded so a misbehaving model can't spin forever.
    let max_iters = (steps.len() * 4).clamp(8, 24);

    // Agent loop: cap iterations so a misbehaving model can't spin forever.
    for _ in 0..max_iters {
        // CHECKLIST AUTHORITY (spec #1/#2): each turn, tell the model EXACTLY
        // which step it is on and forbid anything else. This is the runtime
        // enforcement of sequential execution — combined with the deterministic
        // step_done -> next_step advance + the retry cap, the model cannot
        // legitimately wander back to a finished step.
        let cur_idx = next_step.min(steps.len().saturating_sub(1));
        let done_list = if next_step == 0 {
            "(none yet)".to_string()
        } else {
            steps.iter().take(next_step).enumerate()
                .map(|(i, s)| format!("{}. {} ✓", i + 1, s)).collect::<Vec<_>>().join("; ")
        };
        // If the step's goal is ALREADY MET (a nav-step whose navigation landed
        // on a real destination), the ONLY correct next move is to advance —
        // NOT to click/open/type again. Inject that unambiguously so the model
        // calls step_done (or task_complete on the last step) instead of taking
        // another action and wandering (the exact double-click bug).
        let goal_met = satisfied_step == Some(cur_idx);
        let turn_system = if goal_met {
            let is_last = cur_idx + 1 >= steps.len();
            let cur_url = browser::current_agent_url();
            format!(
                "{system}\n\n── CURRENT STATE ──\n\
                STEP {}/{} (\"{}\") is ALREADY COMPLETE — its goal is MET. The browser \
                successfully navigated to the destination ({}).\n\
                Already completed (do NOT redo): {}.\n\
                Your ONLY valid next action is to {}. Do NOT click, open, type, or read \
                again for this step — it is finished. Do NOT take ANY browser action now.",
                cur_idx + 1, steps.len(),
                steps.get(cur_idx).map(|s| s.as_str()).unwrap_or(""),
                if cur_url.is_empty() { "the intended page".to_string() } else { cur_url },
                done_list,
                if is_last {
                    format!("call step_done({}) to finish the turn (or task_complete with a one-line summary)", cur_idx + 1)
                } else {
                    format!("call step_done({}) so the checklist advances to the next step", cur_idx + 1)
                },
            )
        } else {
            format!(
                "{system}\n\n── CURRENT STATE ──\n\
                You are on STEP {}/{}: \"{}\".\n\
                Already completed (do NOT redo): {}.\n\
                Do ONLY step {} now. When its goal is met, call step_done({}) IMMEDIATELY — \
                do NOT take a second action once the goal is achieved. \
                Do NOT act outside this step. Do NOT re-open or re-search Google if it's already done.",
                cur_idx + 1, steps.len(),
                steps.get(cur_idx).map(|s| s.as_str()).unwrap_or(""),
                done_list,
                cur_idx + 1, cur_idx + 1,
            )
        };
        // Dispatch the turn to the agent's actual provider (Muse etc.), not always Anthropic.
        let resp: serde_json::Value = match provider {
            "openai" | "openrouter" => {
                let (assistant, _stop) = openai_provider::openai_stream_turn(&provider.to_string(), &key, &model, None, &turn_system, &messages, &tools, None, |_| {}).await?;
                // Convert OpenAI assistant (tool_calls) -> Anthropic-like content for the loop below.
                // For the browser loop we only need content as Anthropic blocks; synthesize them.
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if let Some(txt) = assistant.get("content").and_then(|c| c.as_str()) {
                    if !txt.is_empty() { blocks.push(serde_json::json!({"type":"text","text": txt})); }
                }
                if let Some(tcs) = assistant.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                        let f = tc.get("function").cloned().unwrap_or(serde_json::json!({}));
                        let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let args_str = f.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                        let input: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                        blocks.push(serde_json::json!({"type":"tool_use","id": id, "name": name, "input": input}));
                    }
                }
                let stop = if blocks.iter().any(|b| b.get("type").and_then(|t| t.as_str())==Some("tool_use")) { "tool_use" } else { "end_turn" };
                serde_json::json!({"content": blocks, "stop_reason": stop})
            },
            "meta" => {
                let (assistant, _stop) = meta_provider::meta_stream_turn(&key, &model, None, &turn_system, &messages, &tools, None, |_| {}).await?;
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if let Some(txt) = assistant.get("content").and_then(|c| c.as_str()) {
                    if !txt.is_empty() { blocks.push(serde_json::json!({"type":"text","text": txt})); }
                }
                if let Some(tcs) = assistant.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                        let f = tc.get("function").cloned().unwrap_or(serde_json::json!({}));
                        let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let args_str = f.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                        let input: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                        blocks.push(serde_json::json!({"type":"tool_use","id": id, "name": name, "input": input}));
                    }
                }
                let stop = if blocks.iter().any(|b| b.get("type").and_then(|t| t.as_str())==Some("tool_use")) { "tool_use" } else { "end_turn" };
                serde_json::json!({"content": blocks, "stop_reason": stop})
            },
            _ => provider::anthropic_turn(&key, &model, &turn_system, &messages, &tools).await?,
        };
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

                    // STEP_DONE: the model checked a plan step off. Emit the
                    // checkmark to the FE. When the LAST step is checked, END the
                    // turn DETERMINISTICALLY — the checklist is what structurally
                    // stops the old re-search loop (no unchecked step left => no
                    // "search again" to fall into).
                    if name == "step_done" {
                        // Accept the number the model gave, but advance monotonically
                        // so a repeated/skipped number can't stall or overshoot.
                        let claimed = input.get("step").and_then(|s| s.as_i64()).unwrap_or(0);
                        let idx = if claimed >= 1 { (claimed as usize).saturating_sub(1).max(next_step) } else { next_step };
                        let idx = idx.min(steps.len().saturating_sub(1));
                        next_step = idx + 1;
                        // Advancing clears the step-overrun guard for the new step.
                        satisfied_step = None;
                        eprintln!("[aygent][browser][STEP] {}/{} done: {}", next_step, steps.len(),
                            steps.get(idx).map(|s| s.as_str()).unwrap_or(""));
                        emit_ev("step", serde_json::json!({ "index": idx, "done": next_step, "total": steps.len() }));
                        transcript.push_str(&format!("  ✓ step {}/{}: {}\n", next_step, steps.len(),
                            steps.get(idx).map(|s| s.as_str()).unwrap_or("")));
                        if next_step >= steps.len() {
                            // ALL steps checked => turn COMPLETE, deterministically.
                            eprintln!("[aygent][browser][DONE] all {} steps complete", steps.len());
                            emit_ev("done", serde_json::json!({ "total": steps.len() }));
                            transcript.push_str("\n✓ all steps complete\n");
                            return Ok(transcript);
                        }
                        // Not the last step — ack it and let the model continue.
                        tool_results.push(serde_json::json!({
                            "type": "tool_result", "tool_use_id": id,
                            "content": format!("step {} checked off. Now do step {} of {}: {}",
                                next_step, next_step + 1, steps.len(),
                                steps.get(next_step).map(|s| s.as_str()).unwrap_or("")),
                            "is_error": false
                        }));
                        continue;
                    }
                    // TASK_COMPLETE: the model signals it's done EARLY. End the
                    // turn DETERMINISTICALLY. This is the explicit STOP event.
                    if name == "task_complete" {
                        let summary = input.get("summary").and_then(|s| s.as_str()).unwrap_or("done");
                        transcript.push_str(&format!("\n✓ {summary}\n"));
                        eprintln!("[aygent][browser][DONE] task_complete: {summary}");
                        emit_ev("done", serde_json::json!({ "total": steps.len(), "summary": summary }));
                        return Ok(transcript);
                    }
                    // FIRST-RESULT OVERRIDE (ITEM 1, belt+suspenders). If the
                    // CURRENT plan step is a first-result step and the model
                    // tried to click a specific TEXT (browser_click_text with a
                    // model-guessed label like "Claude: Sign in"), FORCE the
                    // deterministic browser_click_first_result path instead. This
                    // removes any dependence on the model routing correctly: a
                    // first-result step ALWAYS uses the anchor-required first-
                    // organic-result selector, never the broad text-match that
                    // clicked a <div>/tweet embed in the observed bug.
                    // NOTE: we do NOT move the original `input` here (a later
                    // borrow, `path`, still references it for the file tools).
                    // We compute an OVERRIDE (name, input) only when firing the
                    // first-result path, and select which to pass below.
                    let first_result_override = name == "browser_click_text"
                        && step_is_first_result(steps.get(next_step).map(|s| s.as_str()).unwrap_or(""));
                    // IMAGE-INDEX OVERRIDE (Google Images): "click the 5th image" should use browser_click_image, not text match.
                    // Detect ordinal + image hint in the CURRENT plan step, not the model-supplied text (which is a guessed caption).
                    let step_text = steps.get(next_step).map(|s| s.as_str()).unwrap_or("");
                    let is_image_step = step_text.to_ascii_lowercase().contains("image")
                        && step_text.chars().any(|c| c.is_ascii_digit());
                    let image_index_override = is_image_step && (name == "browser_click_text" || name == "browser_click_first_result");
                    if first_result_override {
                        eprintln!("[aygent][browser][STEP] OVERRIDE browser_click_text -> browser_click_first_result on first-result step {}/{} (ignoring model text {:?})",
                            next_step + 1, steps.len(),
                            input.get("text").and_then(|t| t.as_str()).unwrap_or(""));
                    }
                    if image_index_override {
                        let idx = step_text.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().unwrap_or(1).max(1);
                        eprintln!("[aygent][browser][STEP] OVERRIDE {} -> browser_click_image[{}] on image step {}/{}: {}", name, idx, next_step+1, steps.len(), step_text);
                    }
                    let (name, image_idx): (&str, Option<u64>) = if image_index_override {
                        let idx = step_text.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().unwrap_or(1).max(1);
                        ("browser_click_image", Some(idx))
                    } else if first_result_override { ("browser_click_first_result", None) } else { (name, None) };
                    let image_override_input = image_idx.map(|i| serde_json::json!({"index": i})).unwrap_or(serde_json::json!({}));
                    let override_input = serde_json::json!({});
                    let (input, _image_guard): (&serde_json::Value, bool) = if image_idx.is_some() { (&image_override_input, true) } else if first_result_override { (&override_input, false) } else { (&input, false) };
                    // EXECUTE THROUGH THE BROKER (jailed) — or the browser tools
                    // (act on the VISIBLE tab + per-agent domain policy + wheel).
                    let (result_text, is_err) = if browser::is_agent_tool(name) {
                        // STEP-OVERRUN GUARD (Part-1 fix): if THIS step's goal is
                        // already met (a nav-step that already navigated to its
                        // destination) and the model tries ANOTHER acting browser
                        // tool on the SAME step, REJECT it — do NOT execute. This
                        // is the code-level backstop that stops the second,
                        // wandering click that landed on the wrong site. Reads are
                        // harmless; only clicks/opens/types wander.
                        let is_acting = matches!(name, "browser_click_text" | "browser_click_first_result" | "browser_click_image" | "browser_open" | "browser_type_text");
                        if is_acting && satisfied_step == Some(next_step) {
                            let cur = steps.get(next_step).map(|s| s.as_str()).unwrap_or("(current step)");
                            eprintln!("[aygent][browser][STOP] rejected extra {name} on satisfied step {}/{}: {}",
                                next_step + 1, steps.len(), cur);
                            (format!(
                                "this step is already complete — advance. Step {}/{} (\"{}\") is DONE: the page \
                                 already navigated to its destination ({}). Do NOT {name} again. Call step_done({}) \
                                 now (or task_complete if this was the last step).",
                                next_step + 1, steps.len(), cur, browser::current_agent_url(), next_step + 1,
                            ), true)
                        } else {
                        browser_actions += 1;
                        // TRANSIENT CURRENT-ACTION line (Part-2 UX): emit a single
                        // replaceable "→ doing X…" label the panel shows live and
                        // OVERWRITES each action — NOT an accumulating log. The
                        // meaningful persistent progress is the checklist.
                        {
                            let label = match name {
                                "browser_open" => format!("opening {}", input.get("url").and_then(|u| u.as_str()).unwrap_or("page")),
                                "browser_read" => "reading the page".to_string(),
                                "browser_click_text" => format!("clicking “{}”", input.get("text").and_then(|t| t.as_str()).unwrap_or("")),
                                "browser_click_first_result" => "clicking the first result".to_string(),
                                "browser_type_text" => {
                                    let submit = input.get("submit").and_then(|b| b.as_bool()).unwrap_or(false);
                                    format!("typing “{}”{}", input.get("text").and_then(|t| t.as_str()).unwrap_or(""), if submit { " and submitting" } else { "" })
                                }
                                "browser_screenshot" => "capturing the page".to_string(),
                                other => other.to_string(),
                            };
                            eprintln!("[aygent][browser][ACTION] step {}/{}: {label}", next_step + 1, steps.len());
                            emit_ev("action", serde_json::json!({ "step": next_step + 1, "total": steps.len(), "label": label }));
                        }
                        // Hard backstop only: too many actions = force a stop.
                        // Real termination is checking off the last plan step.
                        if browser_actions > (steps.len() as u32 * 4).clamp(8, 20) {
                            ("You've taken many actions without finishing your plan. Call step_done for the remaining steps, or task_complete with a summary.".to_string(), true)
                        } else {
                            // No configured allowlist — the human-in-the-loop
                            // permission flow governs new hosts; current tab host
                            // is pre-allowed. Pass empty.
                            let domains: Vec<String> = Vec::new();
                            let out = browser::agent_tool(&app, &browser_state, name, input, &domains).await;

                            // ==========================================================
                            // HARD STOP (Mason's spec #5): Deny / Take Control.
                            // browser.rs signals these by returning a result whose
                            // text STARTS WITH a sentinel token (__DENIED__ /
                            // __TAKEOVER__). We NEVER feed that back to the model —
                            // we END THE TURN right here so the agent cannot call
                            // browser_open again or wander. The wheel was already
                            // set (deny->idle, take->human) inside agent_tool.
                            // ==========================================================
                            if browser::is_hard_stop(&out.0) {
                                let denied = out.0.starts_with(browser::STOP_DENIED);
                                let tail = out.0
                                    .trim_start_matches(browser::STOP_DENIED)
                                    .trim_start_matches(browser::STOP_TAKEOVER)
                                    .trim();
                                let reason = if denied {
                                    format!("stopped: user denied — {tail}")
                                } else {
                                    format!("stopped: user took control — {tail}")
                                };
                                eprintln!("[aygent][browser][STOP] hard stop ({}) at step {}/{}: {}",
                                    if denied { "DENY" } else { "TAKEOVER" },
                                    next_step + 1, steps.len(), tail);
                                transcript.push_str(&format!("\n⛔ {reason}\n"));
                                // Tell the FE the turn ended (spinner off, checklist
                                // frozen where it is) + who's driving now.
                                emit_ev("done", serde_json::json!({
                                    "total": steps.len(),
                                    "stopped": true,
                                    "reason": if denied { "denied" } else { "takeover" },
                                    "at_step": next_step + 1,
                                    "summary": reason,
                                }));
                                return Ok(transcript);
                            }
                            out
                        }
                        } // end step-overrun-guard else (step not already satisfied)
                    } else { match name {
                        "read_file" => match broker.resolve_and_open(resolved_agent_id.as_deref().filter(|a| broker.root_for(a).is_ok()).unwrap_or("default"), path, broker::Mode::Read) {
                            Ok(mut f) => {
                                use std::io::Read;
                                let mut s = String::new();
                                match f.read_to_string(&mut s) {
                                    // PAGED (2026-09-22): whole-file returns blew the context
                                    // on large files and truncated mid-JSON in transit.
                                    Ok(_) => paged_read(&s, input),
                                    Err(e) => (format!("io error: {e}"), true),
                                }
                            }
                            Err(e) => (format!("refused by jail: {e:?}"), true),
                        },
                        "write_file" => {
                            let cnt = input.get("content").and_then(|c| c.as_str()).unwrap_or("");
                            if let Ok(real) = broker.resolve(resolved_agent_id.as_deref().filter(|a| broker.root_for(a).is_ok()).unwrap_or("default"), path, broker::Mode::Write) {
                                if let Some(parent) = real.parent() { let _ = std::fs::create_dir_all(parent); }
                            }
                            match broker.resolve_and_open(resolved_agent_id.as_deref().filter(|a| broker.root_for(a).is_ok()).unwrap_or("default"), path, broker::Mode::Write) {
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
                        "list_files" => match broker.resolve(resolved_agent_id.as_deref().filter(|a| broker.root_for(a).is_ok()).unwrap_or("default"), path, broker::Mode::Read) {
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
                    } };

                    // PER-STEP RETRY CAP (spec #3). Track consecutive FAILED
                    // browser actions on the CURRENT step. A success resets the
                    // streak; the checklist advancing (handled by step_done above)
                    // also naturally moves us to a new streak_step. After
                    // MAX_STEP_RETRIES failures on the same step, we surface an
                    // honest "couldn't complete" message and STOP the turn rather
                    // than let the model retry forever / wander.
                    // A guard-rejection ("this step is already complete — advance")
                    // is NOT a genuine action failure — it means the step already
                    // SUCCEEDED. Do NOT let it count toward the retry cap (that
                    // would falsely hard-stop a completed step). It's is_err only
                    // so the model treats it as "don't do that; advance".
                    let guard_reject = result_text.starts_with("this step is already complete");
                    if browser::is_agent_tool(name) && !guard_reject {
                        if streak_step != next_step { streak_step = next_step; step_fail_streak = 0; }
                        if is_err {
                            step_fail_streak += 1;
                            eprintln!("[aygent][browser][STEP] fail {}/{} on step {}/{}: {}",
                                step_fail_streak, MAX_STEP_RETRIES, next_step + 1, steps.len(),
                                result_text.chars().take(160).collect::<String>());
                            if step_fail_streak >= MAX_STEP_RETRIES {
                                let cur = steps.get(next_step).map(|s| s.as_str()).unwrap_or("(current step)");
                                let msg = format!(
                                    "stopped: couldn't complete step {}/{} (\"{}\") after {} attempts — {}",
                                    next_step + 1, steps.len(), cur, step_fail_streak,
                                    result_text.chars().take(240).collect::<String>());
                                eprintln!("[aygent][browser][STOP] step retry cap hit: {msg}");
                                transcript.push_str(&format!("\n⛔ {msg}\n"));
                                emit_ev("done", serde_json::json!({
                                    "total": steps.len(),
                                    "stopped": true,
                                    "reason": "step_failed",
                                    "at_step": next_step + 1,
                                    "summary": msg,
                                }));
                                return Ok(transcript);
                            }
                        } else {
                            step_fail_streak = 0;
                        }
                    }

                    // ==========================================================
                    // STEP-OVERRUN DETECTION (Part-1 fix — the core of the bug).
                    // If the CURRENT step is a NAV-step ("click the first result",
                    // "open the link", "go to X"…) and an ACTING browser tool just
                    // SUCCEEDED and the page is now on a REAL non-search
                    // destination, the step's GOAL IS MET. Mark it satisfied so
                    // (a) next turn's CURRENT-STATE says "advance, don't act", and
                    // (b) any further click/open/type on this same step is
                    // rejected by the guard above. Keyed off the actual plan step
                    // + actual navigation — NOT the old left_google turn-gate.
                    // We ALSO amend the tool_result the model sees so it advances
                    // instead of clicking again on this very turn.
                    let mut satisfied_now = false;
                    if browser::is_agent_tool(name)
                        && !is_err
                        && matches!(name, "browser_click_text" | "browser_click_first_result" | "browser_click_image" | "browser_open" | "browser_type_text")
                        && satisfied_step != Some(next_step)
                    {
                        let cur_step_txt = steps.get(next_step).map(|s| s.as_str()).unwrap_or("");
                        if step_is_nav(cur_step_txt) {
                            let host = browser::current_agent_host();
                            if !browser::is_search_host(&host) {
                                satisfied_step = Some(next_step);
                                satisfied_now = true;
                                eprintln!("[aygent][browser][STEP] goal MET for step {}/{} (nav landed on {}): require step_done next",
                                    next_step + 1, steps.len(), browser::current_agent_url());
                            }
                        }
                    }

                    transcript.push_str(&format!("  ⚙ {name}({path}) → {}\n",
                        if is_err { format!("✗ {result_text}") } else { "✓".to_string() }));

                    // When the nav-step just became satisfied, steer the model to
                    // advance THIS turn: append an explicit instruction to the
                    // tool_result so it calls step_done next instead of clicking
                    // again (the exact wandering second click we're killing).
                    let content_for_model = if satisfied_now {
                        let is_last = next_step + 1 >= steps.len();
                        format!(
                            "{result_text}\n\n[STEP GOAL MET] Step {}/{} is now COMPLETE — the page navigated to its \
                             destination. Do NOT click, open, or type again for this step. {}",
                            next_step + 1, steps.len(),
                            if is_last {
                                format!("Call step_done({}) to finish (or task_complete with a one-line summary).", next_step + 1)
                            } else {
                                format!("Call step_done({}) so the checklist advances.", next_step + 1)
                            },
                        )
                    } else {
                        result_text.clone()
                    };

                    tool_results.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": content_for_model,
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

    // Loop ended without an explicit step_done/task_complete (model gave a final
    // answer, or the iteration cap hit). Emit a terminal `done` so the FE stops
    // the "working" spinner + marks the checklist finished deterministically.
    eprintln!("[aygent][browser][DONE] loop ended (steps {}/{})", next_step, steps.len());
    emit_ev("done", serde_json::json!({ "total": steps.len(), "partial": next_step < steps.len() }));
    Ok(transcript)
}

/// Parse the planner model's reply into an ordered list of step strings. The
/// planner is told to return ONLY a JSON array of strings, but models sometimes
/// wrap it in prose or a ```json fence — so we extract the first [...] slice and
/// parse that, falling back to line-splitting if JSON parse fails. Steps are
/// trimmed, de-numbered, and capped so a runaway plan can't blow the loop.
fn parse_plan_steps(text: &str) -> Vec<String> {
    let clean = |s: &str| -> String {
        // Strip a leading list marker like "1. ", "- ", "* ".
        let t = s.trim().trim_matches('"').trim();
        let t = t.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')' || c == '-' || c == '*' || c == ' ');
        t.trim().to_string()
    };
    // Prefer the JSON array slice.
    if let (Some(a), Some(b)) = (text.find('['), text.rfind(']')) {
        if b > a {
            if let Ok(arr) = serde_json::from_str::<Vec<String>>(&text[a..=b]) {
                let v: Vec<String> = arr.into_iter().map(|s| clean(&s)).filter(|s| !s.is_empty()).collect();
                if !v.is_empty() { return v.into_iter().take(8).collect(); }
            }
        }
    }
    // Fallback: split lines that look like steps.
    let v: Vec<String> = text.lines()
        .map(clean)
        .filter(|s| !s.is_empty() && s.len() > 2)
        .collect();
    v.into_iter().take(8).collect()
}

/// Does this plan step's GOAL consist of NAVIGATING to a destination — i.e. is
/// it a "click the result", "open the link", "go to X", "visit", "navigate"
/// step? Used by the STEP-OVERRUN GUARD in `agent_run`: for a nav-step, a
/// successful navigation OFF the search page onto a real destination MEANS THE
/// STEP IS DONE, so the loop must require step_done next instead of tolerating a
/// second click. This keys off the ACTUAL plan step text (not hardcoded google
/// logic) — the brittle `left_google` heuristic is NOT reintroduced.
/// Is this plan step a "click the FIRST result / first link / top result" step?
/// Used by the FIRST-RESULT OVERRIDE in `agent_run`: on such a step the model's
/// browser_click_text (with a guessed label) is rewritten to the deterministic
/// text-free `browser_click_first_result` tool, so we ALWAYS click the real
/// first organic <a> instead of whatever text the model read off the page.
fn step_is_first_result(step: &str) -> bool {
    let s = step.to_ascii_lowercase();
    (s.contains("first") && (s.contains("result") || s.contains("link") || s.contains("hit") || s.contains("listing")))
        || s.contains("top result")
        || s.contains("select the first")
        || s.contains("open the first")
        || s.contains("click the first")
}

fn is_placeholder_spin(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    // Muse Spark 1.2 at 150k+ tokens emits these placeholder prefixes (see screenshot 18:24-18:47)
    // They are text-only turns with zero tool_calls that stall until user says "continue"
    t.contains("reworking your")
        || t.contains("matching that")
        || t.contains("building your")
        || t.contains("mapping the current")
        || t.contains("pulling your current")
        || t.contains("tracing how")
        || t.contains("lining up")
        || t.contains("digging into")
        || (t.contains("openrouter") && t.contains("search"))
        || (t.contains("wiring") && t.contains("type-ahead"))
        || (t.contains("openrouter-style") && t.len() < 220)
}

fn step_is_nav(step: &str) -> bool {
    let s = step.to_ascii_lowercase();
    // Any verb that means "end up on a different page/destination".
    (s.contains("click") && (s.contains("result") || s.contains("link") || s.contains("hit")
        || s.contains("listing") || s.contains("title") || s.contains("first")))
        || s.contains("open the") || s.starts_with("open ")
        || s.contains("go to") || s.contains("navigate") || s.contains("visit")
        || s.contains("follow the") || s.contains("select the first")
}

// --- STREAMING agent loop (Phase 1) ----------------------------------------
// Emits normalized StreamEvents to the UI via Tauri events as they arrive.
// Tool calls still route through the broker (jailed). Falls back to turn-based
// automatically if the provider can't stream (UI shows a thinking animation).

/// Execute one jailed tool call, returning (result_text, is_error). `agent_id`
/// selects WHICH agent's broker scope (jail) the file op runs against — M1.4:
/// every tool call is jailed to the calling agent's own folder, not a shared
/// "default" scope, so concurrent agents can't reach into each other's folders.
fn exec_tool(
    broker: &Arc<Broker>,
    agent_id: &str,
    name: &str,
    input: &serde_json::Value,
) -> (String, bool) {
    exec_tool_cfg(broker, agent_id, name, input, &serde_json::json!({}))
}

/// Page a file's text for the model (2026-09-22): default the first 2000 lines,
/// `offset`/`limit` page through, hard char cap as a backstop. Small files pass
/// through byte-identical (no notice, no behavior change).
fn paged_read(s: &str, input: &serde_json::Value) -> (String, bool) {
    let total_lines = s.lines().count();
    let offset = input.get("offset").and_then(|o| o.as_u64()).unwrap_or(1).max(1) as usize;
    let limit = input.get("limit").and_then(|l| l.as_u64()).unwrap_or(2000).clamp(1, 5000) as usize;
    if offset == 1 && total_lines <= limit && s.chars().count() <= 30_000 {
        return (s.to_string(), false);
    }
    let start = (offset - 1).min(total_lines);
    let end = (start + limit).min(total_lines);
    let mut out: String = s.lines().skip(start).take(end - start).collect::<Vec<_>>().join("\n");
    if end < total_lines {
        out.push_str(&format!("\n\n[… lines {}–{} of {} — read more with read_file(path, offset: {}, limit: N)]", start + 1, end, total_lines, end + 1));
    }
    if out.chars().count() > 30_000 {
        let cut: String = out.chars().take(30_000).collect();
        out = format!("{cut}\n\n[… output capped at 30000 chars — re-read with a smaller limit]");
    }
    (out, false)
}

/// Same as exec_tool, but with the PDF tool's per-folder config (font/colors/
/// page-size/margins). `pdf_config` is {} when unavailable.
fn exec_tool_cfg(
    broker: &Arc<Broker>,
    agent_id: &str,
    name: &str,
    input: &serde_json::Value,
    pdf_config: &serde_json::Value,
) -> (String, bool) {
    // SCOPE KEY (Mason 08-03): file tools used the legacy "default" scope while
    // shell exec used the per-agent scope — when the agent folder moved, the two
    // roots DIVERGED (writes landed in a ghost folder the user never saw). Use
    // the agent's own scope, but fall back to "default" if it was never
    // registered — that guard is what the 07-28 'refused by jail on files that
    // plainly exist' bug was about; NoScope is the only case that falls back.
    let agent_id: &str = if broker.root_for(agent_id).is_ok() { agent_id } else { "default" };
    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
    match name {
        "read_file" => match broker.resolve_and_open(agent_id, path, broker::Mode::Read) {
            Ok(mut f) => {
                use std::io::Read;
                let mut s = String::new();
                match f.read_to_string(&mut s) {
                    // PAGED (2026-09-22): whole-file returns blew the context
                    // on large files and truncated mid-JSON in transit.
                    Ok(_) => paged_read(&s, input),
                    Err(e) => (format!("io error: {e}"), true),
                }
            }
            Err(e) => (format!("refused by jail: {e:?}"), true),
        },
        "write_file" => {
            let cnt = input.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if let Ok(real) = broker.resolve(agent_id, path, broker::Mode::Write) {
                if let Some(parent) = real.parent() { let _ = std::fs::create_dir_all(parent); }
            }
            // ATOMIC WRITE (Mason 08-06): write to a fresh temp sibling (always
            // nlink == 1 → the hardlink guard can't false-refuse), fsync, then
            // rename OVER the target. A partial write can no longer truncate the
            // real file ("long replies vanish"), and rewriting a legit
            // hardlinked/cloned file no longer hits "refused by jail". Both temp
            // and final paths resolve THROUGH THE BROKER, so the jail still
            // governs every byte. Mirrors the broker_ws write handler.
            let tmp_rel = format!("{path}.aygent-tmp-{}",
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos()).unwrap_or(0));
            match broker.resolve_and_open(agent_id, &tmp_rel, broker::Mode::Write) {
                Ok(mut f) => {
                    use std::io::Write as _;
                    match f.write_all(cnt.as_bytes()).and_then(|_| f.flush()) {
                        Ok(_) => {
                            let _ = f.sync_all();
                            drop(f);
                            let t = broker.resolve(agent_id, &tmp_rel, broker::Mode::Write);
                            let d = broker.resolve(agent_id, path, broker::Mode::Write);
                            match (t, d) {
                                (Ok(t), Ok(d)) => match std::fs::rename(&t, &d) {
                                    Ok(_) => (format!("wrote {} bytes to {path}", cnt.len()), false),
                                    Err(e) => { let _ = std::fs::remove_file(&t); (format!("io error: {e}"), true) }
                                },
                                (Ok(t), Err(e)) => { let _ = std::fs::remove_file(&t); (format!("refused by jail: {e:?}"), true) }
                                (Err(e), _) => (format!("refused by jail: {e:?}"), true),
                            }
                        }
                        Err(e) => {
                            if let Ok(t) = broker.resolve(agent_id, &tmp_rel, broker::Mode::Write) { let _ = std::fs::remove_file(&t); }
                            (format!("io error: {e}"), true)
                        }
                    }
                }
                Err(e) => (format!("refused by jail: {e:?}"), true),
            }
        }
        // APPEND (2026-09-22): add to the end of a file WITHOUT resending its
        // whole content — the actionable answer to "split a big write into
        // chunks". First chunk: write_file; following chunks: append_file.
        // Same jail + atomic tmp+rename durability as write_file.
        "append_file" => {
            let cnt = input.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if cnt.is_empty() { return ("append_file needs `content`".into(), true); }
            let target = match broker.resolve(agent_id, path, broker::Mode::Write) {
                Ok(p) => p, Err(e) => return (format!("refused by jail: {e:?}"), true),
            };
            let mut cur = String::new();
            if target.is_file() {
                match std::fs::read_to_string(&target) {
                    Ok(t) => cur = t,
                    Err(_) => return (format!("'{path}' is not a text file — append_file only appends to text"), true),
                }
            } else if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            cur.push_str(cnt);
            let tmp_rel = format!("{path}.aygent-tmp-{}",
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos()).unwrap_or(0));
            match broker.resolve_and_open(agent_id, &tmp_rel, broker::Mode::Write) {
                Ok(mut f) => {
                    use std::io::Write as _;
                    match f.write_all(cur.as_bytes()).and_then(|_| f.flush()) {
                        Ok(_) => {
                            let _ = f.sync_all();
                            drop(f);
                            let t = broker.resolve(agent_id, &tmp_rel, broker::Mode::Write);
                            let d = broker.resolve(agent_id, path, broker::Mode::Write);
                            match (t, d) {
                                (Ok(t), Ok(d)) => match std::fs::rename(&t, &d) {
                                    Ok(_) => (format!("appended {} bytes to {path} (now {} bytes)", cnt.len(), cur.len()), false),
                                    Err(e) => { let _ = std::fs::remove_file(&t); (format!("io error: {e}"), true) }
                                },
                                (Ok(t), Err(e)) => { let _ = std::fs::remove_file(&t); (format!("refused by jail: {e:?}"), true) }
                                (Err(e), _) => (format!("refused by jail: {e:?}"), true),
                            }
                        }
                        Err(e) => {
                            if let Ok(t) = broker.resolve(agent_id, &tmp_rel, broker::Mode::Write) { let _ = std::fs::remove_file(&t); }
                            (format!("io error: {e}"), true)
                        }
                    }
                }
                Err(e) => (format!("refused by jail: {e:?}"), true),
            }
        }
        // RENAME / MOVE a file (so the model never has to copy-and-orphan — Mason
        // 07-28). BOTH the source and destination are resolved THROUGH THE JAIL,
        // so a rename can only ever move a file WITHIN the agent's folder.
        "rename_file" => {
            let from = input.get("from").and_then(|p| p.as_str())
                .or_else(|| input.get("path").and_then(|p| p.as_str())).unwrap_or("");
            let to = input.get("to").and_then(|p| p.as_str())
                .or_else(|| input.get("new_path").and_then(|p| p.as_str())).unwrap_or("");
            if from.is_empty() || to.is_empty() {
                return ("rename_file needs `from` and `to` paths".into(), true);
            }
            // Source + destination both jailed, same checked scope key as every
            // other file tool (per-agent scope, "default" only if unregistered —
            // see the scope-key note at the top of exec_tool_cfg, Mason 08-03).
            let src = match broker.resolve(agent_id, from, broker::Mode::Read) {
                Ok(p) => p, Err(e) => return (format!("source refused by jail: {e:?}"), true),
            };
            let dst = match broker.resolve(agent_id, to, broker::Mode::Write) {
                Ok(p) => p, Err(e) => return (format!("destination refused by jail: {e:?}"), true),
            };
            if !src.exists() { return (format!("'{from}' does not exist"), true); }
            if dst.exists() { return (format!("'{to}' already exists — pick a different name or delete it first"), true); }
            if let Some(parent) = dst.parent() { let _ = std::fs::create_dir_all(parent); }
            match std::fs::rename(&src, &dst) {
                Ok(_) => (format!("renamed '{from}' → '{to}'"), false),
                Err(e) => (format!("rename failed: {e}"), true),
            }
        }
        // DELETE a file (Mason 07-28: the agent needs to clean up, not just create).
        // Jailed resolve; refuses directories (use a dedicated dir op later if needed).
        "delete_file" => {
            let target = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
            if target.is_empty() { return ("delete_file needs a `path`".into(), true); }
            let real = match broker.resolve(agent_id, target, broker::Mode::Write) {
                Ok(p) => p, Err(e) => return (format!("refused by jail: {e:?}"), true),
            };
            if !real.exists() { return (format!("'{target}' does not exist"), true); }
            if real.is_dir() { return (format!("'{target}' is a folder — delete_file only removes files"), true); }
            match std::fs::remove_file(&real) {
                Ok(_) => (format!("deleted '{target}'"), false),
                Err(e) => (format!("delete failed: {e}"), true),
            }
        }
        // @shared discovery (parity with the daemon list path): a bare
        // "@shared" enumerates the read-only mount LABELS this agent can browse.
        // Deeper "@shared/<label>/..." falls through to the normal resolve below.
        "list_files" if path == crate::broker::SHARED_ROOT || path == "@shared/" => {
            let labels = broker.shared_labels_for(agent_id);
            if labels.is_empty() {
                ("(no shared folders are mounted for this agent)".to_string(), false)
            } else {
                (labels.join("\n"), false)
            }
        }
        "list_files" => match broker.resolve(agent_id, path, broker::Mode::Read) {
            Ok(real) => {
                if real.is_file() {
                    // Clean guidance instead of a raw 'Not a directory (os error
                    // 20)': the model listed a FILE. Tell it to read_file instead.
                    (format!("'{path}' is a file, not a directory. Use read_file to read it, or list_files on its parent folder."), true)
                } else {
                    match std::fs::read_dir(&real) {
                        Ok(rd) => {
                            let names: Vec<String> = rd.filter_map(|e| e.ok())
                                .map(|e| e.file_name().to_string_lossy().to_string()).collect();
                            if names.is_empty() { ("(empty directory)".into(), false) }
                            else { (names.join("\n"), false) }
                        }
                        Err(e) => (format!("could not list '{path}': {e}"), true),
                    }
                }
            }
            Err(e) => (format!("refused by jail: {e:?}"), true),
        },
        // TOOLS registry: PDF generator (first built-in tool). Output path is
        // resolved THROUGH THE BROKER — a PDF can only land inside the folder.
        "generate_pdf" => {
            let title = input.get("title").and_then(|t| t.as_str()).unwrap_or("Document");
            let content = input.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let out_path = input.get("output_path").and_then(|p| p.as_str())
                .or_else(|| input.get("path").and_then(|p| p.as_str()))
                .unwrap_or("document.pdf");
            match broker.resolve(agent_id, out_path, broker::Mode::Write) {
                Ok(real) => {
                    if let Some(parent) = real.parent() { let _ = std::fs::create_dir_all(parent); }
                    match pdf_tool::generate(title, content, &real, pdf_config) {
                        Ok(_) => (format!("wrote PDF to {out_path}"), false),
                        Err(e) => (format!("pdf error: {e}"), true),
                    }
                }
                Err(e) => (format!("refused by jail: {e:?}"), true),
            }
        }
        // WEB FETCH (M1.9, first real agent tool). The daemon has NO network
        // (Seatbelt); the privileged Rust side makes the request and returns
        // only extracted text — same mediated-reach model as the path broker,
        // applied to the internet. http(s) only; internal/localhost hosts blocked.
        "fetch_url" => {
            let url = input.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if url.is_empty() { return ("fetch_url needs a `url`".into(), true); }
            match web::fetch_blocking(url) {
                Ok(text) => (text, false),
                Err(e) => (format!("fetch failed: {e}"), true),
            }
        }
        // WEB SEARCH (no key, DuckDuckGo). Same mediated-reach model as fetch_url:
        // the privileged side searches, the jailed brain gets titles/urls/snippets.
        "web_search" => {
            let q = input.get("query").and_then(|u| u.as_str()).unwrap_or("");
            if q.trim().is_empty() { return ("web_search needs a `query`".into(), true); }
            match web::search_blocking(q) {
                Ok(text) => (text, false),
                Err(e) => (format!("search failed: {e}"), true),
            }
        }
        // PRO MODE SHELL TOOLS (2026-07-31). Gated: exposed to the model ONLY
        // when the agent holds shell.exec (see agent_tools_for_full). cwd is
        // ALWAYS the agent's jailed root (exec broker pins it). shell_run is the
        // 90% one-shot (git/cargo/npm); shell_spawn/poll/kill drive long-lived
        // processes like `cargo tauri dev`. The daemon can't spawn — only the
        // exec broker does.
        // SPARKS: auto-save the preview so it JUST WORKS on both Chat and Library.
        // The agent calls spark_preview with body-only html; we persist it via the
        // jailed broker to Sparks/<slug>/index.html + spark.json so the Sparks tab
        // sees it immediately. Still renders live in Chat via the same blobUrl.
        // Deleting from the Library (sparks_delete) removes the whole folder.
        "spark_preview" => {
            let slug = input.get("slug").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let title = input.get("title").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let html = input.get("html").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if slug.is_empty() || html.trim().is_empty() {
                ("spark_preview needs a slug and full html".to_string(), true)
            } else if !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || slug.len() > 80 {
                ("invalid slug \u{2014} use lowercase letters, digits, hyphens".to_string(), true)
            } else {
                let html_rel = format!("Sparks/{}/index.html", slug);
                match broker.resolve(agent_id, &html_rel, broker::Mode::Write) {
                    Ok(abs) => {
                        if let Some(parent) = abs.parent() { let _ = std::fs::create_dir_all(parent); }
                        if let Err(e) = std::fs::write(&abs, html.as_bytes()) {
                            (format!("spark save failed: {e}"), true)
                        } else {
                            let man_rel = format!("Sparks/{}/spark.json", slug);
                            if let Ok(mabs) = broker.resolve(agent_id, &man_rel, broker::Mode::Write) {
                                let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                                let manifest = serde_json::json!({
                                    "title": if title.is_empty() { slug.clone() } else { title.clone() },
                                    "description": "",
                                    "created": created,
                                });
                                let _ = std::fs::write(&mabs, serde_json::to_string_pretty(&manifest).unwrap_or_default());
                            }
                            (format!("Spark \"{}\" ({}) is live \u{2014} previewing in chat and saved to Library (Sparks/{}/index.html). Ask for tweaks to hot-swap; delete it from the Sparks tab to remove its files.", if title.is_empty() { slug.clone() } else { title.clone() }, slug, slug), false)
                        }
                    }
                    Err(e) => (format!("spark save refused by jail: {e:?}"), true),
                }
            }
        }
        "shell_run" | "shell_spawn" | "shell_poll" | "shell_write" | "shell_kill" => {
            exec_shell_tool(agent_id, name, input)
        }
        other => (format!("unknown tool: {other}"), true),
    }
}

/// PRO MODE: dispatch a shell_* tool through the global exec broker. Returns the
/// (agent-facing text, is_error) pair like every other tool. The exec broker
/// pins cwd to the agent's jailed root and scrubs env — the tool just names a
/// program + args. Output is already bounded by the broker (digest + tail).
fn exec_shell_tool(agent_id: &str, name: &str, input: &serde_json::Value) -> (String, bool) {
    let Some(xb) = exec::global() else {
        return ("shell exec unavailable (exec broker not initialized)".into(), true);
    };
    // Resolve the jailed root for cwd-pinning via the file broker's scope.
    let root = match exec::global_root(agent_id) {
        Some(r) => r,
        None => return ("no agent folder set — pick a folder first".into(), true),
    };
    let program = input.get("program").and_then(|p| p.as_str())
        .or_else(|| input.get("cmd").and_then(|p| p.as_str())).unwrap_or("");
    let args: Vec<String> = input.get("args").and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if program.is_empty() && name != "shell_poll" && name != "shell_kill" && name != "shell_write" {
        return (format!("{name} needs a `program`"), true);
    }
    let handle = input.get("proc_handle").and_then(|h| h.as_str()).unwrap_or("");

    let result = match name {
        "shell_run" => {
            let timeout_ms = input.get("timeout_ms").and_then(|t| t.as_u64()).unwrap_or(120_000);
            xb.run_for_agent(&root, Some(agent_id), program, &args, timeout_ms)
        }
        "shell_spawn" => xb.spawn_for_agent(&root, Some(agent_id), program, &args),
        "shell_poll" => {
            let cursor = input.get("cursor").and_then(|c| c.as_u64()).unwrap_or(0);
            xb.poll(handle, cursor, /*tail_only=*/ true)
        }
        "shell_write" => {
            let data = input.get("data").and_then(|d| d.as_str()).unwrap_or("");
            xb.write_stdin(handle, data)
        }
        "shell_kill" => {
            let signal = input.get("signal").and_then(|s| s.as_str()).unwrap_or("TERM");
            xb.kill(handle, signal)
        }
        _ => Ok(serde_json::json!({ "ok": false, "error": "unknown shell tool" })),
    };
    match result {
        Ok(v) => {
            // Compact the JSON into readable text for the model: the digest +
            // tail are what it reasons over, not raw bytes.
            let is_err = v.get("ok").and_then(|o| o.as_bool()) == Some(false);
            (serde_json::to_string_pretty(&v).unwrap_or_default(), is_err)
        }
        Err(e) => (format!("shell error: {e}"), true),
    }
}

/// Base file tools, always available. The Tools registry adds MORE on top.
/// M1.4: `send_message` is included when the agent has peers (roster non-empty)
/// — see base_tools_with_peers. This bare version is the file-only fallback.
fn base_tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({ "name": "read_file", "description": "Read a UTF-8 text file inside the agent folder. Path relative to folder root. Large files are PAGED (first 2000 lines by default) — pass offset (1-based line) + limit to read more; the reply says how many lines exist.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "offset": { "type": "number", "description": "1-based line number to start from (default 1)" }, "limit": { "type": "number", "description": "max lines to return (default 2000, max 5000)" } }, "required": ["path"] } }),
        serde_json::json!({ "name": "write_file", "description": "Write a UTF-8 text file inside the agent folder. Path relative to folder root. Keep each write under ~15000 chars of content — larger payloads risk truncation in transit (reported as a parse error). For bigger content: first chunk via write_file, the rest via append_file.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] } }),
        serde_json::json!({ "name": "append_file", "description": "Add text to the END of a file inside the agent folder without rewriting it. Creates the file if missing. Use after write_file when content is too large for one call: first chunk via write_file, following chunks via append_file.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] } }),
        serde_json::json!({ "name": "list_files", "description": "List entries in a directory inside the agent folder. Path relative to root; '.' for root.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } }),
        serde_json::json!({ "name": "rename_file", "description": "Rename or move a file inside the agent folder. Use this to change a file's name (do NOT write a copy and leave the old one). Both paths are relative to the folder root.",
          "input_schema": { "type": "object", "properties": { "from": { "type": "string", "description": "current path" }, "to": { "type": "string", "description": "new path" } }, "required": ["from", "to"] } }),
        serde_json::json!({ "name": "delete_file", "description": "Delete a file inside the agent folder. Path relative to root. Only removes files, not folders.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } }),
    ]
}

/// The inter-agent messaging tool schema (M1.4 #7). Added to an agent's tools
/// only when it has peers. Delivery is ASYNC — the recipient replies on its own
/// lane; the sender does NOT block waiting.
fn send_message_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "send_message",
        "description": "Send a message to ANOTHER agent (by its id from your peer list). Delivery is asynchronous — they'll process it on their own time and may reply back to you. Use this to ask a peer for help, share information, or coordinate. You do NOT wait for a reply in this turn.",
        "input_schema": { "type": "object", "properties": {
            "to_agent": { "type": "string", "description": "the recipient agent's id (from your peer list)" },
            "message": { "type": "string", "description": "what to say to them" }
        }, "required": ["to_agent", "message"] }
    })
}

/// task_continue (2026-08-01): the agent schedules ITSELF a follow-up turn so
/// "I'll check back and report" is a real capability, not a broken promise.
fn task_continue_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "task_continue",
        "description": "Schedule YOURSELF a follow-up turn after a delay, so you can end this turn and still continue the work later (e.g. poll a long build, wait for a render, check a process). You will be woken with your note in a fresh continuation turn that streams live to your chat and has ALL your tools — you keep working there (shell_poll, read files, edit, call task_continue again), not just report. RULE: whenever you would otherwise write 'I'll check back', 'I'll continue later', 'once X finishes' or 'give me a few minutes', you MUST call this tool instead of saying it — words alone never wake you up. Put everything the next turn needs in the note (proc handles, paths, what to check, next step). Calling this pops a timer picker in chat (1/3/5/10/15 min) — the human chooses; your delay_secs is only the no-answer fallback.",
        "input_schema": { "type": "object", "properties": {
            "delay_secs": { "type": "integer", "description": "SUGGESTED seconds until wake-up (5-3600, default 60) — the human picks the real duration (1/3/5/10/15 min) in a chat modal; your value is the fallback if they do not answer" },
            "note": { "type": "string", "description": "note to self: exactly what to check/continue on wake-up (include proc handles, file paths, next steps)" }
        }, "required": ["note"] }
    })
}

/// SPARKS: the preview tool. The agent calls it with the full self-contained
/// html; the UI renders it inline in chat (sandboxed iframe) and offers a Save
/// button. Iterating = call again with the same slug. The tool itself just
/// validates + acks; the render + save happen client-side (spark_save command).
fn spark_preview_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "spark_preview",
        "description": "Render an interactive mini-app (a Spark) LIVE inline in the chat. Pass ONLY the inner body markup using AYGENT's Spark classes (card/label/seg/row/stat/stepper/input-money/grid) plus a single <script> for logic — do NOT include <style>, fonts, or <html>/<head>/<body>; the design system is injected for you. Call again with the same slug to iterate (it hot-swaps). The user saves it to their library — you do not.",
        "input_schema": { "type": "object", "properties": {
            "slug": { "type": "string", "description": "short id, lowercase letters/digits/hyphens (stable across iterations)" },
            "title": { "type": "string", "description": "human title shown on the card + in the library" },
            "html": { "type": "string", "description": "the ENTIRE self-contained HTML document (inline CSS + JS; CDN libs allowed; data embedded as a JS literal)" }
        }, "required": ["slug", "title", "html"] }
    })
}

/// WHOAMI: lets an agent introspect its OWN identity, model configuration, and
/// full capability set — even when none of it is written in its Soul. Read-only
/// and argument-free: an agent can SEE its config but cannot change it (changing
/// it stays a human action in Settings / Connections / Tools).
fn whoami_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "whoami",
        "description": "Report YOUR OWN configuration: your name, your model + provider, your context mode and folder, and every tool, connection, MCP server, and skill you currently have access to — grouped by origin. Use this when the user asks who you are, what model you're running, or what you can do. Read-only: it shows your configuration but cannot change it. Takes no arguments.",
        "input_schema": { "type": "object", "properties": {} }
    })
}

/// The JSON schema for a built-in tool by its agent-facing name.
fn builtin_tool_schema(name: &str) -> Option<serde_json::Value> {
    match name {
        "generate_pdf" => Some(serde_json::json!({
            "name": "generate_pdf",
            "description": "Create a PDF document from markdown/text content, saved into the agent folder.",
            "input_schema": { "type": "object", "properties": {
                "title": { "type": "string" },
                "content": { "type": "string", "description": "markdown or plain text body" },
                "output_path": { "type": "string", "description": "e.g. report.pdf" }
            }, "required": ["title", "content", "output_path"] }
        })),
        "transcribe_audio" => Some(serde_json::json!({
            "name": "transcribe_audio",
            "description": "Transcribe an audio file (mp3, wav, m4a, webm, ogg, flac; max 25MB) inside the agent folder to text, using OpenAI Whisper. Requires an OpenAI key in Settings.",
            "input_schema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "audio file path relative to the folder root" }
            }, "required": ["path"] }
        })),
        "fetch_url" => Some(serde_json::json!({
            "name": "fetch_url",
            "description": "Fetch a web page or API endpoint over HTTPS and return its readable text (HTML is stripped to prose). Use this to read articles, docs, or JSON APIs when you need current information. http(s) only; long pages are truncated.",
            "input_schema": { "type": "object", "properties": {
                "url": { "type": "string", "description": "the full http(s) URL to fetch" }
            }, "required": ["url"] }
        })),
        "web_search" => Some(serde_json::json!({
            "name": "web_search",
            "description": "Search the web (no key needed) and return the top hits as title + url + snippet. Use when the user asks what's current, or to find a page to read with fetch_url.",
            "input_schema": { "type": "object", "properties": {
                "query": { "type": "string", "description": "the search query" }
            }, "required": ["query"] }
        })),
        _ => None,
    }
}

/// Assemble the tool list for a turn: base file tools + any ENABLED registry
/// tools for this folder. Also returns composed-tool instructions to append to
/// the system prompt. `app` provides the app-data dir for the registry.
fn agent_tools_for(app: &tauri::AppHandle, agent_id: Option<&str>, folder: Option<&str>) -> (serde_json::Value, String) {
    agent_tools_for_ex(app, agent_id, folder, false)
}

/// M1.4: like agent_tools_for but adds the inter-agent `send_message` tool when
/// `has_peers` is true (the agent has at least one other agent to talk to).
fn agent_tools_for_ex(app: &tauri::AppHandle, agent_id: Option<&str>, folder: Option<&str>, has_peers: bool) -> (serde_json::Value, String) {
    agent_tools_for_full(app, agent_id, folder, has_peers, None)
}

/// Full assembler that ALSO adds connection tools (e.g. GitHub) when the agent
/// has that connection enabled. `db`/`agent_id` are threaded so we can check
/// per-agent enablement; None keeps the old behavior (no connection tools).
fn agent_tools_for_full(
    app: &tauri::AppHandle,
    agent_id: Option<&str>,
    folder: Option<&str>,
    has_peers: bool,
    conn_ctx: Option<(&writer::Db, &str)>,
) -> (serde_json::Value, String) {
    let mut tools = base_tools();
    tools.push(task_continue_tool());
    // WHOAMI is unconditional: an agent can always ask who/what it is, whether or
    // not it has connections. (The conn_ctx block below WON'T re-add it — see the
    // dedupe guard there.)
    tools.push(whoami_tool());
    if has_peers { tools.push(send_message_tool()); }
    let mut extra_instructions = String::new();

    // DASHBOARD / VIDEO / SPARKS (M2/v0.3): ALWAYS offered when we have an agent
    // identity — they need no connection and no credential (each agent owns its
    // dashboard 1:1, video tools are jailed to the folder, spark_preview writes
    // jailed HTML). These used to sit behind `conn_ctx.is_some()`, which silently
    // stripped them from every agent with zero connections enabled — leaving the
    // model with no tool for the job, narrating bash commands instead.
    // (Connector tools below still need conn_ctx = a db + agent identity.)
    if agent_id.is_some() {
        for schema in dashboard::tool_schemas() { tools.push(schema); }
        extra_instructions.push_str(dashboard::tool_instructions());
        // VIDEO v0.3: frame-accurate edit helpers (jailed; provisioned ffmpeg only).
        for schema in video_tools::tool_schemas() { tools.push(schema); }
        extra_instructions.push_str(video_tools::INSTRUCTIONS);
        extra_instructions.push_str(video_hyperframes::HYPERFRAMES_INSTRUCTIONS);
        tools.push(spark_preview_tool());
        // WHOAMI is already in the base tool list (added unconditionally above).
        // Here we only add the instruction that tells the model it exists.
        extra_instructions.push_str(
            "\n\nSELF-KNOWLEDGE: call the `whoami` tool to see your own name, model,              provider, and the full list of tools / connections / MCP servers / skills              you have — useful when the user asks who or what you are. It is read-only;              you can see your configuration but not change it.");
        // SPARKS — the embedded skill (data, not a tool): teach every agent how
        // to build an interactive mini-app on request. No new capability; it's a
        // way of using the existing jailed write_file. The Sparks tab renders
        // what the agent writes, in a sandboxed iframe.
        extra_instructions.push_str(SPARKS_INSTRUCTIONS);
    }

    // CONNECTION TOOLS, registry-driven. Every connected+enabled provider
    // contributes its tools from its descriptor — no per-provider code here.
    // Write tools appear ONLY when the agent's access_mode is 'write', so an
    // agent in read mode is never even offered a destructive call.
    if let Some((db, agent_id)) = conn_ctx {
        let mut connected: Vec<String> = Vec::new();
        for (provider, _legacy_access) in connections::enabled_providers_for_agent(db, agent_id) {
            let Some(def) = connectors::by_id(&provider) else { continue };
            // SINGLE SOURCE OF TRUTH: the per-tool off-list. `access_mode` used to
            // ALSO gate this, which meant a legacy row left at 'read' silently
            // withheld every write tool while the Connections screen showed them
            // all switched on. Two gates for one question always drift; the
            // switches are the control surface, so they are the only gate.
            let off = connections::disabled_tools(db, agent_id, &provider);
            let mut names: Vec<&str> = Vec::new();
            for t in def.tools_granted(true, &off) {
                tools.push(connectors::tool_schema(t));
                names.push(t.name);
            }
            if names.is_empty() { continue; }
            connected.push(format!("{}: {}", def.label, names.join(", ")));
        }
        if !connected.is_empty() {
            extra_instructions.push_str(&format!(
                "\n\nCONNECTED ACCOUNTS — you may call these tools to read or act on the \
                 user's real accounts:\n- {}\nThese hit live services on the user's behalf. \
                 If a tool reports it needs write access, tell the user to enable it in \
                 Connections rather than trying another route.",
                connected.join("\n- ")
            ));
        }
    }

    // EFFECTIVE FOLDER (2026-09-22): the UI's `folder` param is a legacy
    // contract — it can be None or stale (folder moved since). Browser + shell
    // gating must resolve the agent's LIVE folder from its profile (via
    // conn_ctx's db) instead of silently dropping to no-browsing / Folder Mode,
    // which left the model with no tool and narrating bash commands instead.
    let live_folder: String = match conn_ctx {
        Some((db, aid)) => crate::repo::get_agent(db, aid).ok().flatten()
            .map(|a| a.folder_path).filter(|f| !f.is_empty())
            .or_else(|| folder.map(String::from)).unwrap_or_default(),
        None => folder.unwrap_or_default().to_string(),
    };
    let eff_folder: Option<&str> = if live_folder.is_empty() { None } else { Some(&live_folder) };

    // MCP TOOLS: every enabled+running MCP server contributes its tools,
    // namespaced mcp__<server>__<tool>. Start enabled servers first so their
    // tool lists are known. App-wide (not folder-scoped).
    {
        mcp::ensure_enabled_running(app);
        let (mcp_schemas, mcp_instr) = mcp::agent_tool_schemas(app);
        for s in mcp_schemas { tools.push(s); }
        extra_instructions.push_str(&mcp_instr);
    }

    // BROWSER TOOLS (Slice 4): if the in-app browser is installed AND this agent
    // has at least one allowed browsing domain, expose the browser_* tools.
    // Fails closed — no allowed domains => no agent browsing.
    if browser::is_installed(app) {
        if let Some(f) = eff_folder {
            let domains = agent_browser_domains(app, f);
            if !domains.is_empty() {
                for schema in browser::agent_tool_schemas() { tools.push(schema); }
                extra_instructions.push_str(&format!(
                    "\n\nYou can browse the web in the SHARED in-app browser (the human watches live \
                     and can take over). Allowed domains: {}. Use browser_open to visit a page, \
                     browser_read to read it, browser_click_text / browser_type_text to interact. If \
                     you hit a login, CAPTCHA, or paywall, say so — the human will take the wheel.",
                    domains.join(", ")
                ));
            }
        }
    }

    // PRO MODE SHELL TOOLS (2026-07-31). Exposed ONLY when this folder's agent
    // has Pro Mode enabled (the scary-honest consent screen writes the flag).
    // Fails closed: no flag => no shell tools => Folder Mode (zero-shell). The
    // Rust exec broker ALSO cap-gates at the WS boundary, so this is the UX
    // gate; the broker is the authoritative one.
    if let Some(f) = eff_folder {
        if pro_mode_enabled(app, f) {
            for schema in shell_tool_schemas() { tools.push(schema); }
            extra_instructions.push_str(
                "\n\nPRO MODE: you can run shell commands, rooted in this folder. Use shell_run \
                 for one-shot commands (git pull, cargo build, npm run build, tsc) — it returns a \
                 bounded digest (exit code, error/warning counts, the key error lines, and a short \
                 tail), NOT the full log. For long-running processes (e.g. `cargo tauri dev`) use \
                 shell_spawn to start it, shell_poll to check its digest, shell_kill to stop it. \
                 Commands run from the folder root and cannot leave it. Read the digest's `signal` \
                 lines to find compiler errors; the full log is on disk if you need to grep it.",
            );
        }
    }

    if let (Ok(ad), Some(aid)) = (app_data(app), agent_id) {
        let scope = tools_registry::Scope::new(aid, eff_folder);
        for t in tools_registry::enabled_tools(&ad, &scope) {
            match t.kind.as_str() {
                "builtin" => {
                    if let Some(schema) = builtin_tool_schema(&t.name) { tools.push(schema); }
                }
                // A SKILL (stored as kind "composed" — the storage name predates
                // the rename; the UI calls these Skills). It is exposed as a
                // named tool the model invokes by following its saved
                // instructions using only the base tools it's allowed. A skill
                // grants no new capability: it's a way of working, not a
                // credential or a new reach.
                "composed" => {
                    tools.push(serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": { "type": "object", "properties": {
                            "request": { "type": "string", "description": "what the user wants this tool to do" }
                        } }
                    }));
                    extra_instructions.push_str(&format!(
                        "\n\nTool \"{}\": {}\nWhen using it, follow these instructions: {}\nYou may use these base tools to carry it out: {}.",
                        t.name, t.description, t.instructions, t.allowed_tools.join(", ")
                    ));
                }
                _ => {}
            }
        }
    }
    (serde_json::json!(tools), extra_instructions)
}

/// PRO MODE: is shell.exec enabled for this folder's agent? GUI-managed (no
/// config files) — the scary-honest consent screen writes a flag per folder,
/// same scheme as browser-policy. Fails closed (missing => false => Folder Mode).
pub fn pro_mode_enabled_pub(app: &tauri::AppHandle, folder: &str) -> bool { pro_mode_enabled(app, folder) }
fn pro_mode_enabled(app: &tauri::AppHandle, folder: &str) -> bool {
    let Ok(ad) = app_data(app) else { return false; };
    let path = ad.join("pro-mode").join(format!("{}.json", folder_key_fnv(folder)));
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
        .unwrap_or(false)
}

/// PRO MODE: the shell tool schemas exposed to the model when Pro Mode is on.
/// shell_run is the ergonomic 90% case; spawn/poll/write/kill drive long-lived
/// processes. Output is bounded by the exec broker (digest + tail), never raw.
fn shell_tool_schemas() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "shell_run",
            "description": "Run a one-shot shell command from the agent folder root and wait for it to finish. Best for git, cargo build, npm run build, tsc, etc. Returns a BOUNDED digest: exit_code, error/warning counts, the key error lines (`signal`), and a short output tail — not the full log. cwd is pinned to the folder; the command cannot escape it.",
            "input_schema": { "type": "object", "properties": {
                "program": { "type": "string", "description": "the executable, e.g. \"git\", \"cargo\", \"npm\"" },
                "args": { "type": "array", "items": { "type": "string" }, "description": "arguments, e.g. [\"build\"] or [\"pull\"]" },
                "timeout_ms": { "type": "number", "description": "max wait in ms (default 120000)" }
            }, "required": ["program"] }
        }),
        serde_json::json!({
            "name": "shell_spawn",
            "description": "Start a LONG-RUNNING process (e.g. `cargo tauri dev`, a dev server) from the folder root and return a proc_handle immediately. Use shell_poll to watch it, shell_kill to stop it. cwd is pinned to the folder.",
            "input_schema": { "type": "object", "properties": {
                "program": { "type": "string" },
                "args": { "type": "array", "items": { "type": "string" } }
            }, "required": ["program"] }
        }),
        serde_json::json!({
            "name": "shell_poll",
            "description": "Check a spawned process by its proc_handle. Returns whether it's still running, the exit_code if done, a running digest (error/warning counts, key `signal` lines), and a short output tail. Poll this to follow a long build without ingesting the whole log.",
            "input_schema": { "type": "object", "properties": {
                "proc_handle": { "type": "string" },
                "cursor": { "type": "number", "description": "only return output after this line index (optional)" }
            }, "required": ["proc_handle"] }
        }),
        serde_json::json!({
            "name": "shell_write",
            "description": "Write text to a running process's stdin (e.g. answer a prompt). Identify it by proc_handle.",
            "input_schema": { "type": "object", "properties": {
                "proc_handle": { "type": "string" },
                "data": { "type": "string" }
            }, "required": ["proc_handle", "data"] }
        }),
        serde_json::json!({
            "name": "shell_kill",
            "description": "Stop a running process by proc_handle. Sends SIGTERM by default; pass signal \"KILL\" to force.",
            "input_schema": { "type": "object", "properties": {
                "proc_handle": { "type": "string" },
                "signal": { "type": "string", "enum": ["TERM", "KILL"] }
            }, "required": ["proc_handle"] }
        }),
    ]
}

/// SLICE 4 — per-agent browser DOMAIN POLICY. The agent may only navigate to
/// hosts on this allowlist (fails closed: empty => no agent browsing). Stored
/// per-folder in app data (GUI-managed, no config files): 
///   <app_data>/browser-policy/<folderkey>.json -> ["example.com", ...]
/// The human tier is unrestricted (Slice 3) — this list ONLY gates the agent.
fn agent_browser_domains(app: &tauri::AppHandle, folder: &str) -> Vec<String> {
    let Ok(ad) = app_data(app) else { return vec![]; };
    let dir = ad.join("browser-policy");
    let key = folder_key_fnv(folder);
    let path = dir.join(format!("{key}.json"));
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
}

/// SHARED CONTEXT prompt block: tell the agent it has READ-ONLY access to other
/// folders and EXACTLY how to reach them (the @shared namespace). Without this,
/// an agent never knows a mount exists and never tries — which is why a
/// mounted folder was "invisible" (Mason 08-12). Assembled from the SAME
/// broker.shared_labels_for() the resolver uses, so the labels named here are
/// exactly the ones list_files/read_file accept. Empty when no mounts.
fn mounts_prompt_block(broker: &Arc<Broker>, agent_id: &str) -> String {
    let labels = broker.shared_labels_for(agent_id);
    if labels.is_empty() { return String::new(); }
    let listed = labels.iter().map(|l| format!("  - @shared/{l}/")).collect::<Vec<_>>().join("\n");
    format!(
        "\n\nSHARED CONTEXT (read-only): you have READ access to these other folders, \
         mounted under the virtual `@shared/` namespace:\n{listed}\n\
         - List what's shared: `list_files(\"@shared\")` shows the folder labels; \
         `list_files(\"@shared/<label>\")` browses inside one.\n\
         - Read a shared file: `read_file(\"@shared/<label>/path/to/file\")`.\n\
         - These folders are READ-ONLY — you cannot write or delete in them. To use \
         something you read there, copy it into your OWN folder."
    )
}

/// FNV-1a folder key (same scheme as tools/conversations).
fn folder_key_fnv(folder: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in folder.as_bytes() { hash ^= *b as u64; hash = hash.wrapping_mul(0x100000001b3); }
    format!("{hash:016x}")
}

/// Read the agent's browser allowlist (GUI).
#[tauri::command]
fn browser_policy_get(app: tauri::AppHandle, folder: String) -> Result<Vec<String>, String> {
    Ok(agent_browser_domains(&app, &folder))
}

/// Set the agent's browser allowlist (GUI).
#[tauri::command]
fn browser_policy_set(app: tauri::AppHandle, folder: String, domains: Vec<String>) -> Result<(), String> {
    let ad = app_data(&app)?;
    let dir = ad.join("browser-policy");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir browser-policy: {e}"))?;
    let path = dir.join(format!("{}.json", folder_key_fnv(&folder)));
    let clean: Vec<String> = domains.into_iter()
        .map(|d| d.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    let text = serde_json::to_string_pretty(&clean).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write policy: {e}"))
}

/// The PDF tool's per-folder config ({} if unavailable). Passed into exec so the
/// agent's generate_pdf calls honor the user's font/color/page settings.
fn pdf_config_for(app: &tauri::AppHandle, agent_id: Option<&str>, folder: Option<&str>) -> serde_json::Value {
    if let (Ok(ad), Some(aid)) = (app_data(app), agent_id) {
        let scope = tools_registry::Scope::new(aid, folder);
        tools_registry::tool_config(&ad, &scope, "builtin.pdf")
    } else {
        serde_json::json!({})
    }
}

// ── HYPERFRAMES (in-app video/graphics skill; Level A provisioning) ──────────
// One-click enable: AYGENT downloads a PORTABLE node + static ffmpeg + the
// hyperframes npm package into its OWN app-data space (nothing touches the
// system), then registers a Skill teaching the agent the render loop. The UI
// narrates every step. Remove reclaims the disk. See provision.rs.

/// The Skill id + the instructions the agent follows. Kept here so install can
/// (re)write it against the CURRENT provisioned paths.
const HYPERFRAMES_SKILL_ID: &str = "skill.hyperframes";

fn upsert_hyperframes_skill(app: &tauri::AppHandle) -> Result<(), String> {
    let ad = app_data(app)?;
    let (path_env, hf_bin) = provision::hyperframes_invocation(app)
        .ok_or("HyperFrames CLI not found after install")?;
    let instructions = format!(
        "You can create videos, animations, and motion graphics with HyperFrames — an          HTML-to-MP4 renderer. It is ALREADY installed inside AYGENT (portable Node + FFmpeg +          the hyperframes CLI); you do NOT install anything.

         HOW TO INVOKE (Pro Mode shell): always prefix the provisioned toolchain PATH so the          right node/ffmpeg are used, then call the hyperframes binary directly. Run commands with          shell_run like:
           program: \"bash\"
           args: [\"-lc\", \"export PATH='{path_env}':$PATH && '{hf_bin}' <hyperframes args>\"]

         PRODUCTION LOOP:
         1. Plan the video: scenes, timing, assets. Confirm the brief with the user first.
         2. Scaffold a project in the agent folder: `'{hf_bin}' init <name>` (creates an index.html             composition + project files under the agent folder).
         3. Author the composition by editing index.html with write_file: a #stage div with             data-composition-id/data-width/data-height, `class=\"clip\"` elements with             data-start/data-duration/data-track-index for video/text/audio, and seekable             animation (GSAP timeline assigned to window.__timelines.<id>, or CSS/WAAPI).
         4. Lint + preview: `'{hf_bin}' lint` then (optional) `'{hf_bin}' preview`.
         5. Render to MP4: `'{hf_bin}' render` — the output lands in the project dir.

         RULES: keep compositions deterministic (seekable animation, not wall-clock). Put all          media + output inside the agent folder. Report the final MP4 path when done. If a render          fails, read the hyperframes error (it names the offending clip/attribute) and fix the HTML.

         Provisioned toolchain PATH: {path_env}
         HyperFrames CLI: {hf_bin}"
    );
    let tool = tools_registry::ToolDef {
        id: HYPERFRAMES_SKILL_ID.to_string(),
        name: "hyperframes".to_string(),
        display_name: "HyperFrames — video & motion graphics".to_string(),
        description: "Create videos, animations, and motion graphics from HTML (installed in-app; renders to MP4).".to_string(),
        kind: "composed".to_string(),
        builtin: false,
        instructions,
        allowed_tools: vec![
            "read_file".into(), "write_file".into(), "list_files".into(),
            "shell_run".into(), "shell_spawn".into(), "shell_poll".into(),
        ],
    };
    tools_registry::upsert_tool(&ad, tool)
}

/// Status of the HyperFrames feature: is it fully provisioned + where it lives.
#[tauri::command]
fn hyperframes_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let installed = provision::hyperframes_installed(&app);
    let rt = provision::runtime_dir(&app).ok().map(|p| p.to_string_lossy().to_string());
    Ok(serde_json::json!({ "installed": installed, "runtime_dir": rt }))
}

/// One-click ENABLE: provision node+ffmpeg+hyperframes (narrated on `channel`),
/// register the Skill, AND turn it ON for the acting agent. Without that last
/// step the skill exists in the registry but its per-agent toggle defaults OFF
/// (composed skills default off), so the agent never receives it — the "enabled
/// but not found" bug (Mason 08-06). Everything lands in AYGENT's app-data.
#[tauri::command]
async fn hyperframes_provision(
    app: tauri::AppHandle,
    db: tauri::State<'_, writer::Db>,
    channel: String,
    agent_id: Option<String>,
    folder: Option<String>,
) -> Result<serde_json::Value, String> {
    let report = provision::hyperframes_install(&app, &channel).await?;
    upsert_hyperframes_skill(&app)?;
    // Enable the skill for the acting agent so the model actually gets it. Resolve
    // the agent: explicit id → owner of `folder` → active agent.
    let aid: String = agent_id.filter(|s| !s.trim().is_empty())
        .or_else(|| folder.as_deref().and_then(|f| agent_for_folder(&db, f).ok()))
        .or_else(|| repo::active_id(&db).ok())
        .unwrap_or_default();
    if !aid.is_empty() {
        let ad = app_data(&app)?;
        let scope = tools_registry::Scope::new(&aid, folder.as_deref());
        tools_registry::set_enabled(&ad, &scope, HYPERFRAMES_SKILL_ID, true)?;
    }
    Ok(report)
}

/// DISABLE: remove the HyperFrames Skill + its files. `keep_toolchain=true`
/// leaves the shared node/ffmpeg (for other features); false reclaims all disk.
#[tauri::command]
fn hyperframes_remove(app: tauri::AppHandle, keep_toolchain: Option<bool>) -> Result<serde_json::Value, String> {
    let ad = app_data(&app)?;
    let _ = tools_registry::delete_tool(&ad, HYPERFRAMES_SKILL_ID);
    provision::hyperframes_uninstall(&app, keep_toolchain.unwrap_or(false))
}

// ── MCP servers (Connections) ────────────────────────────────────────────────

/// List all MCP servers (built-in catalog merged with stored state + custom),
/// annotated with whether each is currently running.
#[tauri::command]
fn mcp_list(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let out: Vec<serde_json::Value> = mcp::list(&app).into_iter().map(|c| {
        serde_json::json!({
            "key": c.key, "label": c.label, "command": c.command, "args": c.args,
            "enabled": c.enabled, "builtin": c.builtin, "needs_node": c.needs_node, "needs_uv": c.needs_uv,
            "setup_note": c.setup_note, "verify_tool": c.verify_tool,
            "running": mcp_client::is_running(&c.key),
            "tool_count": mcp_client::get(&c.key).map(|s| s.tools().len()).unwrap_or(0),
        })
    }).collect();
    Ok(serde_json::json!(out))
}

/// The steps enabling a server will run (so the UI can narrate before doing it).
#[tauri::command]
fn mcp_install_plan(app: tauri::AppHandle, key: String) -> Result<serde_json::Value, String> {
    let cfg = mcp::get_config(&app, &key).ok_or_else(|| format!("unknown MCP server `{key}`"))?;
    let steps: Vec<serde_json::Value> = mcp::install_plan(&cfg).into_iter()
        .map(|(label, program, args)| serde_json::json!({ "label": label, "cmd": format!("{program} {}", args.join(" ")) }))
        .collect();
    Ok(serde_json::json!({ "key": key, "label": cfg.label, "setup_note": cfg.setup_note, "steps": steps, "needs_node": cfg.needs_node }))
}

/// ENABLE an MCP server: ensure Node (if needed), run its install plan (narrated
/// on `channel`), mark it enabled + persist, then start it (handshake). Returns
/// the running tool count + the verify tool (if any) so the UI can prompt the
/// user for the manual in-app step (e.g. Premiere's Start Bridge).
#[tauri::command]
async fn mcp_enable(app: tauri::AppHandle, channel: String, key: String) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    let mut cfg = mcp::get_config(&app, &key).ok_or_else(|| format!("unknown MCP server `{key}`"))?;
    if cfg.needs_node {
        let _ = app.emit(&channel, &serde_json::json!({ "phase": "node", "note": "Preparing the toolchain (Node)…" }));
        crate::provision::ensure_node(&app, &channel).await?;
    }
    if cfg.needs_uv {
        let _ = app.emit(&channel, &serde_json::json!({ "phase": "uv", "note": "Preparing the toolchain (uv + Python)…" }));
        crate::provision::ensure_uv(&app, &channel).await?;
    }
    let plan = mcp::install_plan(&cfg);
    if !plan.is_empty() {
        mcp::run_plan(&app, &channel, &plan).await?;
    }
    cfg.enabled = true;
    mcp::upsert(&app, cfg.clone())?;
    let _ = app.emit(&channel, &serde_json::json!({ "phase": "starting", "note": "Starting the server…" }));
    let server = mcp::start_server(&app, &key)?;
    let _ = app.emit(&channel, &serde_json::json!({ "phase": "done", "note": "Connected." }));
    Ok(serde_json::json!({
        "enabled": true, "running": true, "tool_count": server.tools().len(),
        "verify_tool": cfg.verify_tool, "setup_note": cfg.setup_note,
    }))
}

/// DISABLE an MCP server: stop it, mark disabled. `uninstall=true` also runs the
/// uninstall plan (npm remove etc.) so nothing is left on the machine.
#[tauri::command]
async fn mcp_disable(app: tauri::AppHandle, channel: Option<String>, key: String, uninstall: Option<bool>) -> Result<serde_json::Value, String> {
    mcp_client::stop(&key);
    let mut cfg = mcp::get_config(&app, &key).ok_or_else(|| format!("unknown MCP server `{key}`"))?;
    cfg.enabled = false;
    mcp::upsert(&app, cfg.clone())?;
    let mut report = serde_json::json!({ "disabled": true });
    if uninstall.unwrap_or(false) {
        let plan = mcp::uninstall_plan(&cfg);
        if !plan.is_empty() {
            let ch = channel.unwrap_or_default();
            let r = mcp::run_plan(&app, &ch, &plan).await?;
            report["uninstalled"] = serde_json::json!(r);
        }
        // Python (uv) servers: reclaim the provisioned uv toolchain + managed
        // Python + package cache. (Shared across uv servers — fine today since
        // Blender is the only one; revisit if we add a second uv server.)
        if cfg.needs_uv {
            let _ = crate::provision::uv_uninstall(&app);
            report["uv_removed"] = serde_json::json!(true);
        }
    }
    Ok(report)
}

/// Verify a running server via its declared read-only verify tool.
#[tauri::command]
fn mcp_verify(app: tauri::AppHandle, key: String) -> Result<String, String> {
    mcp::verify(&app, &key)
}

/// Add a user MCP server from the web (command + args + env pairs).
#[tauri::command]
fn mcp_add_custom(app: tauri::AppHandle, label: String, command: String, args: Vec<String>, env: Vec<(String, String)>, needs_node: Option<bool>) -> Result<String, String> {
    mcp::add_custom(&app, &label, &command, args, env, needs_node.unwrap_or(true))
}

/// Remove a user-added MCP server (built-ins can only be disabled).
#[tauri::command]
fn mcp_remove_custom(app: tauri::AppHandle, key: String) -> Result<(), String> {
    mcp::remove_custom(&app, &key)
}

const SPARKS_INSTRUCTIONS: &str = "\n\n\
SPARKS \u{2014} interactive mini-apps you build RIGHT IN THE CHAT with the `spark_preview` \
tool. A Spark is a small self-contained web app (calculator, chart, tool, game).\n\
\n\
BEFORE YOU BUILD, PLAN THE UI (do this every time, silently):\n\
1. What is the ONE main thing the user does here? Make that the biggest, most obvious \
control.\n\
2. What is the ONE main result they want? Show it LARGE and live-updating (a .stat).\n\
3. Choose the simplest control for each input (see the recipe below). Group everything \
into ONE .card. Order it top-to-bottom the way a person actually uses it: inputs first, \
result last and prominent.\n\
Aim for something that looks like a polished little iOS-style utility \u{2014} generous \
spacing, one clear primary result, no wall of tiny text.\n\
\n\
STYLING IS HANDLED FOR YOU \u{2014} do NOT write <style>, fonts, colors, or <html>/<head>/<body>. \
Write ONLY the inner body markup with these classes; AYGENT injects the full design system \
(system font, dark mode, styled controls). Any <style> you write is STRIPPED, so styling \
it yourself is wasted effort.\n\
\n\
LAYOUT RECIPE (compose from these \u{2014} they are pre-styled):\n\
- Shell: <h1>Name</h1><p class=\"sub\">one line</p> then ONE <div class=\"card\">\u{2026}</div>.\n\
- A field: <label>Bill amount</label> then its control.\n\
- Money input: <div class=\"input-money\"><span>$</span><input id=\"bill\" type=\"number\" inputmode=\"decimal\" placeholder=\"0.00\"></div>.\n\
- A pick-one set (tip %, options): <div class=\"seg\"><button type=\"button\">10%</button><button type=\"button\" class=\"active\">15%</button><button type=\"button\">20%</button></div> \u{2014} exactly ONE has class active; in JS, on click move the active class and recompute.\n\
- A count (+/\u{2212}): <div class=\"stepper\"><button type=\"button\">\u{2212}</button><span class=\"val\" id=\"n\">1</span><button type=\"button\">+</button></div>.\n\
Use type=\"button\" on EVERY button so it never submits a form. In JS guard every getElementById: if(el) el.addEventListener(...).\n\nJS SAFETY (Sparks run sandboxed \u{2014} no console): null.addEventListener throws kill the whole script and buttons appear dead. Never call getElementById(...).addEventListener without a null check.\n\n- A big live result: <div class=\"stat\" id=\"total\">$0.00</div> \u{2014} use this for the primary output.\n\
- Secondary results: <div class=\"row\"><span class=\"k\">Per person</span><span class=\"v\" id=\"pp\">$0.00</span></div> (label left, value right \u{2014} NEVER put label and value adjacent in plain text).\n\
- Side-by-side metrics: <div class=\"grid\">\u{2026}</div>. Tables: plain <table>.\n\
Put ALL logic in one <script> at the end: read inputs, wire addEventListener, update result \
elements by id, and compute on every change so the result is always live.\n\
\n\
EXAMPLE \u{2014} a tip calculator's body (follow this shape, adapt the fields):\n\
<h1>Tip Calculator</h1><p class=\"sub\">Split the bill, no mental math.</p>\
<div class=\"card\">\
<label>Bill amount</label><div class=\"input-money\"><span>$</span><input id=\"bill\" type=\"number\" inputmode=\"decimal\" placeholder=\"0.00\"></div>\
<label>Tip</label><div class=\"seg\"><button type=\"button\">10%</button><button type=\"button\" class=\"active\">15%</button><button type=\"button\">20%</button></div>\
<label>Split between</label><div class=\"stepper\"><button type=\"button\" id=\"dec\">\u{2212}</button><span class=\"val\" id=\"n\">1</span><button type=\"button\" id=\"inc\">+</button></div>\
<div class=\"stat\" id=\"total\" style=\"margin-top:14px\">$0.00</div>\
<div class=\"row\"><span class=\"k\">Tip</span><span class=\"v\" id=\"tip\">$0.00</span></div>\
<div class=\"row\"><span class=\"k\">Per person</span><span class=\"v\" id=\"pp\">$0.00</span></div>\
</div><script>/* wire it up: recompute on input + seg/stepper clicks */</script>\n\
\n\
PERSISTENCE: a Spark's state IS saved \u{2014} localStorage works normally AND persists across \
sessions (checklists stay checked, counters keep counting when the user leaves and comes back). \
For structured data use window.spark.set(key,value) / window.spark.get(key) / window.spark.all() \
(any JSON value). No setup needed \u{2014} great for to-do lists, trackers, saved settings.\n\
\
DATA AT BUILD TIME: the Spark is sandboxed \u{2014} it CANNOT call you or read files. If it needs \
the user's real data, gather it FIRST with your tools, then embed it in the <script> as a \
JS literal (const DATA = {\u{2026}}). Never put secrets in a Spark.\n\
\n\
FLOW: call spark_preview({slug, title, html}) \u{2014} it renders live inline in chat. Iterate by \
calling again with the SAME slug (it hot-swaps). The user clicks Save to Library when happy. \
For charts, add <script src=\"https://cdn.jsdelivr.net/npm/chart.js\"></script> before your script.";

const AGENT_SYSTEM: &str = "You are AYGENT, a helpful, concise, friendly assistant running privately \
    on the user's own machine. You have TOOLS available but they are OPTIONAL — use a tool ONLY when \
    the user's request actually requires it. If the user is just chatting, sharing information, or \
    asking a question you can answer directly, JUST REPLY — do not call any tools, do not read or list \
    files, do not call an API. Never explore the folder or call tools speculatively 'to gather \
    context'; act only on what was asked. When a task DOES need a tool: use read_file/write_file/\
    list_files for files in the user's chosen folder (you cannot run shell commands). To RENAME or \
    MOVE a file use rename_file (NEVER write a copy under the new name and leave the old file — \
    rename it); to remove a file use delete_file. For content too large for one write, use write_file for the first chunk then append_file for the rest. Use web_search to search the web, fetch_url to \
    read a web page/API over HTTPS, and any connected-service tools (e.g. github_list_prs) for \
    that service. Prefer the smallest number of tool calls that gets the job done.";

/// Appended to the system prompt on the Muse (meta) provider only.
const MUSE_QUIET_TOOLS: &str = "\n\nTOOL-CALL STYLE: do NOT write a sentence restating or re-affirming the user's request \
    before a tool call (no \"Pulling X now…\", \"Let me check Y…\", \"Got it — doing Z\"). Call the tool \
    silently. Write text only when you have something to report: a result, a decision, a question, \
    or the final answer. One reply at the end beats a status line before every call.";

// Base system prompt for local models. When the model is tool-capable, we
// APPEND its family-native tool instructions (local_tools::system_prompt_with_tools).
const AGENT_SYSTEM_LOCAL: &str = "You are AYGENT, a helpful AI assistant running privately \
    on the user's own machine. Be concise and friendly.";

/// Max tool-calling turns for LOCAL models. Anthropic uses 8; local models are
/// slower and can loop unproductively, so we cap at 4 (Mason's call).
const LOCAL_TOOL_TURN_CAP: usize = 4;

/// STREAMING chat turn. `history` is the running conversation (array of
/// {role, content}); we append the new user prompt, run the agent loop with
/// streaming, emit events to the UI, and return the FULL updated history so the
/// UI can persist multi-turn memory. Events are emitted on channel `agent://<id>`.
#[tauri::command]
async fn agent_stream(
    app: tauri::AppHandle,
    broker: tauri::State<'_, Arc<Broker>>,
    lanes: tauri::State<'_, lanes::Lanes>,
    db: tauri::State<'_, writer::Db>,
    drain: tauri::State<'_, drainer::DrainSignal>,
    browser_state: tauri::State<'_, browser::BrowserProc>,
    cancel_reg: tauri::State<'_, cancel::CancelRegistry>,
    channel: String,
    prompt: String,
    history: serde_json::Value,
    model: Option<String>,
    provider: Option<String>,
    folder: Option<String>,
    session_id: Option<String>,
    agent_id: Option<String>,
    attachments: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    let broker = broker.inner().clone();
    let provider_kind = provider.unwrap_or_default();

    // STOP BUTTON: register this turn's cancel flag under its event channel.
    // The guard's Drop removes the entry on ANY exit path (this fn has many:
    // early `?`, explicit `return Ok`/`return Err`, natural fall-through at the
    // bottom of each provider branch) so a finished turn never leaves a stale
    // flag behind to falsely cancel a later turn reusing the same channel.
    let (_cancel_guard, cancel_flag) = cancel::CancelGuard::new(cancel_reg.inner().clone(), channel.clone());

    // ---- PER-SESSION LANE (M1.1) -------------------------------------------
    // Serialize turns for THIS session: if another turn is already running on
    // it, we wait here until it finishes. One turn at a time per session kills
    // tool/session races (double writes, torn streams, double SAVE POINTs) at
    // the source. The lane key is the session id when the UI supplies one, else
    // the per-conversation event channel (also unique per conversation). The
    // guard is held for the whole turn — dropped automatically on return.
    let lane_key = session_id.clone().unwrap_or_else(|| channel.clone());
    let _lane = lanes.acquire(&lane_key).await;

    // ---- WHICH AGENT IS THIS? (M1.4) ---------------------------------------
    // Resolve the acting agent's real id. Every file op + SAVE POINT below jails
    // to THIS agent's own broker scope (not a shared "default"), so concurrent
    // agents stay confined to their own folders. Precedence: explicit agent_id
    // from the UI → the agent that owns `folder` → the active agent → "default"
    // (legacy fallback so a pre-M1.4 caller still works).
    let scope_id: String = {
        if let Some(id) = agent_id.clone().filter(|s| !s.trim().is_empty()) {
            id
        } else if let Some(f) = folder.clone().filter(|s| !s.is_empty()) {
            agent_for_folder(&db, &f).unwrap_or_else(|_| "default".to_string())
        } else {
            repo::active_id(&db).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "default".to_string())
        }
    };
    // If the agent has a folder but its scope isn't registered yet (e.g. created
    // this session), register it now so the jail is live for this turn.
    if let Ok(Some(fp)) = repo::folder_for(&db, &scope_id) {
        let p = std::path::PathBuf::from(&fp);
        if p.is_dir() {
            let canonical = std::fs::canonicalize(&p).unwrap_or(p);
            broker.set_scope(&scope_id, canonical, false);
        }
    }
    // The agent's own system prompt (personality/values) — prepended to the base
    // AYGENT instructions below. This is what makes agents feel DISTINCT.
    let agent_persona: String = repo::get_agent(&db, &scope_id)
        .ok().flatten().map(|a| a.system_prompt).unwrap_or_default();
    // M1.4 #4: the agent's uploaded context documents, as a capped prepend block
    // ("" if none). Bridge until M1.7 embedding retrieval (chunks already stored).
    let context_block: String = context_docs::prepend_block(&db, &scope_id).unwrap_or_default();
    // M1.4 #7: the roster of OTHER agents this agent can message, so its
    // send_message tool knows valid recipients and it's AWARE of its peers.
    let roster: Vec<(String, String)> = mailbox::roster(&db, &scope_id).unwrap_or_default();
    let roster_block: String = if roster.is_empty() { String::new() } else {
        let list = roster.iter().map(|(id, name)| format!("- {name} (id: {id})")).collect::<Vec<_>>().join("\n");
        format!("\n\nOTHER AGENTS you can message with the send_message tool (async — they reply on their own time):\n{list}")
    };
    // The full extra system block appended to AGENT_SYSTEM: persona + peers +
    // context docs. Assembled once, used by all three provider paths.
    let extra_block: String = {
        let mut s = String::new();
        if !agent_persona.trim().is_empty() { s.push_str("\n\n"); s.push_str(agent_persona.trim()); }
        s.push_str(&roster_block);
        s.push_str(&context_block);
        s.push_str(&mounts_prompt_block(&broker, &scope_id));
        s
    };

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

        // Context window = the MODEL's real capability, capped to what THIS Mac
        // can hold. A model may advertise 128k tokens, but the KV cache grows
        // with context and would eat gigabytes of RAM at full size — so we pick
        // the model's window but never exceed a memory-safe budget. This makes
        // "context matches the model" true for a non-technical user, while it
        // still just-works within their hardware.
        let ctx_tokens = local_context_budget(&path);

        // Detect this model's native tool capability from its GGUF chat template
        // (detect, don't guess). Tool-capable => run the tool loop; else chat-only.
        let cap = gguf::detect_tool_capability(&path);
        let ctx_note = format!("{}k context", ctx_tokens / 1024);

        if !cap.tools_supported {
            // CHAT-ONLY model: single turn, no tools (honest — its template
            // never declared tool support).
            let _ = app.emit(&channel, &provider::StreamEvent::Info {
                text: format!("local model · chat only · {ctx_note}"),
            });
            let (content, _stop) = local_provider::local_stream_turn(
                &path, AGENT_SYSTEM_LOCAL, &messages, ctx_tokens,
                |ev| { let _ = app.emit(&channel, &ev); },
            ).await?;
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "assistant", "content": content
            }));
            return Ok(messages);
        }

        // TOOL-CAPABLE model: build the family-native system prompt + run a
        // bounded tool loop (cap at LOCAL_TOOL_TURN_CAP).
        let _ = app.emit(&channel, &provider::StreamEvent::Info {
            text: format!("local model · {} tools · {ctx_note}", cap.format),
        });
        // Baseline SAVE POINT before any tool writes (same as Anthropic path).
        if let Ok(root) = broker.root_for(&scope_id) {
            let _ = savepoint::snapshot(&root, "Checkpoint");
        }
        // Enabled registry tools (e.g. PDF) contribute extra instructions the
        // local model should know about, appended to its native tool prompt.
        let (_reg_tools, reg_instr) = agent_tools_for(&app, Some(&scope_id), folder.as_deref());
        let pdf_cfg = pdf_config_for(&app, Some(&scope_id), folder.as_deref());
        // AUTO-INJECT MEMORY (RAG, Mason 08-19): retrieve notes relevant to THIS
        // prompt and put them straight into the system block. Small local models
        // are unreliable at *choosing* to call a memory tool — this way relevant
        // memory is simply present, no call required. The recall tool remains for
        // explicit searches ("read your memory about X"). Failures are silent:
        // memory is an enhancement, never a blocker for the turn.
        let memory_block: String = match ensure_embed_model(&app).await {
            Ok(em) => match memory::retrieve(&db, "agent", &scope_id, &prompt, 3, 1, &em, "").await {
                Ok(notes) if !notes.is_empty() => {
                    let mut b = String::from("\n\nRelevant notes from your long-term memory (use them if they help):\n");
                    for n in &notes {
                        b.push_str(&format!("- [{}] {}: {}\n", n.ntype, n.title, n.snippet));
                    }
                    b
                }
                _ => String::new(),
            },
            Err(_) => String::new(),
        };
        let base_sys = format!("{AGENT_SYSTEM_LOCAL}{extra_block}{reg_instr}{memory_block}");
        let sys = local_tools::system_prompt_with_tools(&base_sys, &cap.format);

        for turn in 0..LOCAL_TOOL_TURN_CAP {
            let (content, _stop) = local_provider::local_stream_turn(
                &path, &sys, &messages, ctx_tokens,
                |ev| { let _ = app.emit(&channel, &ev); },
            ).await?;
            // The assistant's raw text (content is [{type:text,text:...}]).
            let text = content.as_array()
                .and_then(|a| a.first())
                .and_then(|b| b.get("text")).and_then(|t| t.as_str())
                .unwrap_or("").to_string();
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "assistant", "content": content
            }));

            // Parse tool calls in the model's native format — from the VISIBLE
            // text only: reasoning models (Qwen3) muse about hypothetical calls
            // inside <think> blocks; those must never execute.
            let visible = local_tools::strip_think(&text);
            let mut calls = local_tools::parse_tool_calls(&visible, &cap.format);
            if calls.is_empty() {
                // Qwen3 sometimes emits the call INSIDE an unclosed <think>
                // ("memory call stuffed in a thought", Mason 08-19) — rescue it.
                calls = local_tools::rescue_call_from_unclosed_think(&text, &cap.format);
            }
            if calls.is_empty() {
                // The model TRIED to call a tool but we couldn't parse it
                // (malformed JSON, raw newlines in strings...). Silently
                // breaking here made the write "look emitted but never run"
                // (Mason 08-19, bug #2b) — instead, tell the model what went
                // wrong so it can retry within the turn cap.
                let tried = local_tools::has_tool_marker(&visible, &cap.format)
                    || local_tools::unclosed_think_tail(&text)
                        .is_some_and(|t| local_tools::has_tool_marker(t, &cap.format));
                if tried && turn + 1 < LOCAL_TOOL_TURN_CAP {
                    let _ = app.emit(&channel, &provider::StreamEvent::Info {
                        text: "tool call couldn't be parsed — asking the model to retry".to_string(),
                    });
                    messages.as_array_mut().unwrap().push(serde_json::json!({
                        "role": "user",
                        "content": local_tools::format_tool_result(&cap.format, "parser",
                            "Your tool call could not be parsed. Emit EXACTLY one tool call with valid single-line JSON: {\"name\": \"write_file\", \"arguments\": {\"path\": \"...\", \"content\": \"...\"}} — escape newlines in strings as \\n, and put path/content directly inside \"arguments\" (do not nest another \"arguments\" object).", true),
                    }));
                    continue;
                }
                break; // no tool wanted → done
            }

            // Execute each call through the SAME jailed broker + emit UI events.
            // Local calls have no provider tool_use id — synthesize one so the
            // ToolResult binds to its exact card (parity with cloud paths).
            let mut results_text = String::new();
            for (ci, c) in calls.iter().enumerate() {
                let call_id = format!("local-{turn}-{ci}");
                let _ = app.emit(&channel, &serde_json::json!({
                    "kind": "ToolUse", "id": call_id, "name": c.name,
                    "input": c.input,
                }));
                let (result, is_err) = if c.name == "whoami" {
                    // SELF-INTROSPECTION (parity with the cloud paths): read-only
                    // identity + model + capabilities. No jail/broker call needed.
                    (introspect::build_whoami(&app, &db, &scope_id, folder.as_deref()), false)
                } else if c.name == "recall" {
                    // MEMORY SEARCH (Mason 08-19): semantic retrieval over the
                    // agent's vault — the read path local models were missing
                    // ("read your memory" used to have no correct tool).
                    let q = c.input.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if q.is_empty() {
                        ("recall needs a \"query\" argument — what should I search my memory for?".to_string(), true)
                    } else {
                        match ensure_embed_model(&app).await {
                            Err(e) => (format!("memory unavailable: {e}"), true),
                            Ok(em) => match memory::retrieve(&db, "agent", &scope_id, &q, 4, 1, &em, "").await {
                                Err(e) => (format!("memory search failed: {e}"), true),
                                Ok(notes) if notes.is_empty() => (format!("No memories found for \"{q}\"."), false),
                                Ok(notes) => {
                                    let mut r = String::new();
                                    for n in &notes {
                                        r.push_str(&format!("[{}] {} — {}\n", n.ntype, n.title, n.snippet));
                                    }
                                    (r, false)
                                }
                            },
                        }
                    }
                } else if video_tools::is_video_tool(&c.name) {
                    video_tools::exec(&broker, &scope_id, &c.name, &c.input)
                } else if dashboard::is_dashboard_tool(&c.name) {
                    dashboard::exec_dashboard_tool(&db, &scope_id, &c.name, &c.input)
                } else {
                    exec_tool_cfg(&broker, &scope_id, &c.name, &c.input, &pdf_cfg)
                };
                let path_s = c.input.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                let _ = app.emit(&channel, &serde_json::json!({
                    "kind": "ToolResult", "id": call_id, "name": c.name, "path": path_s,
                    "ok": !is_err,
                    // Bounded output preview on success too (was error-only),
                    // same contract as the cloud providers' activity cards.
                    "detail": if is_err { result.clone() } else { result.chars().take(2000).collect::<String>() }
                }));
                results_text.push_str(&local_tools::format_tool_result(&cap.format, &c.name, &result, is_err));
                results_text.push('\n');
            }

            // Feed results back as a user turn and loop.
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "user", "content": results_text
            }));

            if turn == LOCAL_TOOL_TURN_CAP - 1 {
                let _ = app.emit(&channel, &provider::StreamEvent::Info {
                    text: format!("stopped after {LOCAL_TOOL_TURN_CAP} tool steps"),
                });
            }
        }

        // Snapshot AFTER the turn's writes, labeled with the prompt (same C4
        // semantics as the Anthropic path — rewindable local tool edits).
        if let Ok(root) = broker.root_for(&scope_id) {
            let _ = savepoint::snapshot(&root, &prompt);
        }
        run_auto_capture(&app, &db, &broker, &scope_id, &prompt, &channel).await;
        return Ok(messages);
    }

        // ---- MLX MODEL PATH (Apple Silicon, mlx-community/* via sidecar) ---------
    // `model` here is the Hugging Face repo id (e.g. mlx-community/Qwen3-4B-4bit).
    // v1 is a SINGLE chat turn (no tools) — same honest shape as a chat-only GGUF.
    // The sidecar (mlx_lm.server on localhost) auto-downloads weights on first
    // boot into the in-app HF cache; switching repos restarts the server.
    if provider_kind == "mlx" {
        let repo = model.filter(|m| !m.trim().is_empty())
            .ok_or_else(|| "no MLX model selected — pull one in Settings > Local Models".to_string())?;
        let mut messages = if history.is_array() { history } else { serde_json::json!([]) };
        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": prompt }));
        let _ = app.emit(&channel, &provider::StreamEvent::Info {
            text: format!("mlx · {repo}"),
        });
        // System prompt rides as leading context (mlx.rs flattens it into the
        // OpenAI message list); stored history keeps just the user prompt.
        let mut send = serde_json::json!([{ "role": "user", "content": AGENT_SYSTEM_LOCAL }]);
        for m in messages.as_array().unwrap() { send.as_array_mut().unwrap().push(m.clone()); }
        let content = mlx::mlx_stream_turn(
            &app, &channel, &repo, &send, 2048,
            |ev| { let _ = app.emit(&channel, &ev); },
        ).await?;
        messages.as_array_mut().unwrap().push(serde_json::json!({
            "role": "assistant", "content": content
        }));
        run_auto_capture(&app, &db, &broker, &scope_id, &prompt, &channel).await;
        return Ok(messages);
    }

    // ---- OPENAI / OPENROUTER PATH ------------------------------------------
    // Shared Chat Completions wire format; one impl, two base URLs. Full tool-
    // use: same jailed exec_tool + broker + SAVE POINTs as every other provider.
    if provider_kind == "openai" || provider_kind == "openrouter" || provider_kind == "meta" {
        let key = keychain::get_key(&provider_kind)
            .map_err(|_| format!("no {provider_kind} key set — add one in Settings"))?;
        let model = model.filter(|m| !m.trim().is_empty())
            .ok_or_else(|| format!("no {provider_kind} model selected — pick one in Settings"))?;
        // MODEL VARIANT: the agent's reasoning knob (Muse Spark effort etc.).
        // Resolved from the profile so every caller (chat, video dock) gets it
        // without changing the turn args. Soul/compact/planner use complete()
        // (default effort) deliberately — short utility calls, keep them cheap.
        let variant: String = repo::get_agent(&db, &scope_id).ok().flatten().map(|a| a.model_variant).unwrap_or_default();
        let variant_opt = if variant.trim().is_empty() { None } else { Some(variant.as_str()) };

        // Baseline SAVE POINT before the turn (rewind anchor), same as Anthropic.
        if let Ok(root) = broker.root_for(&scope_id) {
            let _ = savepoint::snapshot(&root, "Checkpoint");
        }

        let (tools, reg_instr) = agent_tools_for_full(&app, Some(&scope_id), folder.as_deref(), !roster.is_empty(), Some((&db, &scope_id)));
        let pdf_cfg = pdf_config_for(&app, Some(&scope_id), folder.as_deref());
        // MUSE SPARK (Mason 09-04): the model narrates a one-line restatement of
        // the user's intent before EVERY function call ("Pulling PR #3328 — let me
        // locate that branch…"). Nothing on our side asked for it; it's the model's
        // habit. Tell it plainly not to, on this provider only.
        let muse_quiet = if provider_kind == "meta" { MUSE_QUIET_TOOLS } else { "" };
        let sys = format!("{AGENT_SYSTEM}{extra_block}{reg_instr}{muse_quiet}");
        let mut messages = if history.is_array() { history } else { serde_json::json!([]) };
        // ATTACHMENTS (bug fix — these were SILENTLY DROPPED on OpenAI/OpenRouter:
        // the Anthropic branch built real content blocks from `attachments`, this
        // branch never even looked at the parameter). Same OpenAI vision wire shape
        // (image_url data: URLs) as any other OpenAI-compatible call.
        let att = attachments.clone().unwrap_or_default();
        if att.is_empty() {
            messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": prompt }));
        } else {
            let mut content = vec![serde_json::json!({ "type": "text", "text": prompt })];
            content.extend(attachment_blocks_openai(&broker, &scope_id, &att));
            messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": content }));
        }
        let _ = app.emit(&channel, &provider::StreamEvent::Info { text: if variant.trim().is_empty() { format!("model: {model}") } else { format!("model: {model} · {variant}") } });

        // PRO MODE UNCAPPED (Mason 08-03): same policy as the Anthropic path —
        // no round cap in Pro Mode; the stall detector replaces it. Non-Pro
        // keeps the 20-round cap.
        let pro_uncapped = folder.as_deref().map(|f| pro_mode_enabled(&app, f)).unwrap_or(false);
        let max_rounds: usize = if pro_uncapped { usize::MAX } else { 20 };
        let mut rounds: usize = 0;
        let mut tool_calls_total: usize = 0;
        let mut last_action: String = String::new();
        let mut last_sig: String = String::new();
        let mut same_sig_streak: usize = 0;
        let mut err_round_streak: usize = 0;
        let mut stall_reason: Option<String> = None;
        let mut placeholder_streak: usize = 0;
        let mut finished_naturally = false;
        while rounds < max_rounds {
            rounds += 1;
            if pro_uncapped && rounds > 1 && rounds % 25 == 1 {
                let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("still working — {tool_calls_total} tool calls so far") });
            }
            if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                messages.as_array_mut().unwrap().push(serde_json::json!({
                    "role": "assistant", "content": "⏹️ stopped by user"
                }));
                finished_naturally = true;
                break;
            }
            let mut round_calls: usize = 0;
            let mut round_errs: usize = 0;
            let stream_result = if provider_kind == "meta" {
                meta_provider::meta_stream_turn(
                    &key, &model, variant_opt, &sys, &messages, &tools, Some(&cancel_flag),
                    |ev| { let _ = app.emit(&channel, &ev); },
                ).await
            } else {
                openai_provider::openai_stream_turn(
                    &provider_kind, &key, &model, variant_opt, &sys, &messages, &tools, Some(&cancel_flag),
                    |ev| { let _ = app.emit(&channel, &ev); },
                ).await
            };
            if let Err(e) = &stream_result {
                if e == "__CANCELLED__" {
                    messages.as_array_mut().unwrap().push(serde_json::json!({
                        "role": "assistant", "content": "⏹️ stopped by user"
                    }));
                    finished_naturally = true;
                    break;
                }
            }
            let (assistant, _stop) = stream_result?;

            // Push the assistant message (OpenAI-native shape, may carry tool_calls).
            messages.as_array_mut().unwrap().push(assistant.clone());
            // 1.0.9 placeholder-spin guard (Muse 1.2 at 150k+ tok: see screenshots)
            {
                let is_ph = assistant.get("content").and_then(|c| c.as_str()).map(|s| is_placeholder_spin(s)).unwrap_or(false);
                let has_tools = assistant.get("tool_calls").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
                if is_ph && !has_tools { placeholder_streak += 1; } else if has_tools { placeholder_streak = 0; } else if !is_ph { placeholder_streak = 0; }
            }
            // Execute tool_calls (OpenAI shape) through the SAME jailed broker.
            let mut had_tools = false;
            if let Some(tcs) = assistant.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tcs {
                    had_tools = true;
                    let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let f = tc.get("function").cloned().unwrap_or(serde_json::json!({}));
                    let name = f.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let args_str = f.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                    let input: serde_json::Value = serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                    // M1.4 #7: inter-agent send_message routes through the mailbox.
                    let (result_text, is_err) = if name == "whoami" {
                        (introspect::build_whoami(&app, &db, &scope_id, folder.as_deref()), false)
                    } else if name == "task_continue" {
                        continue_gate::handle_task_continue(&app, &db, &scope_id, session_id.clone().unwrap_or_default(), &input).await
                    } else if name == "send_message" {
                        let to = input.get("to_agent").and_then(|t| t.as_str()).unwrap_or("");
                        let body = input.get("message").and_then(|m| m.as_str()).unwrap_or("");
                        match mailbox::send(&db, &scope_id, to, body, 0) {
                            Ok(mailbox::SendResult::Queued { .. }) => { drain.nudge(); (format!("message delivered to {to}"), false) }
                            Ok(mailbox::SendResult::Refused { reason }) => (format!("not sent: {reason}"), true),
                            Err(e) => (format!("send failed: {e}"), true),
                        }
                    } else if name == "transcribe_audio" {
                        let rel = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
                        match broker.resolve(&scope_id, rel, broker::Mode::Read) {
                            Err(e) => (format!("refused by jail: {e:?}"), true),
                            Ok(abs) => match std::fs::read(&abs) {
                                Err(e) => (format!("read audio: {e}"), true),
                                Ok(bytes) => {
                                    let fname = abs.file_name().and_then(|n| n.to_str()).unwrap_or("audio.mp3").to_string();
                                    match whisper::transcribe_bytes(bytes, &fname).await {
                                        Ok(text) => (text, false),
                                        Err(e) => (e, true),
                                    }
                                }
                            }
                        }
                    } else if connectors::is_connector_tool(&name) {
                        // Registry-driven connector tools (GitHub, Notion, ...):
                        // same privileged-side credential attach as the Anthropic
                        // path. This branch was MISSING here — the schemas were
                        // offered (agent_tools_for_full with conn_ctx) but dispatch
                        // fell through to "unknown tool" (Mason 08-19, bug #1).
                        connector_exec::exec(&db, &scope_id, &name, &input).await
                    } else if mcp::is_mcp_tool(&name) {
                        mcp::exec(&name, &input)
                    } else if video_tools::is_video_tool(&name) {
                        let o = video_tools::exec_full(&broker, &scope_id, &name, &input);
                        (o.text, o.is_err)
                    } else if dashboard::is_dashboard_tool(&name) {
                        dashboard::exec_dashboard_tool(&db, &scope_id, &name, &input)
                    } else {
                        exec_tool_cfg(&broker, &scope_id, &name, &input, &pdf_cfg)
                    };
                    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                    // STALL DETECTOR bookkeeping (see the Anthropic path).
                    round_calls += 1;
                    if is_err { round_errs += 1; }
                    tool_calls_total += 1;
                    let sig = format!("{name}:{}", serde_json::to_string(&input).unwrap_or_default());
                    if sig == last_sig { same_sig_streak += 1; } else { last_sig = sig; same_sig_streak = 1; }
                    last_action = if path.is_empty() { name.clone() } else { format!("{name} {path}") };
                    let _ = app.emit(&channel, &serde_json::json!({
                        "kind": "ToolResult", "id": id, "name": name, "path": path,
                        "ok": !is_err,
                        // Bounded output for the expanded activity card (Mason 08-03):
                        // errors in full flavor, successes as a 2000-char preview —
                        // the full result still goes to the model / logs regardless.
                        "detail": if is_err { result_text.clone() } else { result_text.chars().take(2000).collect::<String>() }
                    }));
                    // OpenAI expects tool results as {role:"tool", tool_call_id, content};
                    // build_openai_messages translates our tool_result blocks into that.
                    messages.as_array_mut().unwrap().push(serde_json::json!({
                        "role": "user",
                        "content": [{ "type": "tool_result", "tool_use_id": id, "content": result_text, "is_error": is_err }]
                    }));
                    if name == "video_look" {
                        let vf = video_tools::take_pending_vision();
                        let vb = vision_bytes(&broker, &scope_id, &vf);
                        if !vb.is_empty() {
                            let mut vc = vec![serde_json::json!({ "type": "text", "text": "Canvas frame(s) — you SEE them as images in this message." })];
                            vc.extend(vision_blocks_openai(&vb));
                            messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": vc }));
                        }
                    }
                }
            }

            // CLEO-GUARD:assistant-toolcalls (2026-08-02) — if the assistant carried
            // tool_calls but they were all empty (id/name not captured — a
            // reasoning-model wire quirk), do NOT push a half-built tool pairing
            // and loop; surface a clear error instead of a confusing 400 on the
            // next send.
            if had_tools {
                let all_empty = assistant.get("tool_calls")
                    .and_then(|tc| tc.as_array())
                    .map(|arr| arr.iter().all(|c| c.get("id").and_then(|i| i.as_str()).unwrap_or("").is_empty()
                                              || c.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("").is_empty()))
                    .unwrap_or(true);
                if all_empty {
                    return Err(format!(
                        "{provider_kind} emitted a tool_call whose capture came back empty (id/name missing) —                          this model's streamed tool-use isn't fully supported. Try a different model, or                          disable the tool that triggered the call."
                    ));
                }
            }
            if had_tools {
                if round_calls > 0 && round_errs == round_calls { err_round_streak += 1; } else { err_round_streak = 0; }
                // STALL DETECTOR (see the Anthropic path): nudge once, then pause.
                let stalled = same_sig_streak >= 5 || err_round_streak >= 4 || placeholder_streak >= 3;
                if stalled {
                    if stall_reason.is_none() {
                        stall_reason = Some(if placeholder_streak >= 3 { "placeholder spin (model stalled without tools)".to_string() } else if same_sig_streak >= 5 { "repeating the same call".to_string() } else { "every tool call failing".to_string() });
                        same_sig_streak = 0; err_round_streak = 0;
                        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user",
                            "content": "[system note] You appear stuck (repeating calls / repeated failures). Change approach, or stop calling tools and summarize where you are and what is blocking you." }));
                    } else {
                        break;
                    }
                }
                continue;
            }
            finished_naturally = true;
            break;
        }
        if !finished_naturally {
            // NEVER a blank or generic ending (Mason 08-03): say WHY, WHAT the
            // last action was, and HOW MUCH happened.
            let why = match &stall_reason {
                Some(r) => format!("stall detected — {r}"),
                None => format!("hit the {max_rounds}-round tool cap"),
            };
            let last = if last_action.is_empty() { String::new() } else { format!(" Last action: {last_action}.") };
            let warn = format!("\u{26A0}\u{FE0F} paused after {tool_calls_total} tool calls ({why}).{last} Reply to continue.");
            let _ = app.emit(&channel, &provider::StreamEvent::Info { text: warn.clone() });
            messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "assistant", "content": warn }));
        }

        // Snapshot AFTER the turn's writes, labeled with the prompt (C4).
        if let Ok(root) = broker.root_for(&scope_id) {
            match savepoint::snapshot(&root, &prompt) {
                Ok(Some(sha)) => { let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("SAVE POINT {sha}") }); }
                Ok(None) => {}
                // NEVER swallow snapshot failures (v1.0.1 polish #3: nested-repo
                // errors killed save points silently for days).
                Err(e) => { let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("\u{26A0} save point failed: {e}") }); }
            }
        }
        run_auto_capture(&app, &db, &broker, &scope_id, &prompt, &channel).await;
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

    // SAVE POINT (C4) part 1: ensure a BASELINE snapshot exists before the turn
    // runs. This captures the folder's pre-turn state (labeled "baseline") ONLY
    // if there are uncommitted changes / no history yet — so there's always an
    // anchor to rewind *back before* this turn's edits. It is NOT labeled with
    // the prompt: the prompt-labeled SAVE POINT is taken AFTER the turn (below),
    // so it correctly represents "the state produced by this prompt." This fixes
    // the bug where a turn's writes were absorbed (mislabeled) into the NEXT
    // turn's pre-snapshot, or lost entirely if they were the last edit.
    if let Ok(root) = broker.root_for(&scope_id) {
        let _ = savepoint::snapshot(&root, "Checkpoint");
    }

    let (tools, reg_instr) = agent_tools_for_full(&app, Some(&scope_id), folder.as_deref(), !roster.is_empty(), Some((&db, &scope_id)));
    let anthropic_sys = format!("{AGENT_SYSTEM}{extra_block}{reg_instr}");
    let mut messages = if history.is_array() { history } else { serde_json::json!([]) };
    // ATTACHMENTS (Mason 08-01): images/PDFs/text ride INTO the model as real
    // content blocks — the model sees what you attached, not a filename.
    let att = attachments.clone().unwrap_or_default();
    if att.is_empty() {
        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": prompt }));
    } else {
        let mut content = vec![serde_json::json!({ "type": "text", "text": prompt })];
        content.extend(attachment_blocks(&broker, &scope_id, &att));
        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": content }));
    }

    let emit = |ev: &provider::StreamEvent| { let _ = app.emit(&channel, ev); };
    emit(&provider::StreamEvent::Info { text: format!("model: {model}") });

    // PRO MODE UNCAPPED (Mason 08-03): in Pro Mode there is NO round cap — the
    // loop runs until the model stops calling tools, the user hits Stop, or the
    // STALL DETECTOR fires (the cap's replacement: a count cap punishes honest
    // long work; a stall detector catches actual degenerate loops). Non-Pro
    // keeps the 20-round cap. This is the fix for the "20 continues" session.
    let pro_uncapped = folder.as_deref().map(|f| pro_mode_enabled(&app, f)).unwrap_or(false);
    let max_rounds: usize = if pro_uncapped { usize::MAX } else { 20 };
    let mut rounds: usize = 0;
    let mut tool_calls_total: usize = 0;
    let mut last_action: String = String::new();
    let mut last_sig: String = String::new();   // name+input of the previous tool call
    let mut same_sig_streak: usize = 0;          // consecutive IDENTICAL calls
    let mut err_round_streak: usize = 0;         // consecutive rounds where EVERY tool errored
    let mut stall_reason: Option<String> = None;
    let mut placeholder_streak: usize = 0;
    let mut finished_naturally = false;
    while rounds < max_rounds {
        rounds += 1;
        // Soft checkpoint: long uncapped runs stay legible in the transcript.
        if pro_uncapped && rounds > 1 && rounds % 25 == 1 {
            emit(&provider::StreamEvent::Info { text: format!("still working — {tool_calls_total} tool calls so far") });
        }
        // STOP between rounds: the stream checks the flag on network chunks, but
        // a stop pressed DURING local tool execution lands here.
        if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "assistant", "content": [{ "type": "text", "text": "⏹️ stopped by user" }]
            }));
            finished_naturally = true;
            break;
        }
        let stream_result = provider::anthropic_stream_turn(
            &key, &model, &anthropic_sys, &messages, &tools, Some(&cancel_flag),
            |ev| { let _ = app.emit(&channel, &ev); },
        ).await;
        if let Err(e) = &stream_result {
            if e == "__CANCELLED__" {
                messages.as_array_mut().unwrap().push(serde_json::json!({
                    "role": "assistant", "content": [{ "type": "text", "text": "⏹️ stopped by user" }]
                }));
                finished_naturally = true;
                break;
            }
        }
        let (content, stop) = stream_result?;

        messages.as_array_mut().unwrap().push(serde_json::json!({
            "role": "assistant", "content": content.clone()
        }));

        // Execute any tool_use blocks through the broker; emit results live.
        let mut tool_results = Vec::new();
        let mut round_calls: usize = 0;   // stall detector: calls this round
        let mut round_errs: usize = 0;    // stall detector: errored calls this round
        if let Some(arr) = content.as_array() {
            for blk in arr {
                if blk.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let name = blk.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let id = blk.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let input = blk.get("input").cloned().unwrap_or(serde_json::json!({}));
                    // M1.4 #7: send_message is an inter-agent tool — it enqueues on
                    // the mailbox (needs db, not the broker), delivered ASYNC on
                    // the recipient's lane. Handle it here before the file-tool path.
                    let (result_text, is_err) = if name == "whoami" {
                        // SELF-INTROSPECTION: read-only identity + model + capabilities.
                        (introspect::build_whoami(&app, &db, &scope_id, folder.as_deref()), false)
                    } else if browser::is_agent_tool(&name) {
                        // BROWSER TOOLS (Slice 4/5): async, driven through the CDP
                        // session + per-agent domain policy + the shared-control
                        // wheel. Policy keys off the FOLDER (same key as
                        // agent_tools_for_full used to expose the tools).
                        let domains = folder.as_deref().map(|f| agent_browser_domains(&app, f)).unwrap_or_default();
                        browser::agent_tool(&app, &browser_state, &name, &input, &domains).await
                    } else if name == "task_continue" {
                        continue_gate::handle_task_continue(&app, &db, &scope_id, session_id.clone().unwrap_or_default(), &input).await
                    } else if name == "send_message" {
                        let to = input.get("to_agent").and_then(|t| t.as_str()).unwrap_or("");
                        let body = input.get("message").and_then(|m| m.as_str()).unwrap_or("");
                        let r = match mailbox::send(&db, &scope_id, to, body, 0) {
                            Ok(mailbox::SendResult::Queued { .. }) => { drain.nudge(); (format!("message delivered to {to} (they'll reply on their own time)"), false) }
                            Ok(mailbox::SendResult::Refused { reason }) => (format!("not sent: {reason}"), true),
                            Err(e) => (format!("send failed: {e}"), true),
                        };
                        r
                    } else if connectors::is_connector_tool(&name) {
                        // Registry-driven: one path for every connected service.
                        // The credential is resolved from the keychain and
                        // attached HERE, on the privileged side — the jailed
                        // brain never receives a token.
                        connector_exec::exec(&db, &scope_id, &name, &input).await
                    } else if name == "transcribe_audio" {
                        // Whisper (task #6): resolve the audio THROUGH THE JAIL
                        // (read mode), then the async API call. Key from Keychain.
                        let rel = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
                        match broker.resolve(&scope_id, rel, broker::Mode::Read) {
                            Err(e) => (format!("refused by jail: {e:?}"), true),
                            Ok(abs) => match std::fs::read(&abs) {
                                Err(e) => (format!("read audio: {e}"), true),
                                Ok(bytes) => {
                                    let fname = abs.file_name().and_then(|n| n.to_str()).unwrap_or("audio.mp3").to_string();
                                    match whisper::transcribe_bytes(bytes, &fname).await {
                                        Ok(text) => (text, false),
                                        Err(e) => (e, true),
                                    }
                                }
                            }
                        }
                    } else if mcp::is_mcp_tool(&name) {
                        mcp::exec(&name, &input)
                    } else if video_tools::is_video_tool(&name) {
                        let o = video_tools::exec_full(&broker, &scope_id, &name, &input);
                        (o.text, o.is_err)
                    } else if dashboard::is_dashboard_tool(&name) {
                        dashboard::exec_dashboard_tool(&db, &scope_id, &name, &input)
                    } else {
                    // Run the tool inside catch_unwind so a PANIC (e.g. deep in
                    // genpdf table/render) becomes a VISIBLE tool error the model
                    // gets back — instead of aborting the turn task silently and
                    // leaving the UI dead. This is the safety net that turns
                    // "silently fails" into a diagnosable message.
                        let b = &broker; let n = &name; let inp = &input; let sid = &scope_id;
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exec_tool(b, sid, n, inp))) {
                            Ok(r) => r,
                            Err(e) => {
                                let msg = e.downcast_ref::<&str>().map(|s| s.to_string())
                                    .or_else(|| e.downcast_ref::<String>().cloned())
                                    .unwrap_or_else(|| "tool panicked (unknown)".to_string());
                                (format!("tool '{name}' panicked: {msg}"), true)
                            }
                        }
                    };
                    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                    // STALL DETECTOR bookkeeping (Mason 08-03): identical-call streaks
                    // and all-errored rounds are what distinguish a degenerate loop
                    // from honest long work — this replaces the blunt 20-round cap
                    // in Pro Mode.
                    round_calls += 1;
                    if is_err { round_errs += 1; }
                    tool_calls_total += 1;
                    let sig = format!("{name}:{}", serde_json::to_string(&input).unwrap_or_default());
                    if sig == last_sig { same_sig_streak += 1; } else { last_sig = sig; same_sig_streak = 1; }
                    last_action = if path.is_empty() { name.clone() } else { format!("{name} {path}") };
                    // tell the UI the tool's OUTCOME (the ToolUse start already fired)
                    let _ = app.emit(&channel, &serde_json::json!({
                        "kind": "ToolResult", "id": id, "name": name, "path": path,
                        "ok": !is_err,
                        // Bounded output for the expanded activity card (Mason 08-03):
                        // errors in full flavor, successes as a 2000-char preview —
                        // the full result still goes to the model / logs regardless.
                        "detail": if is_err { result_text.clone() } else { result_text.chars().take(2000).collect::<String>() }
                    }));
                    // VISION must ride INSIDE this tool_result (Anthropic requires every
                    // tool_use to be followed immediately by its result; a separate user
                    // message here would break role alternation and 400 the next call).
                    let content_v: serde_json::Value = if name == "video_look" {
                        let vf = video_tools::take_pending_vision();
                        let vb = vision_bytes(&broker, &scope_id, &vf);
                        if vb.is_empty() { serde_json::json!(result_text.clone()) } else {
                            let mut vc = vec![serde_json::json!({ "type": "text", "text": format!("{}\n\nCanvas frame(s) — you SEE them as images in this message.", result_text) })];
                            vc.extend(vision_blocks_anthropic(&vb));
                            serde_json::json!(vc)
                        }
                    } else { serde_json::json!(result_text.clone()) };
                    tool_results.push(serde_json::json!({
                        "type": "tool_result", "tool_use_id": id,
                        "content": content_v, "is_error": is_err
                    }));
                }
            }
        }

        // 1.0.9 placeholder-spin (anthropic): count text-only placeholder turns
        if tool_results.is_empty() {
            let txt = content.as_array().and_then(|a| a.iter().find_map(|b| b.get("text").and_then(|v| v.as_str()))).unwrap_or("");
            if is_placeholder_spin(txt) { placeholder_streak += 1; } else { placeholder_streak = 0; }
        } else { placeholder_streak = 0; }

        // CRITICAL Anthropic invariant: EVERY tool_use block MUST be followed
        // immediately by a message containing its matching tool_result. So the
        // decision to send results is driven by "did we produce any tool_use
        // blocks?" (i.e. tool_results is non-empty) — NOT by the stop_reason.
        // The old guard keyed on `stop == "tool_use"`; if the model both spoke
        // and called a tool (stop can arrive as end_turn) we'd push the
        // assistant message with the dangling tool_use, skip the results, and
        // break — leaving history malformed and 400-ing the NEXT request.
        if !tool_results.is_empty() {
            // All-errored-round streak (a round where EVERY call failed is the
            // strongest loop smell; one mixed round of progress resets it).
            if round_calls > 0 && round_errs == round_calls { err_round_streak += 1; } else { err_round_streak = 0; }
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "user", "content": tool_results
            }));
            // STALL DETECTOR: 5 identical consecutive calls, or 4 consecutive
            // all-errored rounds → first offense injects a course-correct nudge
            // the model sees with its tool results; a persisting stall pauses the
            // turn with an HONEST status instead of looping forever.
            let stalled = same_sig_streak >= 5 || err_round_streak >= 4 || placeholder_streak >= 3;
            if stalled {
                if stall_reason.is_none() {
                    stall_reason = Some(if placeholder_streak >= 3 { "placeholder spin (model stalled without tools)".to_string() } else if same_sig_streak >= 5 { "repeating the same call".to_string() } else { "every tool call failing".to_string() });
                    same_sig_streak = 0; err_round_streak = 0;
                    messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": [{ "type": "text",
                        "text": "[system note] You appear stuck (repeating calls / repeated failures). Change approach, or stop calling tools and summarize where you are and what is blocking you." }] }));
                    if stop == "tool_use" { continue; }
                } else {
                    // Second stall after the nudge: pause the turn honestly.
                    break;
                }
            }
            // Only keep looping if the model actually wants to continue the
            // tool cycle; otherwise send results once and finish this turn.
            if stop == "tool_use" { continue; }
        }
        finished_naturally = true;
        break;
    }
    if !finished_naturally {
        // NEVER a blank or generic ending (Mason 08-03): say WHY the turn ended,
        // WHAT the last action was, and HOW MUCH happened — the reader should
        // not need to diagnose the loop from silence.
        let why = match &stall_reason {
            Some(r) => format!("stall detected — {r}"),
            None => format!("hit the {max_rounds}-round tool cap"),
        };
        let last = if last_action.is_empty() { String::new() } else { format!(" Last action: {last_action}.") };
        let warn = format!("\u{26A0}\u{FE0F} paused after {tool_calls_total} tool calls ({why}).{last} Reply to continue.");
        emit(&provider::StreamEvent::Info { text: warn.clone() });
        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "assistant", "content": [{ "type": "text", "text": warn }] }));
    }

    // SAVE POINT (C4) part 2: snapshot the folder AFTER the turn's writes, labeled
    // with THIS turn's prompt. Now every turn that changed files gets its own
    // correctly-labeled SAVE POINT, and "Rewind here" restores the state produced
    // by that prompt — which is what a user intuitively expects. Skips silently if
    // nothing changed (no empty SAVE POINTs). Best-effort: never blocks the reply.
    if let Ok(root) = broker.root_for(&scope_id) {
        match savepoint::snapshot(&root, &prompt) {
            Ok(Some(sha)) => { let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("save point {sha}") }); }
            Ok(None) => {}
            Err(e) => { let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("save point skipped: {e}") }); }
        }
        // Auto-prune anything past the retention window (best-effort; never blocks).
        if let Ok(days) = savepoint::get_retention(&root) {
            let _ = savepoint::prune(&root, days);
        }
    }

    // M1.7 AUTO-CAPTURE (Anthropic path). Shared helper so ALL provider paths
    // capture (bug: it was only here, on the Anthropic fall-through).
    run_auto_capture(&app, &db, &broker, &scope_id, &prompt, &channel).await;

    emit(&provider::StreamEvent::Done { stop_reason: "end_turn".into() });
    Ok(messages) // full history back for multi-turn persistence
}

// M1.7 AUTO-CAPTURE INTO THE REAL TURN LOOP: after a turn completes, run the
// salience+novelty-gated capture over the USER's message (the human's input
// carries the durable facts — "I prefer X", "I decided Y"). The Self-Gardening
// loop firing on ACTUAL conversation. Best-effort (gated on a per-agent toggle,
// never blocks the reply) BUT it now LOGS every branch to the terminal so a
// silent no-op can't hide (Mason 07-28: no 🧠 note + zero diagnosis). Called
// from every provider path's return so capture works regardless of model.
async fn run_auto_capture(
    app: &tauri::AppHandle,
    db: &writer::Db,
    broker: &Arc<Broker>,
    agent_id: &str,
    prompt: &str,
    channel: &str,
) {
    use tauri::Emitter;
    if !memory_auto_remember_enabled(db, agent_id) {
        eprintln!("[aygent][mem] auto-capture OFF for agent {agent_id}");
        return;
    }
    let abs_sentinel = match broker.resolve(agent_id, "Memory/.aygent-scope", broker::Mode::Write) {
        Ok(p) => p,
        Err(e) => { eprintln!("[aygent][mem] no vault scope for {agent_id} ({e:?}) — skipping capture"); return; }
    };
    let Some(abs_memory_dir) = abs_sentinel.parent().map(|p| p.to_path_buf()) else {
        eprintln!("[aygent][mem] could not resolve Memory dir — skipping"); return;
    };
    let embed_model = match ensure_embed_model(app).await {
        Ok(m) => m,
        Err(e) => { eprintln!("[aygent][mem] embed model unavailable ({e}) — skipping capture"); return; }
    };
    match memory::auto_capture(db, "agent", agent_id, &abs_memory_dir, "Memory", prompt, 0.65, "", &embed_model, "").await {
        Ok(r) => {
            eprintln!("[aygent][mem] auto-capture: {} candidate(s) → {} created, {} reinforced", r.candidates, r.created, r.reinforced);
            if r.created + r.reinforced > 0 {
                // Word it naturally for each case (reinforce-only was silent
                // before — Mason 07-28: '0 created, 2 reinforced' showed nothing).
                let text = match (r.created, r.reinforced) {
                    (c, 0) => format!("\u{1F9E0} remembered {c} new"),
                    (0, rf) => format!("\u{1F9E0} already knew that ({rf} reinforced)"),
                    (c, rf) => format!("\u{1F9E0} remembered {c} new, reinforced {rf}"),
                };
                // Emit a DEDICATED event (not a transient Info line, which the UI
                // dropped on finalize). The chat renders this as a small badge
                // under the USER's message that triggered it. (Mason 07-28.)
                let _ = app.emit(channel, &serde_json::json!({
                    "kind": "MemoryCaptured", "text": text,
                    "created": r.created, "reinforced": r.reinforced,
                }));
            }
        }
        Err(e) => eprintln!("[aygent][mem] auto-capture failed: {e}"),
    }
}

// --- HEADLESS INTER-AGENT TURN (M1.4 delivery engine) ----------------------
// Called by the drainer (drainer.rs) when a recipient agent has a delivered
// mailbox message. Runs ONE turn for the recipient using ITS own provider/model/
// folder/soul, jailed to ITS broker scope, executing tools through the broker,
// then PERSISTS the exchange to the recipient's conversation history (so the
// user sees it when they open that agent) and BROADCASTS activity events on a
// global `agent-activity` channel so any open UI pane can watch it happen live.
//
// This reuses the SAME provider primitives as agent_stream (anthropic_complete /
// the tool loop); it is intentionally a leaner, non-streaming turn — inter-agent
// turns don't need token streaming, and keeping it separate avoids destabilizing
// the battle-tested human path. If the recipient calls send_message during this
// turn, that's just another mailbox row the drainer picks up next tick (the
// reply loops back — the mailbox IS the channel).
pub async fn run_headless_turn(
    app: &tauri::AppHandle,
    db: &writer::Db,
    broker: &Arc<Broker>,
    _lanes: &lanes::Lanes,
    agent_id: &str,
    msg: &mailbox::Message,
) -> Result<(), String> {
    use tauri::Emitter;
    let agent = repo::get_agent(db, agent_id)?.ok_or("recipient agent gone")?;

    // Register the recipient's jail scope (it may not be active in the UI).
    if !agent.folder_path.is_empty() {
        let p = std::path::PathBuf::from(&agent.folder_path);
        if p.is_dir() {
            let canonical = std::fs::canonicalize(&p).unwrap_or(p);
            broker.set_scope(agent_id, canonical, false);
        }
    }

    // ORIGIN: a scheduler-fired turn has from_agent = "scheduler:<run_id>" — it
    // is NOT a peer message. It's a TASK to perform, with no one to reply to.
    // Framing + display name branch on this so the agent does the work and
    // never tries to send_message back to a non-existent "scheduler" agent
    // (Mason 07-28: it composed a great fact then failed trying to reply to
    // scheduler:7). Inter-agent turns keep the reply-capable peer framing.
    let is_scheduled = msg.from_agent.starts_with("scheduler:");
    let is_continue = msg.from_agent.starts_with("continue:");
    // AYGENT REMOTE (R4): a remote-originated turn IS the user speaking (from
    // their phone through the E2E channel) — framed verbatim, no task wrapper.
    let is_remote = msg.from_agent.starts_with("remote:");
    // task_continue routing (Mason 08-01, UI task #1): body may carry a
    // "conv:<session id>" first line — the chat the wake-up reports INTO.
    let (continue_conv, msg_body): (Option<String>, String) = if is_continue || is_remote {
        match msg.body.split_once('\n') {
            Some((first, rest)) if first.starts_with("conv:") => {
                let id = first.trim_start_matches("conv:").trim().to_string();
                (if id.is_empty() { None } else { Some(id) }, rest.to_string())
            }
            _ => (None, msg.body.clone()),
        }
    } else { (None, msg.body.clone()) };
    let msg_body = msg_body.as_str();

    let is_telegram = msg.from_agent.starts_with("telegram:");
    let telegram_chat_id: Option<String> = if is_telegram {
        let rest = msg.from_agent.strip_prefix("telegram:").unwrap_or("");
        let chat = rest.split(':').next().unwrap_or(rest).to_string();
        if chat.is_empty() { None } else { Some(chat) }
    } else { None };
    let telegram_cmd = if is_telegram { telegram::parse_command(msg_body) } else { telegram::Command::Chat };
    // Telegram pinned chat title + commands (Fix 7 follow-up): compact/newsession are local.
    // This keeps /compact fast (no LLM) and /newsession deterministic (wipe + confirm).
    let is_telegram_compact = is_telegram && telegram_cmd == telegram::Command::Compact;
    let is_telegram_new = is_telegram && telegram_cmd == telegram::Command::NewSession;
    let from_name = if is_telegram {
        "Telegram".to_string()
    } else if is_remote {
        "Remote".to_string()
    } else if is_continue {
        "Continuation".to_string()
    } else if is_scheduled {
        "Scheduler".to_string()
    } else {
        repo::get_agent(db, &msg.from_agent)?.map(|a| a.name).unwrap_or_else(|| msg.from_agent.clone())
    };

    let framed = if is_remote {
        // The user, remotely. No wrapper beyond a one-line situational note —
        // tools, memory, soul all apply exactly as if typed at the desk.
        format!(
            "(This message arrived via AYGENT Remote — the user is chatting from \
             their phone/browser. Reply normally; your answer streams back to \
             their remote screen and is saved in this conversation.)\n\n{}",
            msg_body
        )
    } else if is_continue {
        format!(
            "WAKE-UP: you previously called task_continue and asked to resume work. Your note to self:\n\n{}\n\n\
             This is a FULL working turn — you have all your tools. Continue the task now: check any processes you \
             started (shell_poll), read/edit files, run commands, finish the work, then report the outcome — this \
             turn streams live to your chat. If the work still is not done when you must stop, call task_continue \
             AGAIN with an updated note (never just say you will check back). Do NOT use send_message; there is no \
             sender to reply to.",
            msg_body
        )
    } else if is_scheduled {
        format!(
            "This is a SCHEDULED TASK that just fired (no sender to reply to). Do the task, \
             then stop — your output is recorded in your own notes. Do NOT use send_message; \
             there is no one to send it to.\n\nTask:\n\n{}",
            msg_body
        )
    } else if is_telegram {
        // Telegram: tag the message as from Telegram user; include chat routing.
        format!("Telegram message (chat {chat}):\n\n{body}", chat=telegram_chat_id.clone().unwrap_or_default(), body=msg_body)
    } else {
        format!(
            "You just received a message from another agent, {from_name} (id: {}). Their message:\n\n{}\n\n\
             Respond/act as appropriate. If you want to reply to them, use the send_message tool with \
             to_agent = \"{}\". Otherwise just do the work.",
            msg.from_agent, msg.body, msg.from_agent
        )
    };

    // The per-agent stream channel the UI subscribes to (SAME id the human path
    // uses for this agent's inbox conversation) so the turn streams LIVE into
    // whichever pane is viewing the recipient — you WATCH the work happen.
    let stream_channel = if is_telegram {
        format!("telegram-{agent_id}")
    } else {
        match &continue_conv {
            Some(cid) => cid.clone(),                    // wake-up streams into the origin chat
            None => format!("inbox-{agent_id}"),
        }
    };

    // 1) DISPATCH-TIME VISIBILITY: show the inbound message in the recipient's
    //    inbox thread IMMEDIATELY (before any work), so "📨 from Atlas: …" appears
    //    the instant it's sent. We persist it now + emit a stream event so an
    //    open pane renders it live.
    {
        let conv_id = if is_telegram { format!("telegram-{agent_id}") } else { continue_conv.clone().unwrap_or_else(|| format!("inbox-{agent_id}")) };
        let existing = repo::load_conversation(db, &conv_id).ok();
        let mut ui_msgs = existing.as_ref().and_then(|c| c.msgs.as_array().cloned()).unwrap_or_default();
        let inbound_text = if is_remote { format!("\u{1F4F1} {}", msg_body) }
            else if is_continue { format!("\u{23F0} resumed: {}", msg_body) }
            else { format!("\u{1F4E8} from {from_name}: {}", msg_body) };
        ui_msgs.push(serde_json::json!({ "role": "user", "from": from_name, "text": inbound_text }));
        let title = if is_telegram { "Telegram".to_string() } else { existing.as_ref().map(|c| c.title.clone()).unwrap_or_else(|| "Activity".into()) };
        let pinned = if is_telegram { true } else { existing.as_ref().map(|c| c.pinned).unwrap_or(true) };
        let order = if is_telegram { 0 } else { existing.as_ref().map(|c| c.order).unwrap_or(1) };
        let conv = repo::Conversation {
            id: conv_id, agent_id: agent_id.to_string(),
            title, updated: 0, pinned, order,
            msgs: serde_json::json!(ui_msgs),
            history: existing.map(|c| c.history).unwrap_or(serde_json::json!([])),
        };
        let _ = repo::save_conversation(db, conv);
        // Live: an inbound-message event the pane renders as a user bubble now.
        let _ = app.emit(&stream_channel, &serde_json::json!({
            "kind": "InboundMessage", "from": msg.from_agent, "fromName": from_name, "text": msg.body,
        }));
    }

    // Broadcast: this agent is now WORKING (rail shows a spinner) + the pane's
    // turn store flips to running so it shows a thinking indicator + live stream.
    let _ = app.emit("agent-activity", &serde_json::json!({
        "agentId": agent_id, "kind": "turn_start", "from": msg.from_agent, "fromName": from_name,
    }));
    let _ = app.emit(&stream_channel, &provider::StreamEvent::Info { text: format!("working on {from_name}’s request…") });

    // STOP support for headless turns (needed by AYGENT Remote's stop{turn}):
    // register a cancel flag under the stream channel — remote_runtime maps
    // turn->conv and calls request_stop(conv). Guard drops on any exit path.
    let cancel_reg = app.state::<cancel::CancelRegistry>();
    let (_hl_cancel_guard, hl_cancel_flag) =
        cancel::CancelGuard::new(cancel_reg.inner().clone(), stream_channel.clone());

    // Build the recipient's system prompt: soul + peers + context docs (same as
    // the human path, minus streaming).
    let persona = if agent.system_prompt.trim().is_empty() { String::new() } else { format!("\n\n{}", agent.system_prompt.trim()) };
    let context_block = context_docs::prepend_block(db, agent_id).unwrap_or_default();
    let roster = mailbox::roster(db, agent_id).unwrap_or_default();
    let roster_block = if roster.is_empty() { String::new() } else {
        let list = roster.iter().map(|(id, name)| format!("- {name} (id: {id})")).collect::<Vec<_>>().join("\n");
        format!("\n\nOTHER AGENTS you can message with send_message:\n{list}")
    };
    let mounts_block = mounts_prompt_block(broker, agent_id);
    // Resolve provider/model (recipient's own; fallback anthropic auto/haiku).
    let provider_kind = if agent.provider.is_empty() { "anthropic".to_string() } else { agent.provider.clone() };
    let muse_quiet = if provider_kind == "meta" { MUSE_QUIET_TOOLS } else { "" };

    // Tools: the FULL registry (file tools, connectors, MCP, video, dashboard,
    // task_continue…) — a wake-up must be able to keep doing real work, not just
    // talk (Mason 09-04). `reg_instr` carries the registry's usage instructions.
    let (tools, reg_instr) = agent_tools_for_full(app, Some(&agent.id), Some(&agent.folder_path), !roster.is_empty(), Some((db, &agent.id)));
    let pdf_cfg = pdf_config_for(app, Some(&agent.id), Some(&agent.folder_path));
    let system = format!("{AGENT_SYSTEM}{persona}{roster_block}{context_block}{mounts_block}{reg_instr}{muse_quiet}");

    // Baseline SAVE POINT before any writes.
    if let Ok(root) = broker.root_for(agent_id) { let _ = savepoint::snapshot(&root, "Checkpoint"); }

    // /compact and /newsession are handled LOCALLY for Telegram (no LLM stall).
    if is_telegram_compact {
        let conv_id = format!("telegram-{agent_id}");
        if let Ok(conv) = repo::load_conversation(db, &conv_id) {
            let hist = conv.history.as_array().cloned().unwrap_or_default();
            if hist.len() >= 4 {
                let flat = crate::flatten_history_for_summary(&hist);
                let clipped = &flat[..flat.len().min(6000)];
                let seed = serde_json::json!([{"role":"user","content":format!("[Compacted Telegram context]
{}", clipped)}, {"role":"assistant","content":"Understood \\u{2014} context compacted."}]);
                let _ = repo::save_conversation(db, repo::Conversation { id: conv_id.clone(), agent_id: agent_id.to_string(), title: "Telegram".into(), updated: 0, pinned: true, order: -1, msgs: conv.msgs, history: seed });
            }
        }
        telegram::reply_to_origin(&agent_id, &msg.from_agent, "Compacted context \\u{2014} ready for more.").await;
        return Ok(());
    }
    if is_telegram_new {
        let conv_id = format!("telegram-{agent_id}");
        // Wipe the Telegram pinned chat and start fresh
        let _ = repo::delete_conversation(db, &conv_id);
        let fresh = repo::Conversation { id: conv_id.clone(), agent_id: agent_id.to_string(), title: "Telegram".into(), updated: 0, pinned: true, order: -1, msgs: serde_json::json!([{ "role": "assistant", "text": "Started a new Telegram session." }]), history: serde_json::json!([]) };
        let _ = repo::save_conversation(db, fresh);
        telegram::reply_to_origin(&agent_id.to_string(), &msg.from_agent, "Started a new session.").await;
        return Ok(());
    }
    let mut messages = serde_json::json!([{ "role": "user", "content": framed }]);
    let mut reply_text = String::new();
    // WAKE-UP CONTEXT (Mason 09-04): a task_continue wake-up must remember what it
    // was doing. Feed the origin conversation's recent provider-format history to
    // the model (bounded), while `messages` stays the turn DELTA that gets
    // appended to that history at persist time (no duplication).
    let prior_history: Vec<serde_json::Value> = if is_continue {
        continue_conv.as_ref()
            .and_then(|cid| repo::load_conversation(db, cid).ok())
            .and_then(|c| c.history.as_array().cloned())
            .map(|h| { let n = h.len(); h.into_iter().skip(n.saturating_sub(40)).collect() })
            .unwrap_or_default()
    } else { Vec::new() };
    let with_prior = |turn: &serde_json::Value| -> serde_json::Value {
        if prior_history.is_empty() { return turn.clone(); }
        let mut all = prior_history.clone();
        if let Some(t) = turn.as_array() { all.extend(t.iter().cloned()); }
        serde_json::Value::Array(all)
    };

    // Only Anthropic + OpenAI/OpenRouter run headless for now (local models are
    // slower + the human path is where they're exercised). Non-cloud recipients
    // get a note instead of silently doing nothing.
    if provider_kind == "anthropic" {
        let key = keychain::get_key("anthropic").map_err(|_| "recipient has no anthropic key".to_string())?;
        let model = if agent.model.trim().is_empty() {
            let models = provider::anthropic_list_models(&key).await?;
            models.iter().find(|m| m.contains("haiku")).cloned().or_else(|| models.first().cloned()).ok_or("no model")?
        } else { agent.model.clone() };

        for _ in 0..6 {
            // STREAM LIVE to the recipient's inbox channel — same event shape the
            // human path emits — so an open pane WATCHES the work happen (tokens +
            // tool cards), not just a rail spinner.
            let model_msgs = with_prior(&messages);
            let (content, stop) = provider::anthropic_stream_turn(
                &key, &model, &system, &model_msgs, &tools, Some(&hl_cancel_flag),
                |ev| { let _ = app.emit(&stream_channel, &ev); },
            ).await?;
            messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "assistant", "content": content.clone() }));
            let mut tool_results = Vec::new();
            // Dedupe identical send_message calls WITHIN one turn: the model
            // sometimes emits the same reply twice in a single response. The
            // first delivers; the rest are acknowledged as already-sent (not an
            // error) so the model doesn't narrate a failure (Mason's 07-28 test).
            let mut sent_in_turn: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
            if let Some(arr) = content.as_array() {
                for blk in arr {
                    match blk.get("type").and_then(|t| t.as_str()) {
                        Some("text") => { if let Some(t) = blk.get("text").and_then(|t| t.as_str()) { reply_text.push_str(t); reply_text.push('\n'); } }
                        Some("tool_use") => {
                            let name = blk.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let id = blk.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                            let input = blk.get("input").cloned().unwrap_or(serde_json::json!({}));
                            let (result_text, is_err) = if name == "whoami" {
                                (introspect::build_whoami(app, db, agent_id, Some(&agent.folder_path)), false)
                            } else if name == "task_continue" {
                                continue_gate::handle_task_continue(app, db, agent_id, continue_conv.clone().unwrap_or_default(), &input).await
                            } else if name == "send_message" {
                                let to = input.get("to_agent").and_then(|t| t.as_str()).unwrap_or("");
                                let body = input.get("message").and_then(|m| m.as_str()).unwrap_or("");
                                // Already sent this exact (to, body) this turn? Ack, don't resend.
                                if !sent_in_turn.insert((to.to_string(), body.to_string())) {
                                    tool_results.push(serde_json::json!({ "type": "tool_result", "tool_use_id": id, "content": format!("already delivered to {to} in this turn"), "is_error": false }));
                                    continue;
                                }
                                // Reply carries the parent id so budget + chain track (msg.id).
                                match mailbox::send(db, agent_id, to, body, msg.id) {
                                    Ok(mailbox::SendResult::Queued { .. }) => {
                                        // WAKE the drainer so the reply is delivered to the
                                        // recipient near-instantly (loops the conversation).
                                        // Headless turns don't own the DrainSignal as a param,
                                        // so pull it from Tauri managed state (set at boot).
                                        app.state::<drainer::DrainSignal>().nudge();
                                        (format!("message delivered to {to}"), false)
                                    }
                                    Ok(mailbox::SendResult::Refused { reason }) => (format!("not sent: {reason}"), true),
                                    Err(e) => (format!("send failed: {e}"), true),
                                }
                            } else if mcp::is_mcp_tool(&name) {
                                mcp::exec(&name, &input)
                            } else if video_tools::is_video_tool(&name) {
                                let o = video_tools::exec_full(broker, agent_id, &name, &input);
                                (o.text, o.is_err)
                            } else if dashboard::is_dashboard_tool(&name) {
                                dashboard::exec_dashboard_tool(db, agent_id, &name, &input)
                            } else {
                                exec_tool_cfg(broker, agent_id, &name, &input, &pdf_cfg)
                            };
                            let content_v: serde_json::Value = if name == "video_look" {
                                let vf = video_tools::take_pending_vision();
                                let vb = vision_bytes(broker, agent_id, &vf);
                                if vb.is_empty() { serde_json::json!(result_text.clone()) } else {
                                    let mut vc = vec![serde_json::json!({ "type": "text", "text": format!("{}\n\nCanvas frame(s) — you SEE them as images in this message.", result_text) })];
                                    vc.extend(vision_blocks_anthropic(&vb));
                                    serde_json::json!(vc)
                                }
                            } else { serde_json::json!(result_text.clone()) };
                            tool_results.push(serde_json::json!({ "type": "tool_result", "tool_use_id": id, "content": content_v, "is_error": is_err }));
                        }
                        _ => {}
                    }
                }
            }
            if !tool_results.is_empty() {
                messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": tool_results }));
                if stop == "tool_use" { continue; }
            }
            break;
        }
    } else if provider_kind == "openai" || provider_kind == "openrouter" || provider_kind == "meta" {
        let key = keychain::get_key(&provider_kind).map_err(|_| format!("recipient has no {provider_kind} key"))?;
        if agent.model.trim().is_empty() { return Err("recipient has no model set".into()); }
        // TOOL LOOP (Mason 09-04): this branch used to be a single no-tools text
        // turn, so a task_continue wake-up on Muse/OpenAI could only TALK — it could
        // not shell_poll, read files or call task_continue again. Now it runs the
        // same bounded tool loop as the Anthropic headless path, streaming live.
        let _ = app.emit(&stream_channel, &provider::StreamEvent::Info { text: "thinking…".into() });
        // Headless turns act AS the agent, so they carry its variant too.
        let variant_hl: String = agent.model_variant.clone();
        let variant_hl_opt = if variant_hl.trim().is_empty() { None } else { Some(variant_hl.as_str()) };
        let mut sent_in_turn: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
        for _ in 0..12 {
            let model_msgs = with_prior(&messages);
            let mut round_text = String::new();
            let hl_stream = if provider_kind == "meta" {
                meta_provider::meta_stream_turn(&key, &agent.model, variant_hl_opt, &system, &model_msgs, &tools, Some(&hl_cancel_flag),
                    |ev| { if let provider::StreamEvent::TextDelta { text } = &ev { round_text.push_str(text); } let _ = app.emit(&stream_channel, &ev); }).await
            } else {
                openai_provider::openai_stream_turn(&provider_kind, &key, &agent.model, variant_hl_opt, &system, &model_msgs, &tools, Some(&hl_cancel_flag),
                    |ev| { if let provider::StreamEvent::TextDelta { text } = &ev { round_text.push_str(text); } let _ = app.emit(&stream_channel, &ev); }).await
            };
            let (assistant, stop) = match hl_stream {
                Ok(v) => v,
                Err(e) => { reply_text = format!("(couldn't complete the reply: {e})"); break; }
            };
            // Text the stream didn't deliver as deltas (some models only fill content).
            if round_text.trim().is_empty() {
                if let Some(c) = assistant.get("content").and_then(|c| c.as_str()) { if !c.trim().is_empty() { round_text = c.to_string(); let _ = app.emit(&stream_channel, &provider::StreamEvent::TextDelta { text: c.to_string() }); } }
            }
            if !round_text.trim().is_empty() { reply_text.push_str(round_text.trim()); reply_text.push('\n'); }
            messages.as_array_mut().unwrap().push(assistant.clone());
            let calls = assistant.get("tool_calls").and_then(|t| t.as_array()).cloned().unwrap_or_default();
            if calls.is_empty() || stop != "tool_use" { break; }
            let mut tool_results = Vec::new();
            for c in &calls {
                let id = c.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                let name = c.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                let input: serde_json::Value = c.get("function").and_then(|f| f.get("arguments")).and_then(|a| a.as_str())
                    .and_then(|a| serde_json::from_str(a).ok()).unwrap_or(serde_json::json!({}));
                let (result_text, is_err) = if name == "whoami" {
                    (introspect::build_whoami(app, db, agent_id, Some(&agent.folder_path)), false)
                } else if name == "task_continue" {
                    continue_gate::handle_task_continue(app, db, agent_id, continue_conv.clone().unwrap_or_default(), &input).await
                } else if name == "send_message" {
                    let to = input.get("to_agent").and_then(|t| t.as_str()).unwrap_or("");
                    let body = input.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    if !sent_in_turn.insert((to.to_string(), body.to_string())) { (format!("already delivered to {to} in this turn"), false) }
                    else {
                        match mailbox::send(db, agent_id, to, body, msg.id) {
                            Ok(mailbox::SendResult::Queued { .. }) => { app.state::<drainer::DrainSignal>().nudge(); (format!("message delivered to {to}"), false) }
                            Ok(mailbox::SendResult::Refused { reason }) => (format!("not sent: {reason}"), true),
                            Err(e) => (format!("send failed: {e}"), true),
                        }
                    }
                } else if connectors::is_connector_tool(&name) {
                    connector_exec::exec(db, agent_id, &name, &input).await
                } else if mcp::is_mcp_tool(&name) {
                    mcp::exec(&name, &input)
                } else if video_tools::is_video_tool(&name) {
                    let o = video_tools::exec_full(broker, agent_id, &name, &input);
                    (o.text, o.is_err)
                } else if dashboard::is_dashboard_tool(&name) {
                    dashboard::exec_dashboard_tool(db, agent_id, &name, &input)
                } else {
                    exec_tool_cfg(broker, agent_id, &name, &input, &pdf_cfg)
                };
                let _ = app.emit(&stream_channel, &serde_json::json!({ "kind": "ToolResult", "id": id, "name": name, "path": input.get("path").and_then(|p| p.as_str()).unwrap_or(""), "ok": !is_err, "detail": if is_err { result_text.clone() } else { result_text.chars().take(2000).collect::<String>() } }));
                tool_results.push(serde_json::json!({ "type": "tool_result", "tool_use_id": id, "content": result_text, "is_error": is_err }));
                if name == "video_look" {
                    let vf = video_tools::take_pending_vision();
                    let vb = vision_bytes(broker, agent_id, &vf);
                    if !vb.is_empty() {
                        let mut vc = vec![serde_json::json!({ "type": "text", "text": "Canvas frame(s) — you SEE them as images in this message." })];
                        vc.extend(vision_blocks_openai(&vb));
                        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": vc }));
                    }
                }
            }
            messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": tool_results }));
        }
    } else {
        reply_text = format!("({} runs a local model — headless inter-agent turns use a cloud provider for now.)", agent.name);
    }

    // Snapshot after writes.
    if let Ok(root) = broker.root_for(agent_id) { let _ = savepoint::snapshot(&root, &format!("from {from_name}")); }

    // If the turn produced no text (e.g. it only called write_file), don't show
    // a blank bubble — summarize what it did so the user + sender see SOMETHING.
    // This fixes bug #5's "blank message back".
    let reply_display = if reply_text.trim().is_empty() {
        "(done — completed the request without a text reply)".to_string()
    } else { reply_text.trim().to_string() };
    if msg.from_agent.starts_with("telegram:") {
        let origin = msg.from_agent.clone();
        let text = reply_display.clone();
        let aid = agent_id.to_string();
        tauri::async_runtime::spawn(async move { telegram::reply_to_origin(&aid, &origin, &text).await; });
    }

    // PERSIST to the recipient's conversation. CRITICAL (bug #5): we persist the
    // REAL provider-format `history` (messages), not just a display record — so
    // this exchange becomes actual conversational MEMORY. The inbox thread and
    // the agent's normal chat now share ONE conversation id so when you talk to
    // the agent directly it REMEMBERS the inter-agent message. We append the
    // full turn (framed inbound + assistant reply) to whatever history exists.
    let conv_id = if is_telegram { format!("telegram-{agent_id}") } else { continue_conv.clone().unwrap_or_else(|| format!("inbox-{agent_id}")) };
    let existing = repo::load_conversation(db, &conv_id).ok();
    let mut ui_msgs = existing.as_ref().and_then(|c| c.msgs.as_array().cloned()).unwrap_or_default();
    // NOTE (Mason 08-03, double-bubble fix): the inbound "resumed:"/"from X:"
    // user bubble was ALREADY persisted by the dispatch-time visibility block
    // before the turn ran — appending it again here rendered every wake-up
    // twice. Only the assistant reply is new at end-of-turn.
    ui_msgs.push(serde_json::json!({ "role": "assistant", "text": reply_display, "tools": [] }));
    // Real history: prior history + this turn's messages (framed user + all
    // assistant/tool turns we accumulated in `messages`). `messages` starts with
    // the framed user msg; append the whole thing to prior history.
    let mut hist = existing.as_ref().and_then(|c| c.history.as_array().cloned()).unwrap_or_default();
    if let Some(turn) = messages.as_array() { for m in turn { hist.push(m.clone()); } }
    let title2 = if is_telegram { "Telegram".to_string() } else { existing.as_ref().map(|c| c.title.clone()).unwrap_or_else(|| "Activity".into()) };
    let pinned2 = if is_telegram { true } else { existing.as_ref().map(|c| c.pinned).unwrap_or(true) };
    let order2 = if is_telegram { 0 } else { existing.as_ref().map(|c| c.order).unwrap_or(1) };
    let conv = repo::Conversation {
        id: conv_id, agent_id: agent_id.to_string(),
        title: title2, updated: 0, pinned: pinned2, order: order2,
        msgs: serde_json::json!(ui_msgs),
        history: serde_json::json!(hist),
    };
    let _ = repo::save_conversation(db, conv);

    // Broadcast: done + unread bump for the rail.
    let _ = app.emit("agent-activity", &serde_json::json!({
        "agentId": agent_id, "kind": "turn_done", "from": msg.from_agent, "fromName": from_name,
    }));
    Ok(())
}

/// Persist a VISIBLE error into an agent's inbox thread when a headless turn
/// fails before it could reply (Atlas #5 cause 3: no key/model → rail rings then
/// silence). Now the user sees WHY in the thread instead of a blank rail.
pub fn persist_inbox_error(db: &writer::Db, agent_id: &str, _msg: &mailbox::Message, err: &str) {
    let is_tg = _msg.from_agent.starts_with("telegram:");
    let conv_id = if is_tg { format!("telegram-{agent_id}") } else { format!("inbox-{agent_id}") };
    let existing = repo::load_conversation(db, &conv_id).ok();
    let mut ui_msgs = existing.as_ref().and_then(|c| c.msgs.as_array().cloned()).unwrap_or_default();
    // (Mason 08-03) Do NOT re-append the inbound bubble — dispatch-time
    // visibility already persisted it before the turn failed.
    ui_msgs.push(serde_json::json!({ "role": "assistant", "text": format!("⚠️ Couldn't process this message: {err}. (Check this agent has a provider key + model set.)"), "tools": [] }));
    let conv = repo::Conversation {
        id: conv_id, agent_id: agent_id.to_string(),
        title: "Activity".into(), updated: 0, pinned: true, order: 1,
        msgs: serde_json::json!(ui_msgs),
        history: existing.map(|c| c.history).unwrap_or(serde_json::json!([])),
    };
    let _ = repo::save_conversation(db, conv);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ENGINE-CEF (Phase 1): initialize CEF FIRST — before tauri::Builder builds
    // + starts NSApp. On macOS CefInitialize must run before the app run loop;
    // doing it in Tauri's setup (inside applicationDidFinishLaunching) makes it
    // fail (returns 0) and abort in a non-unwinding Obj-C frame. init_early()
    // does the framework load + execute_process + CefInitialize and NEVER
    // panics (returns false on failure → the browser degrades instead of
    // crashing the whole app). The AppHandle-dependent wiring happens later in
    // setup via cef_engine::attach_app().
    #[cfg(all(target_os = "macos", feature = "engine-cef"))]
    {
        eprintln!("[aygent][cef] run(): init_early BEFORE tauri::Builder");
        let ok = cef_engine::init_early();
        eprintln!("[aygent][cef] init_early -> {ok}");
    }

    let state = Arc::new(DaemonState {
        ws_token: mint_ws_token(),
        ..Default::default()
    });
    let broker = broker::Broker::new();
    // Separate per-session token for the broker WS (jail-boundary channel).
    let broker_token = mint_ws_token();

    // Per-session lanes (M1.1): serialize turn execution one-at-a-time per
    // session. Created here, managed as Tauri state, acquired in agent_stream.
    let lanes = lanes::Lanes::new();

    // M1.4 delivery engine: the signal the send path uses to WAKE the drainer
    // the instant a message is enqueued (near-instant delivery). Managed as
    // state so mailbox commands can nudge it.
    let drain_signal = drainer::DrainSignal::new();
    let sched_signal = scheduler::SchedSignal::new();

    // BROWSER (Slice 1): the long-lived headless Chromium handle. Launched on
    // first navigate, reused across navigations, held here as managed state.
    // NOTE: manage the PLAIN struct (it holds its own Mutex) so the managed type
    // matches the commands' State<'_, BrowserProc> — an Arc<BrowserProc> would
    // register a DIFFERENT type => 'state not managed'.
    let browser_proc = browser::BrowserProc::new();

    // STOP BUTTON: the turn-cancellation registry (see cancel.rs). Managed as
    // state so both agent_stream (registers/checks the flag) and the new
    // agent_stop command (flips it from the UI's Stop click) share ONE map.
    let cancel_registry = cancel::CancelRegistry::new();
    // AYGENT REMOTE: runtime handle (Settings starts/stops it; boot autostarts
    // if paired). Managed even when unpaired so state::<RemoteRuntime> is safe.
    let remote_runtime = remote_runtime::RemoteRuntime::default();

    // PRO MODE (2026-07-31): the exec broker — the ONLY code with process-spawn
    // authority. The daemon (Seatbelt deny-exec) requests spawns over the broker
    // WS; only THIS spawns. cwd-pinned to the agent scope, env-scrubbed. Managed
    // as state + handed to broker_ws so exec.* ops resolve against it.
    let exec_broker = exec::ExecBroker::new();
    // Install process-wide handles so the synchronous agent-tool dispatch
    // (exec_tool_cfg → exec_shell_tool) can reach the exec broker + resolve the
    // jailed root without threading state through every tool call site.
    exec::install_global(exec_broker.clone(), broker.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(broker.clone())
        .manage(exec_broker.clone())
        .manage(state.clone())
        .manage(lanes.clone())
        .manage(drain_signal.clone())
        .manage(sched_signal.clone())
        .manage(browser_proc)
        .manage(cancel_registry)
        .manage(continue_gate::ContinueGate::default())
        .manage(remote_runtime)
        .invoke_handler(tauri::generate_handler![
            whisper::transcribe_audio_b64,
            chat_attach_file,
            tool_file_before,
            agent_stop,
            remote_cmds::remote_status,
            remote_cmds::remote_pair,
            remote_cmds::remote_unpair,
            remote_cmds::remote_connect,
            remote_cmds::remote_local_status,
            remote_cmds::remote_set_enabled,
            remote_cmds::theme_sync,
            provider_verify_key,
            daemon_info, pick_agent_folder, broker_probe,
            set_provider_key, has_provider_key, anthropic_test, anthropic_models, agent_run,
            agent_stream, reveal_in_finder, get_selected_model, set_selected_model,
            get_selection, set_selection, detect_hardware, local_catalog, local_search, local_lookup, local_downloaded,
            local_download, local_delete, local_tool_capability,
            mlx::mlx_status, mlx::mlx_install_cmd, mlx::mlx_pull_cmd, mlx::mlx_stop_cmd, mlx::mlx_downloaded, mlx::mlx_delete_cmd, mlx::mlx_allow_code_cmd, mlx::mlx_code_status_cmd, restore_agent_folder,
            browser::browser_status, browser::browser_install, browser::browser_launch_probe,
            browser::browser_navigate, browser::browser_shutdown, browser::browser_uninstall, browser::browser_start_view, browser::browser_set_viewport,
            browser::browser_click, browser::browser_scroll, browser::browser_type, browser::browser_key,
            browser_policy_get, browser_policy_set,
            browser::browser_control_status, browser::browser_take_wheel, browser::browser_release_wheel,
            set_active_agent_marker,
            browser::browser_history_nav, browser::browser_page_info, browser::browser_downloads_list,
            browser::browser_history_list, browser::browser_history_clear,
            browser::browser_download_url,
                        browser::set_active_browser_tab, browser::browser_permission_answer,
            browser::set_browser_hittest, browser::browser_engine_info,
            openai_models, tools_list, tools_upsert, tools_delete, tools_set_enabled,
            tools_config, tools_set_config,
            hyperframes_status, hyperframes_provision, hyperframes_remove,
            mcp_list, mcp_install_plan, mcp_enable, mcp_disable, mcp_verify, mcp_add_custom, mcp_remove_custom,
            capabilities_list, skills_list,
            sparks_list, sparks_read, sparks_delete,
            spark_save,
            spark_state_get, spark_state_set, spark_state_set_key,
            video::video_status, video::video_projects, video::video_load, video::video_save, video::video_create,
            video::video_chat_save, video::video_pick_media, video::video_import_paths, video::video_delete_project, video::video_remove_asset, video::video_relink_asset,
            video::video_refresh_thumbs, video::video_list_luts, video::video_pick_lut, video::video_reveal,
            video::video_create_media_folder, video::video_rename_media_folder, video::video_delete_media_folder, video::video_move_media_assets, video::video_move_media_folder, video::video_pick_folder, video::video_create_sequence, video::video_rename_sequence, video::video_delete_sequence,
            video_render::video_render, video_render::video_render_cancel, video_render::video_frame,
            video_render::video_validate, video_render::video_list_renders,
            video_hyperframes::video_build_captions, video_hyperframes::video_render_overlay_cmd, video_hyperframes::video_pick_style_guide, video_hyperframes::video_caption_timing, video_hyperframes::video_save_transcript,
            video_tools::video_tool,
            dashboard::dashboard_load, dashboard::dashboard_upsert_module,
            dashboard::dashboard_remove_module, dashboard::dashboard_arrange,
            dashboard::dashboard_undo,
            dashboard_data::dashboard_refresh, dashboard_data::dashboard_refresh_module,
            dashboard_data::dashboard_approve_exec,
            dashboard_data::dashboard_run_tool,
            savepoint_snapshot, savepoint_timeline, savepoint_rewind,
            savepoint_undo, savepoint_redo,
            savepoint_get_retention, savepoint_set_retention, savepoint_purge,
            conv_list, conv_load, conv_save, conv_rename, conv_delete, conv_reorder,
            conv_compact, history_compact, chat_model_info,
            agents_list, agents_create, agents_update, agents_reorder, agents_delete,
            agents_set_active, agents_get_active, agents_sharing_folder,
            agent_mounts_list, agent_mount_add, agent_mount_remove,
            set_app_icon,
            agent_context_add, agent_context_list, agent_context_remove,
            agent_generate_soul,
            mailbox_pending_counts, mailbox_take_next, mailbox_roster,
            continue_gate::task_continue_answer,
            get_app_knobs, set_app_knobs,
            memory_ingest, memory_retrieve, memory_stats,
            memory_append_daily, memory_gate_check, memory_remember,
            memory_auto_capture, scheduler_list, scheduler_runs,
            scheduler_create, scheduler_set_enabled, scheduler_delete,
            scheduler_set_paused, scheduler_get_paused, scheduler_run_now,
            scheduler_reset_counters,
            github_connect, connections_list, connection_disconnect,
            connection_set_agent_enabled, connection_enabled_for_agent,
            connectors_catalog, connector_connect, connection_agent_state,
            connection_set_write, connection_set_tool_enabled, connection_tool_states,
            connection_set_read_only,
            memory_get_auto_remember, memory_set_auto_remember,
            pro_mode_get, pro_mode_set, shell_procs, shell_kill_proc,
            github_git_auth,
            telegram_status, telegram_set_token, telegram_test_token,
            onboarding_status, onboarding_pick_root, onboarding_set_root,
            onboarding_make_agent_home, onboarding_finish, import_memory
        ])
        .register_uri_scheme_protocol(video_media::SCHEME, video_media::handle)
        .setup(move |_app| {
            video_tools::install_app(_app.handle().clone());
            // CACHE-BUST FIRST (Mason 08-08): if this is a new build, clear the
            // stale WKWebView frontend cache before the window loads, so the new
            // UI code actually runs. Must happen before any content load.
            bust_webview_cache_on_version_change(&_app.handle());

            // ENGINE-CEF (Phase 1): CEF was ALREADY initialized at the top of
            // run() (init_early), BEFORE tauri::Builder — CefInitialize must run
            // before NSApp's run loop starts, so it CANNOT go here (setup fires
            // inside applicationDidFinishLaunching, after tao started NSApp; that
            // ordering made CefInitialize return 0 and abort in a non-unwinding
            // Obj-C frame). Here we only do the LATE wiring: stash the AppHandle
            // + downloads dir so handlers can emit events + save downloads.
            #[cfg(all(target_os = "macos", feature = "engine-cef"))]
            {
                use tauri::Manager;
                let dl = _app.handle().path().app_data_dir()
                    .map(|d| d.join("browser").join("downloads"))
                    .unwrap_or_else(|_| std::env::temp_dir());
                let _ = std::fs::create_dir_all(&dl);
                cef_engine::attach_app(_app.handle().clone(), dl);
            }

            // M1.1: bring up the SQLite state spine + single-writer actor, then
            // run the one-time JSON→SQLite import. Do this synchronously in
            // setup so every command that follows sees a ready DB. A failure
            // here is fatal — the app has no state without it.
            {
                // CONFIG RELOCATION: the state dir now comes from paths::state_dir
                // — <root>/.aygent when a root folder is configured (via root.json),
                // else the OS app-data dir (pre-onboarding / first launch). SQLite
                // + all JSON stores live there, so the whole config lives WITH the
                // folder Mason chose (owned + portable), not buried in app-support.
                let app_data = paths::state_dir(_app.handle())
                    .map_err(|e| format!("state_dir: {e}"))?;
                let db = writer::Db::start(app_data.clone())
                    .map_err(|e| format!("db init: {e}"))?;
                if let Err(e) = migrate_json::run(&db, &app_data) {
                    // Non-fatal: a bad legacy file shouldn't brick startup. Log
                    // and continue with whatever imported cleanly.
                    eprintln!("[aygent] JSON→SQLite migration warning: {e}");
                }
                _app.manage(db.clone());
                // Per-agent shell GitHub PAT scoping: make Db reachable from exec.rs
                crate::exec::install_db(db.clone());

                // M1.4 DELIVERY ENGINE: spawn the background drainer that runs
                // recipient inter-agent turns headlessly (independent of any
                // open chat pane). This is what makes Atlas→Copywriter→Atlas
                // actually execute. It nudges on send + polls as a safety net.
                {
                    use tauri::Manager;
                    let sig = _app.state::<drainer::DrainSignal>().inner().clone();
                    let brk = _app.state::<Arc<Broker>>().inner().clone();
                    let lns = _app.state::<lanes::Lanes>().inner().clone();
                    drainer::spawn(_app.handle().clone(), db.clone(), brk, lns, sig);

                    // AYGENT REMOTE: if this Mac is paired, bring the Realtime
                    // session up at boot — with retries, because the browser
                    // key may not be published yet (first-time pairing).
                    remote_runtime::spawn_autostart(_app.handle().clone());
                    telegram::spawn_all(_app.handle().clone(), db.clone());
                }

                // M1.8 SCHEDULER: spawn the ticker ("the drainer with a clock in
                // front of it"). Sleeps-until-next, marks fires exactly-once
                // through the writer actor, hands each fire to the drainer. Slice
                // 1: seeds one 60s proof schedule so a headless turn fires on
                // time, unattended, out of the box.
                {
                    use tauri::Manager;
                    let ssig = _app.state::<scheduler::SchedSignal>().inner().clone();
                    let dsig = _app.state::<drainer::DrainSignal>().inner().clone();
                    let brk = _app.state::<Arc<Broker>>().inner().clone();
                    let lns = _app.state::<lanes::Lanes>().inner().clone();
                    scheduler::spawn(_app.handle().clone(), db, brk, lns, ssig, dsig);
                }
            }

            // DOWNLOAD BRIDGE: the page-injected script emits `browser:save-request`
            // { url, name } on a right-click image save / download-link click.
            // wry's on_download never fires for context-menu saves, so THIS is
            // how downloads actually happen: our process fetches the bytes and
            // writes them to the agent folder (no WebKit, no sandbox).
            {
                use tauri::Listener;
                let dl_handle = _app.handle().clone();
                _app.listen_any("browser:save-request", move |ev| {
                    #[derive(serde::Deserialize)]
                    struct SaveReq { url: String, name: Option<String> }
                    // Payload is a JSON string; parse it.
                    if let Ok(req) = serde_json::from_str::<SaveReq>(ev.payload()) {
                        eprintln!("[aygent][browser][DL] save-request url={} name={:?}", req.url, req.name);
                        let h = dl_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            match browser::browser_download_url(h, req.url.clone(), req.name).await {
                                Ok(p) => eprintln!("[aygent][browser][DL] saved -> {p}"),
                                Err(e) => eprintln!("[aygent][browser][DL] save failed: {e}"),
                            }
                        });
                    }
                });
            }

            let broker = broker.clone();
            let exec_broker = exec_broker.clone();
            let state = state.clone();
            let broker_token = broker_token.clone();
            // Start the Rust-hosted broker WS server (M0.2b), then spawn the
            // daemon, handing it the broker-WS {port, token} so it can connect
            // as an authed client. jailed=false in dev; Seatbelt (jailed=true)
            // is finalized later in M0.2. The exec broker is passed in so exec.*
            // ops (Pro Mode) resolve against the same privileged actor.
            let app_handle_for_daemon = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match broker_ws::start(broker, exec_broker, broker_token.clone()).await {
                    Ok(broker_port) => {
                        // M0.2(e): jail ON by default on macOS (deny file+exec
                        // Seatbelt). Override with AYGENT_JAILED=0 for dev if a
                        // Seatbelt issue needs isolating.
                        let jailed = std::env::var("AYGENT_JAILED").as_deref() != Ok("0");
                        if let Err(e) = supervisor::spawn_daemon(
                            &app_handle_for_daemon, state.clone(), jailed, broker_port, &broker_token,
                        ) {
                            eprintln!("[aygent] daemon spawn failed: {e}");
                        }
                    }
                    Err(e) => eprintln!("[aygent] broker-ws failed to start: {e}"),
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building AYGENT")
        .run(|app_handle, event| match event {
            // CMD+Q / Quit menu (ExitRequested): clean shutdown. Kill daemon BEFORE exit
            // so no orphaned child triggers "quit unexpectedly" (was the crash report).
            // This is the ONLY path that terminates the process.
            tauri::RunEvent::ExitRequested { .. } => {
                if let Some(state) = app_handle.try_state::<Arc<supervisor::DaemonState>>() {
                    supervisor::shutdown(&state);
                }
                // Let Tauri complete the exit cleanly; do NOT call abort/exit(0) here
                // so Exit handler runs and reports normal termination.
            }
            // Red X (window close) => HIDE, don't quit. Scheduler + drainer keep running
            // in the background; Dock click reopens the window (Reopen event).
            tauri::RunEvent::WindowEvent {
                label, event: tauri::WindowEvent::CloseRequested { api, .. }, ..
            } => {
                // Prevent the window from being destroyed; hide it instead.
                api.prevent_close();
                if let Some(win) = app_handle.get_webview_window(&label) {
                    let _ = win.hide();
                }
            }
            // macOS Dock icon click when no windows visible => Reopen (show the window again).
            tauri::RunEvent::Reopen { .. } => {
                for (_label, win) in app_handle.webview_windows() {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            // Final teardown: CefShutdown must run once, after the run loop
            // ends — cef_engine::shutdown_engine was written for this but was
            // never wired (found in the 08 warning sweep). No-op if CEF never
            // initialized (CEF_READY guard).
            tauri::RunEvent::Exit => {
                mcp_client::stop_all();
                #[cfg(all(target_os = "macos", feature = "engine-cef"))]
                cef_engine::shutdown_engine();
            }
            _ => {}
        });
}
