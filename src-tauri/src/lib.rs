// AYGENT — Tauri shell entry (the privileged side).
// Owns: window, tray, keychain broker, the PATH BROKER (trust boundary),
// and the daemon supervisor that launches the Node daemon (on macOS, UNDER a
// Seatbelt profile that denies file+exec — Atlas C1). Also mints the
// per-session WS token (C6) and hands it + the daemon port to the UI.

mod agents;
mod broker;
mod broker_ws;
mod catalog;
mod checkpoint;
mod context_docs;
mod conversations;
mod db;
mod drainer;
mod lanes;
mod mailbox;
mod memory;
mod migrate_json;
mod repo;
mod writer;
mod gguf;
mod hardware;
mod keychain;
mod local_provider;
mod local_tools;
mod openai_provider;
mod pdf_tool;
mod provider;
mod settings;
mod supervisor;
mod tools_registry;

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
    for a in agents {
        if a.archived || a.folder_path.is_empty() { continue; }
        let path = std::path::PathBuf::from(&a.folder_path);
        if !path.is_dir() { continue; }
        let canonical = std::fs::canonicalize(&path).unwrap_or(path);
        broker.set_scope(&a.id, canonical, false);
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

/// Pick a folder WITHOUT changing any agent's scope — used by the memory test
/// panel so you browse to a vault instead of hand-typing a path (which is how a
/// repo root gets picked by mistake). Returns the chosen absolute path or None.
#[tauri::command]
async fn pick_vault_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |chosen| {
        let _ = tx.send(chosen);
    });
    let chosen = tokio::task::spawn_blocking(move || rx.recv().ok().flatten())
        .await
        .map_err(|e| e.to_string())?;
    Ok(chosen.map(|p| p.to_string()))
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
    openai_provider::list_models(&provider, &key).await
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
#[tauri::command]
fn get_selection(db: tauri::State<writer::Db>, folder: String) -> Result<serde_json::Value, String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    let s = repo::load_settings(&db, &agent_id)?;
    Ok(serde_json::json!({ "provider": s.provider, "model": s.model }))
}

#[tauri::command]
fn set_selection(db: tauri::State<writer::Db>, folder: String, provider: String, model: String) -> Result<(), String> {
    let agent_id = agent_for_folder(&db, &folder)?;
    let mut s = repo::load_settings(&db, &agent_id)?;
    s.provider = provider;
    s.model = model;
    repo::save_settings(&db, &agent_id, s)
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
        &context_mode.unwrap_or_default(),
        &system_prompt.unwrap_or_default(),
    )?;
    // M1.4: register the new agent's scope immediately so it's runnable this
    // session (concurrent with every other agent) — no restart needed.
    register_all_agent_scopes(&db, &broker);
    Ok(created)
}

#[tauri::command]
fn agents_update(db: tauri::State<writer::Db>, broker: tauri::State<'_, Arc<Broker>>, profile: repo::AgentProfile) -> Result<(), String> {
    repo::update_agent(&db, profile)?;
    // Folder may have changed — re-register all scopes so the jail tracks it.
    register_all_agent_scopes(&db, &broker);
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

#[tauri::command]
fn agents_get_active(db: tauri::State<writer::Db>) -> Result<Option<repo::AgentProfile>, String> {
    repo::get_active_agent(&db)
}

/// M1.4 (#2): the other non-archived agents sharing `folder_path` with `agent_id`
/// (pass "" as agent_id to include all). Agents on the SAME folder share
/// checkpoint history + the folder write-lock (CONTRACTS §4). The UI uses this
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
        "openai" | "openrouter" => {
            let key = keychain::get_key(&provider).map_err(|_| format!("no {provider} key set"))?;
            let model = agent.model.clone();
            if model.trim().is_empty() { return Err("pick a model for this agent first".into()); }
            openai_provider::complete(&provider, &key, &model, &meta).await?
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
            "context_tokens": m.context_tokens,
            "downloads": m.downloads, "updated": m.updated, "quants": quants,
        })
    }).collect();
    Ok(serde_json::json!({ "hardware": hw, "models": scored }))
}

/// Choose the context window (tokens) for a local model: the model's real
/// advertised window, capped to a memory-safe budget for THIS machine. The KV
/// cache scales ~linearly with context, so we cap by usable accelerator memory.
/// Heuristic: budget ~ half of usable memory for context, at a rough
/// ~0.5 MB/token for a small model's KV cache. Clamped to sane bounds. This is
/// intentionally conservative so it "just works" without OOMing a user's Mac.
fn local_context_budget(gguf_path: &str) -> u32 {
    // Model's real window, parsed from its filename via the catalog's rules.
    let name = std::path::Path::new(gguf_path)
        .file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    let model_ctx = catalog::context_window(&name); // 0 if unknown

    // Memory-safe ceiling from detected hardware.
    let hw = hardware::detect();
    // Reserve ~40% of usable memory for context; ~0.0005 GB per token is a
    // conservative small-model KV estimate → tokens = mem*0.4 / 0.0005.
    let mem_tokens = ((hw.accel_mem_gb as f64) * 0.40 / 0.0005) as u32;

    let chosen = match (model_ctx, mem_tokens) {
        (0, 0) => 4096,
        (0, m) => m.min(8192),           // unknown model window: modest default
        (c, 0) => c.min(8192),
        (c, m) => c.min(m),              // model window, capped by memory
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

// --- TOOLS registry (extensible agent capabilities) ------------------------
// (uses the existing `app_data` helper defined earlier)

/// Full registry (builtins + user tools) with each tool's enabled-state for a
/// folder folded in.
#[tauri::command]
fn tools_list(app: tauri::AppHandle, folder: Option<String>) -> Result<serde_json::Value, String> {
    let ad = app_data(&app)?;
    let all = tools_registry::load_registry(&ad);
    let enabled = folder.as_ref().map(|f| tools_registry::load_enabled(&ad, f)).unwrap_or_default();
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
fn tools_set_enabled(app: tauri::AppHandle, folder: String, id: String, on: bool) -> Result<(), String> {
    tools_registry::set_enabled(&app_data(&app)?, &folder, &id, on)
}

/// A tool's config SCHEMA (what settings it exposes) + the folder's saved VALUES.
#[tauri::command]
fn tools_config(app: tauri::AppHandle, folder: String, id: String) -> Result<serde_json::Value, String> {
    let ad = app_data(&app)?;
    Ok(serde_json::json!({
        "schema": tools_registry::config_schema(&id),
        "values": tools_registry::tool_config(&ad, &folder, &id),
        "fonts": pdf_tool::system_fonts(),
    }))
}

/// Save a tool's config values for a folder.
#[tauri::command]
fn tools_set_config(app: tauri::AppHandle, folder: String, id: String, values: serde_json::Value) -> Result<(), String> {
    tools_registry::set_tool_config(&app_data(&app)?, &folder, &id, values)
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

/// Same as exec_tool, but with the PDF tool's per-folder config (font/colors/
/// page-size/margins). `pdf_config` is {} when unavailable.
fn exec_tool_cfg(
    broker: &Arc<Broker>,
    agent_id: &str,
    name: &str,
    input: &serde_json::Value,
    pdf_config: &serde_json::Value,
) -> (String, bool) {
    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
    match name {
        "read_file" => match broker.resolve_and_open(agent_id, path, broker::Mode::Read) {
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
        // TOOLS registry: PDF generator (first built-in tool). Output path is
        // resolved THROUGH THE BROKER — a PDF can only land inside the folder.
        "generate_pdf" => {
            let title = input.get("title").and_then(|t| t.as_str()).unwrap_or("Document");
            let content = input.get("content").and_then(|c| c.as_str()).unwrap_or("");
            let out_path = input.get("output_path").and_then(|p| p.as_str())
                .or_else(|| input.get("path").and_then(|p| p.as_str()))
                .unwrap_or("document.pdf");
            match broker.resolve("default", out_path, broker::Mode::Write) {
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
        other => (format!("unknown tool: {other}"), true),
    }
}

/// Base file tools, always available. The Tools registry adds MORE on top.
/// M1.4: `send_message` is included when the agent has peers (roster non-empty)
/// — see base_tools_with_peers. This bare version is the file-only fallback.
fn base_tools() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({ "name": "read_file", "description": "Read a UTF-8 text file inside the agent folder. Path relative to folder root.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } }),
        serde_json::json!({ "name": "write_file", "description": "Write a UTF-8 text file inside the agent folder. Path relative to folder root.",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] } }),
        serde_json::json!({ "name": "list_files", "description": "List entries in a directory inside the agent folder. Path relative to root; '.' for root.",
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
        _ => None,
    }
}

/// Assemble the tool list for a turn: base file tools + any ENABLED registry
/// tools for this folder. Also returns composed-tool instructions to append to
/// the system prompt. `app` provides the app-data dir for the registry.
fn agent_tools_for(app: &tauri::AppHandle, folder: Option<&str>) -> (serde_json::Value, String) {
    agent_tools_for_ex(app, folder, false)
}

/// M1.4: like agent_tools_for but adds the inter-agent `send_message` tool when
/// `has_peers` is true (the agent has at least one other agent to talk to).
fn agent_tools_for_ex(app: &tauri::AppHandle, folder: Option<&str>, has_peers: bool) -> (serde_json::Value, String) {
    let mut tools = base_tools();
    if has_peers { tools.push(send_message_tool()); }
    let mut extra_instructions = String::new();

    if let (Ok(ad), Some(f)) = (app_data(app), folder) {
        for t in tools_registry::enabled_tools(&ad, f) {
            match t.kind.as_str() {
                "builtin" => {
                    if let Some(schema) = builtin_tool_schema(&t.name) { tools.push(schema); }
                }
                "composed" => {
                    // A composed tool is exposed as a named tool the model can
                    // "invoke" by following its saved instructions using the base
                    // tools it's allowed. We surface it as a no-arg-ish tool plus
                    // an instruction block so the model knows what it does.
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

/// Back-compat: the base-only tool set (used where no folder/app context).
fn agent_tools() -> serde_json::Value {
    serde_json::json!(base_tools())
}

/// The PDF tool's per-folder config ({} if unavailable). Passed into exec so the
/// agent's generate_pdf calls honor the user's font/color/page settings.
fn pdf_config_for(app: &tauri::AppHandle, folder: Option<&str>) -> serde_json::Value {
    if let (Ok(ad), Some(f)) = (app_data(app), folder) {
        tools_registry::tool_config(&ad, f, "builtin.pdf")
    } else {
        serde_json::json!({})
    }
}

const AGENT_SYSTEM: &str = "You are AYGENT, an agent that can ONLY touch files inside the user's \
    chosen folder via your tools. You cannot run shell commands. Use read_file/write_file/list_files \
    for file work. Be concise and friendly.";

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
    channel: String,
    prompt: String,
    history: serde_json::Value,
    model: Option<String>,
    provider: Option<String>,
    folder: Option<String>,
    session_id: Option<String>,
    agent_id: Option<String>,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    let broker = broker.inner().clone();
    let provider_kind = provider.unwrap_or_default();

    // ---- PER-SESSION LANE (M1.1) -------------------------------------------
    // Serialize turns for THIS session: if another turn is already running on
    // it, we wait here until it finishes. One turn at a time per session kills
    // tool/session races (double writes, torn streams, double checkpoints) at
    // the source. The lane key is the session id when the UI supplies one, else
    // the per-conversation event channel (also unique per conversation). The
    // guard is held for the whole turn — dropped automatically on return.
    let lane_key = session_id.clone().unwrap_or_else(|| channel.clone());
    let _lane = lanes.acquire(&lane_key).await;

    // ---- WHICH AGENT IS THIS? (M1.4) ---------------------------------------
    // Resolve the acting agent's real id. Every file op + checkpoint below jails
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
        // Baseline checkpoint before any tool writes (same as Anthropic path).
        if let Ok(root) = broker.root_for(&scope_id) {
            let _ = checkpoint::snapshot(&root, "baseline");
        }
        // Enabled registry tools (e.g. PDF) contribute extra instructions the
        // local model should know about, appended to its native tool prompt.
        let (_reg_tools, reg_instr) = agent_tools_for(&app, folder.as_deref());
        let pdf_cfg = pdf_config_for(&app, folder.as_deref());
        let base_sys = format!("{AGENT_SYSTEM_LOCAL}{extra_block}{reg_instr}");
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

            // Parse tool calls in the model's native format.
            let calls = local_tools::parse_tool_calls(&text, &cap.format);
            if calls.is_empty() { break; } // no tool wanted → done

            // Execute each call through the SAME jailed broker + emit UI events.
            let mut results_text = String::new();
            for c in &calls {
                let _ = app.emit(&channel, &serde_json::json!({
                    "kind": "ToolUse", "name": c.name,
                    "input": c.input,
                }));
                let (result, is_err) = exec_tool_cfg(&broker, &scope_id, &c.name, &c.input, &pdf_cfg);
                let path_s = c.input.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                let _ = app.emit(&channel, &serde_json::json!({
                    "kind": "ToolResult", "name": c.name, "path": path_s,
                    "ok": !is_err, "detail": if is_err { result.clone() } else { String::new() }
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
            let _ = checkpoint::snapshot(&root, &prompt);
        }
        return Ok(messages);
    }

    // ---- OPENAI / OPENROUTER PATH ------------------------------------------
    // Shared Chat Completions wire format; one impl, two base URLs. Full tool-
    // use: same jailed exec_tool + broker + checkpoints as every other provider.
    if provider_kind == "openai" || provider_kind == "openrouter" {
        let key = keychain::get_key(&provider_kind)
            .map_err(|_| format!("no {provider_kind} key set — add one in Settings"))?;
        let model = model.filter(|m| !m.trim().is_empty())
            .ok_or_else(|| format!("no {provider_kind} model selected — pick one in Settings"))?;

        // Baseline checkpoint before the turn (rewind anchor), same as Anthropic.
        if let Ok(root) = broker.root_for(&scope_id) {
            let _ = checkpoint::snapshot(&root, "baseline");
        }

        let (tools, reg_instr) = agent_tools_for_ex(&app, folder.as_deref(), !roster.is_empty());
        let pdf_cfg = pdf_config_for(&app, folder.as_deref());
        let sys = format!("{AGENT_SYSTEM}{extra_block}{reg_instr}");
        let mut messages = if history.is_array() { history } else { serde_json::json!([]) };
        messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": prompt }));
        let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("model: {model}") });

        for _ in 0..8 {
            let (assistant, _stop) = openai_provider::openai_stream_turn(
                &provider_kind, &key, &model, &sys, &messages, &tools,
                |ev| { let _ = app.emit(&channel, &ev); },
            ).await?;

            // Push the assistant message (OpenAI-native shape, may carry tool_calls).
            messages.as_array_mut().unwrap().push(assistant.clone());

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
                    let (result_text, is_err) = if name == "send_message" {
                        let to = input.get("to_agent").and_then(|t| t.as_str()).unwrap_or("");
                        let body = input.get("message").and_then(|m| m.as_str()).unwrap_or("");
                        match mailbox::send(&db, &scope_id, to, body, 0) {
                            Ok(mailbox::SendResult::Queued { .. }) => { drain.nudge(); (format!("message delivered to {to}"), false) }
                            Ok(mailbox::SendResult::Refused { reason }) => (format!("not sent: {reason}"), true),
                            Err(e) => (format!("send failed: {e}"), true),
                        }
                    } else {
                        exec_tool_cfg(&broker, &scope_id, &name, &input, &pdf_cfg)
                    };
                    let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                    let _ = app.emit(&channel, &serde_json::json!({
                        "kind": "ToolResult", "name": name, "path": path,
                        "ok": !is_err, "detail": if is_err { result_text.clone() } else { String::new() }
                    }));
                    // OpenAI expects tool results as {role:"tool", tool_call_id, content};
                    // build_openai_messages translates our tool_result blocks into that.
                    messages.as_array_mut().unwrap().push(serde_json::json!({
                        "role": "user",
                        "content": [{ "type": "tool_result", "tool_use_id": id, "content": result_text, "is_error": is_err }]
                    }));
                }
            }

            if had_tools { continue; }
            break;
        }

        // Snapshot AFTER the turn's writes, labeled with the prompt (C4).
        if let Ok(root) = broker.root_for(&scope_id) {
            match checkpoint::snapshot(&root, &prompt) {
                Ok(Some(sha)) => { let _ = app.emit(&channel, &provider::StreamEvent::Info { text: format!("checkpoint {sha}") }); }
                _ => {}
            }
        }
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

    let (tools, reg_instr) = agent_tools_for_ex(&app, folder.as_deref(), !roster.is_empty());
    let anthropic_sys = format!("{AGENT_SYSTEM}{extra_block}{reg_instr}");
    let mut messages = if history.is_array() { history } else { serde_json::json!([]) };
    messages.as_array_mut().unwrap().push(serde_json::json!({ "role": "user", "content": prompt }));

    let emit = |ev: &provider::StreamEvent| { let _ = app.emit(&channel, ev); };
    emit(&provider::StreamEvent::Info { text: format!("model: {model}") });

    for _ in 0..8 {
        let (content, stop) = provider::anthropic_stream_turn(
            &key, &model, &anthropic_sys, &messages, &tools,
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
                    // M1.4 #7: send_message is an inter-agent tool — it enqueues on
                    // the mailbox (needs db, not the broker), delivered ASYNC on
                    // the recipient's lane. Handle it here before the file-tool path.
                    let (result_text, is_err) = if name == "send_message" {
                        let to = input.get("to_agent").and_then(|t| t.as_str()).unwrap_or("");
                        let body = input.get("message").and_then(|m| m.as_str()).unwrap_or("");
                        let r = match mailbox::send(&db, &scope_id, to, body, 0) {
                            Ok(mailbox::SendResult::Queued { .. }) => { drain.nudge(); (format!("message delivered to {to} (they'll reply on their own time)"), false) }
                            Ok(mailbox::SendResult::Refused { reason }) => (format!("not sent: {reason}"), true),
                            Err(e) => (format!("send failed: {e}"), true),
                        };
                        r
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

        // CRITICAL Anthropic invariant: EVERY tool_use block MUST be followed
        // immediately by a message containing its matching tool_result. So the
        // decision to send results is driven by "did we produce any tool_use
        // blocks?" (i.e. tool_results is non-empty) — NOT by the stop_reason.
        // The old guard keyed on `stop == "tool_use"`; if the model both spoke
        // and called a tool (stop can arrive as end_turn) we'd push the
        // assistant message with the dangling tool_use, skip the results, and
        // break — leaving history malformed and 400-ing the NEXT request.
        if !tool_results.is_empty() {
            messages.as_array_mut().unwrap().push(serde_json::json!({
                "role": "user", "content": tool_results
            }));
            // Only keep looping if the model actually wants to continue the
            // tool cycle; otherwise send results once and finish this turn.
            if stop == "tool_use" { continue; }
        }
        break;
    }

    // CHECKPOINT (C4) part 2: snapshot the folder AFTER the turn's writes, labeled
    // with THIS turn's prompt. Now every turn that changed files gets its own
    // correctly-labeled checkpoint, and "Rewind here" restores the state produced
    // by that prompt — which is what a user intuitively expects. Skips silently if
    // nothing changed (no empty checkpoints). Best-effort: never blocks the reply.
    if let Ok(root) = broker.root_for(&scope_id) {
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

    // Look up the sender's display name for the in-thread "from X" tag.
    let from_name = repo::get_agent(db, &msg.from_agent)?.map(|a| a.name).unwrap_or_else(|| msg.from_agent.clone());

    // The prompt = the incoming message, framed so the recipient knows it's from
    // a peer and MAY reply via send_message (back to the sender).
    let framed = format!(
        "You just received a message from another agent, {from_name} (id: {}). Their message:\n\n{}\n\n\
         Respond/act as appropriate. If you want to reply to them, use the send_message tool with \
         to_agent = \"{}\". Otherwise just do the work.",
        msg.from_agent, msg.body, msg.from_agent
    );

    // The per-agent stream channel the UI subscribes to (SAME id the human path
    // uses for this agent's inbox conversation) so the turn streams LIVE into
    // whichever pane is viewing the recipient — you WATCH the work happen.
    let stream_channel = format!("inbox-{agent_id}");

    // 1) DISPATCH-TIME VISIBILITY: show the inbound message in the recipient's
    //    inbox thread IMMEDIATELY (before any work), so "📨 from Atlas: …" appears
    //    the instant it's sent. We persist it now + emit a stream event so an
    //    open pane renders it live.
    {
        let conv_id = format!("inbox-{agent_id}");
        let existing = repo::load_conversation(db, &conv_id).ok();
        let mut ui_msgs = existing.as_ref().and_then(|c| c.msgs.as_array().cloned()).unwrap_or_default();
        ui_msgs.push(serde_json::json!({ "role": "user", "from": from_name, "text": format!("\u{1F4E8} from {from_name}: {}", msg.body) }));
        let conv = repo::Conversation {
            id: conv_id, agent_id: agent_id.to_string(),
            title: "Inter-agent inbox".into(), updated: 0, pinned: true, order: 1,
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

    // Build the recipient's system prompt: soul + peers + context docs (same as
    // the human path, minus streaming).
    let persona = if agent.system_prompt.trim().is_empty() { String::new() } else { format!("\n\n{}", agent.system_prompt.trim()) };
    let context_block = context_docs::prepend_block(db, agent_id).unwrap_or_default();
    let roster = mailbox::roster(db, agent_id).unwrap_or_default();
    let roster_block = if roster.is_empty() { String::new() } else {
        let list = roster.iter().map(|(id, name)| format!("- {name} (id: {id})")).collect::<Vec<_>>().join("\n");
        format!("\n\nOTHER AGENTS you can message with send_message:\n{list}")
    };
    let system = format!("{AGENT_SYSTEM}{persona}{roster_block}{context_block}");

    // Tools: base file tools + send_message (has_peers = it has a roster).
    let (tools, _reg) = agent_tools_for_ex(app, Some(&agent.folder_path), !roster.is_empty());
    let pdf_cfg = pdf_config_for(app, Some(&agent.folder_path));

    // Resolve provider/model (recipient's own; fallback anthropic auto/haiku).
    let provider_kind = if agent.provider.is_empty() { "anthropic".to_string() } else { agent.provider.clone() };

    // Baseline checkpoint before any writes.
    if let Ok(root) = broker.root_for(agent_id) { let _ = checkpoint::snapshot(&root, "baseline"); }

    let mut messages = serde_json::json!([{ "role": "user", "content": framed }]);
    let mut reply_text = String::new();

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
            let (content, stop) = provider::anthropic_stream_turn(
                &key, &model, &system, &messages, &tools,
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
                            let (result_text, is_err) = if name == "send_message" {
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
                            } else {
                                exec_tool_cfg(broker, agent_id, &name, &input, &pdf_cfg)
                            };
                            tool_results.push(serde_json::json!({ "type": "tool_result", "tool_use_id": id, "content": result_text, "is_error": is_err }));
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
    } else if provider_kind == "openai" || provider_kind == "openrouter" {
        let key = keychain::get_key(&provider_kind).map_err(|_| format!("recipient has no {provider_kind} key"))?;
        if agent.model.trim().is_empty() { return Err("recipient has no model set".into()); }
        // Non-streaming completion (streaming tool-loop parity is a fast-follow).
        // Emit a visible "working" line so the pane isn't blank while it runs.
        let _ = app.emit(&stream_channel, &provider::StreamEvent::Info { text: "thinking…".into() });
        reply_text = openai_provider::complete(&provider_kind, &key, &agent.model, &framed).await.unwrap_or_default();
        // Emit the completed text as one delta so the pane shows it live.
        let _ = app.emit(&stream_channel, &provider::StreamEvent::TextDelta { text: reply_text.clone() });
    } else {
        reply_text = format!("({} runs a local model — headless inter-agent turns use a cloud provider for now.)", agent.name);
    }

    // Snapshot after writes.
    if let Ok(root) = broker.root_for(agent_id) { let _ = checkpoint::snapshot(&root, &format!("from {from_name}")); }

    // If the turn produced no text (e.g. it only called write_file), don't show
    // a blank bubble — summarize what it did so the user + sender see SOMETHING.
    // This fixes bug #5's "blank message back".
    let reply_display = if reply_text.trim().is_empty() {
        "(done — completed the request without a text reply)".to_string()
    } else { reply_text.trim().to_string() };

    // PERSIST to the recipient's conversation. CRITICAL (bug #5): we persist the
    // REAL provider-format `history` (messages), not just a display record — so
    // this exchange becomes actual conversational MEMORY. The inbox thread and
    // the agent's normal chat now share ONE conversation id so when you talk to
    // the agent directly it REMEMBERS the inter-agent message. We append the
    // full turn (framed inbound + assistant reply) to whatever history exists.
    let conv_id = format!("inbox-{agent_id}");
    let existing = repo::load_conversation(db, &conv_id).ok();
    let mut ui_msgs = existing.as_ref().and_then(|c| c.msgs.as_array().cloned()).unwrap_or_default();
    ui_msgs.push(serde_json::json!({ "role": "user", "from": from_name, "text": format!("\u{1F4E8} from {from_name}: {}", msg.body) }));
    ui_msgs.push(serde_json::json!({ "role": "assistant", "text": reply_display, "tools": [] }));
    // Real history: prior history + this turn's messages (framed user + all
    // assistant/tool turns we accumulated in `messages`). `messages` starts with
    // the framed user msg; append the whole thing to prior history.
    let mut hist = existing.as_ref().and_then(|c| c.history.as_array().cloned()).unwrap_or_default();
    if let Some(turn) = messages.as_array() { for m in turn { hist.push(m.clone()); } }
    let conv = repo::Conversation {
        id: conv_id, agent_id: agent_id.to_string(),
        title: "Inter-agent inbox".into(), updated: 0, pinned: true, order: 1,
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
pub fn persist_inbox_error(db: &writer::Db, agent_id: &str, msg: &mailbox::Message, err: &str) {
    let from_name = repo::get_agent(db, &msg.from_agent).ok().flatten().map(|a| a.name).unwrap_or_else(|| msg.from_agent.clone());
    let conv_id = format!("inbox-{agent_id}");
    let existing = repo::load_conversation(db, &conv_id).ok();
    let mut ui_msgs = existing.as_ref().and_then(|c| c.msgs.as_array().cloned()).unwrap_or_default();
    ui_msgs.push(serde_json::json!({ "role": "user", "from": from_name, "text": format!("\u{1F4E8} from {from_name}: {}", msg.body) }));
    ui_msgs.push(serde_json::json!({ "role": "assistant", "text": format!("⚠️ Couldn't process this message: {err}. (Check this agent has a provider key + model set.)"), "tools": [] }));
    let conv = repo::Conversation {
        id: conv_id, agent_id: agent_id.to_string(),
        title: "Inter-agent inbox".into(), updated: 0, pinned: true, order: 1,
        msgs: serde_json::json!(ui_msgs),
        history: existing.map(|c| c.history).unwrap_or(serde_json::json!([])),
    };
    let _ = repo::save_conversation(db, conv);
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

    // Per-session lanes (M1.1): serialize turn execution one-at-a-time per
    // session. Created here, managed as Tauri state, acquired in agent_stream.
    let lanes = lanes::Lanes::new();

    // M1.4 delivery engine: the signal the send path uses to WAKE the drainer
    // the instant a message is enqueued (near-instant delivery). Managed as
    // state so mailbox commands can nudge it.
    let drain_signal = drainer::DrainSignal::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(broker.clone())
        .manage(state.clone())
        .manage(lanes.clone())
        .manage(drain_signal.clone())
        .invoke_handler(tauri::generate_handler![
            daemon_info, pick_agent_folder, broker_probe,
            set_provider_key, has_provider_key, anthropic_test, anthropic_models, agent_run,
            agent_stream, reveal_in_finder, get_selected_model, set_selected_model,
            get_selection, set_selection, detect_hardware, local_catalog, local_downloaded,
            local_download, local_delete, local_tool_capability, restore_agent_folder,
            openai_models, tools_list, tools_upsert, tools_delete, tools_set_enabled,
            tools_config, tools_set_config,
            checkpoint_snapshot, checkpoint_timeline, checkpoint_rewind,
            checkpoint_undo, checkpoint_redo,
            checkpoint_get_retention, checkpoint_set_retention, checkpoint_purge,
            conv_list, conv_load, conv_save, conv_delete, conv_reorder,
            agents_list, agents_create, agents_update, agents_delete,
            agents_set_active, agents_get_active, agents_sharing_folder,
            agent_context_add, agent_context_list, agent_context_remove,
            agent_generate_soul,
            mailbox_pending_counts, mailbox_take_next, mailbox_roster,
            get_app_knobs, set_app_knobs,
            memory_ingest, memory_retrieve, memory_stats, pick_vault_folder
        ])
        .setup(move |_app| {
            // M1.1: bring up the SQLite state spine + single-writer actor, then
            // run the one-time JSON→SQLite import. Do this synchronously in
            // setup so every command that follows sees a ready DB. A failure
            // here is fatal — the app has no state without it.
            {
                use tauri::Manager;
                let app_data = _app.handle().path().app_data_dir()
                    .map_err(|e| format!("app_data_dir: {e}"))?;
                let db = writer::Db::start(app_data.clone())
                    .map_err(|e| format!("db init: {e}"))?;
                if let Err(e) = migrate_json::run(&db, &app_data) {
                    // Non-fatal: a bad legacy file shouldn't brick startup. Log
                    // and continue with whatever imported cleanly.
                    eprintln!("[aygent] JSON→SQLite migration warning: {e}");
                }
                _app.manage(db.clone());

                // M1.4 DELIVERY ENGINE: spawn the background drainer that runs
                // recipient inter-agent turns headlessly (independent of any
                // open chat pane). This is what makes Atlas→Copywriter→Atlas
                // actually execute. It nudges on send + polls as a safety net.
                {
                    use tauri::Manager;
                    let sig = _app.state::<drainer::DrainSignal>().inner().clone();
                    let brk = _app.state::<Arc<Broker>>().inner().clone();
                    let lns = _app.state::<lanes::Lanes>().inner().clone();
                    drainer::spawn(_app.handle().clone(), db, brk, lns, sig);
                }
            }

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
