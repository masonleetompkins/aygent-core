// AYGENT — Tauri shell entry (the privileged side).
// Owns: window, tray, keychain broker, the PATH BROKER (trust boundary),
// and the daemon supervisor that launches the Node daemon (on macOS, UNDER a
// Seatbelt profile that denies file+exec — Atlas C1). Also mints the
// per-session WS token (C6) and hands it + the daemon port to the UI.

mod broker;
mod supervisor;

use std::sync::Arc;
use rand::Rng;
use supervisor::DaemonState;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(DaemonState {
        ws_token: mint_ws_token(),
        ..Default::default()
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(broker::Broker::new())
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![daemon_info])
        .setup(move |_app| {
            // M0.1: launch the daemon so the UI<->daemon WS loop works.
            // jailed=false in dev (build the loop first); the macOS Seatbelt
            // jail (jailed=true) is finalized in M0.2.
            let jailed = false;
            if let Err(e) = supervisor::spawn_daemon(state.clone(), jailed) {
                eprintln!("[aygent] daemon spawn failed: {e}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AYGENT");
}
