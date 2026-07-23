// AYGENT — Tauri shell entry (the privileged side).
// Owns: window, tray, keychain broker, the PATH BROKER (trust boundary),
// and the Seatbelt supervisor that launches the Node daemon UNDER a profile
// that denies file+exec (Atlas C1). Also mints the per-session WS token (C6).

mod broker;
mod supervisor;

use rand::Rng;

/// Mint a random per-session WS token (Atlas C6). Injected into the daemon via
/// env and into the WebView via Tauri state — the daemon rejects any WS
/// connection lacking it. localhost alone is NOT access control.
fn mint_ws_token() -> String {
    let mut rng = rand::thread_rng();
    (0..48)
        .map(|_| {
            let c = rng.gen_range(0..62);
            match c {
                0..=9 => (b'0' + c) as char,
                10..=35 => (b'a' + (c - 10)) as char,
                _ => (b'A' + (c - 36)) as char,
            }
        })
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let ws_token = mint_ws_token();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(broker::Broker::new())
        .setup({
            let ws_token = ws_token.clone();
            move |_app| {
                // Phase 0 (M0.1): spawn the Node daemon under the Seatbelt
                // profile, passing the WS token via env. supervisor::spawn
                // is the ONLY thing allowed to launch the daemon.
                #[cfg(target_os = "macos")]
                supervisor::spawn_daemon(&ws_token)?;
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = &ws_token; // non-mac dev: daemon launched manually
                    eprintln!("[aygent] non-macOS host: Seatbelt jail is a no-op; daemon must be run manually for UI dev only.");
                }
                Ok(())
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running AYGENT");
}
