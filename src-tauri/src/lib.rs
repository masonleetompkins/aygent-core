// AYGENT — Tauri shell entry (the privileged side).
// Owns: window, tray, keychain broker, the PATH BROKER (trust boundary),
// and the daemon supervisor that launches the Node daemon (on macOS, UNDER a
// Seatbelt profile that denies file+exec — Atlas C1). Also mints the
// per-session WS token (C6) and hands it + the daemon port to the UI.

mod broker;
mod broker_ws;
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
        .invoke_handler(tauri::generate_handler![daemon_info, pick_agent_folder, broker_probe])
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
                        let jailed = false;
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
