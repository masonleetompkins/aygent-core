// AYGENT REMOTE — Tauri commands for the Settings card (R5).
//
// The app-side servicing seam: pair with a code from masonlee.build, show
// status (paired? connected? SAS?), unpair. Thin over remote/remote_runtime —
// no protocol logic lives here.

use tauri::Manager;

/// Everything the Settings card renders, in one call.
#[derive(serde::Serialize)]
pub struct RemoteStatus {
    pub paired: bool,
    /// Realtime session currently up.
    pub running: bool,
    /// User's online/offline toggle (AgentRail): false = paired but silent.
    pub enabled: bool,
    /// Site the pairing points at (for display; empty when unpaired).
    pub site: String,
}

/// v2 account pairing: no SAS, no browser-link state — pairing is between
/// the ACCOUNT and this Mac; any logged-in browser just works.
#[tauri::command]
pub fn remote_status(app: tauri::AppHandle) -> RemoteStatus {
    RemoteStatus {
        paired: crate::remote::is_paired(),
        running: app.state::<crate::remote_runtime::RemoteRuntime>().is_running(),
        enabled: crate::remote::is_enabled(),
        site: crate::remote::load_meta().map(|m| m.site).unwrap_or_default(),
    }
}

/// Lightweight LOCAL status — no network. Safe for the AgentRail to poll.
/// (remote_status above fetches the device row for the SAS; polling that
/// would hammer the site.)
#[derive(serde::Serialize)]
pub struct RemoteLocalStatus {
    pub paired: bool,
    pub enabled: bool,
    pub running: bool,
}

#[tauri::command]
pub fn remote_local_status(app: tauri::AppHandle) -> RemoteLocalStatus {
    RemoteLocalStatus {
        paired: crate::remote::is_paired(),
        enabled: crate::remote::is_enabled(),
        running: app.state::<crate::remote_runtime::RemoteRuntime>().is_running(),
    }
}

/// The online/offline toggle (AgentRail). OFF: keep the pairing, tear down
/// the Realtime session entirely — no socket, no heartbeat. ON: bring it
/// back up (autostart loop handles the browser-key wait).
#[tauri::command]
pub fn remote_set_enabled(app: tauri::AppHandle, on: bool) {
    crate::remote::set_enabled(on);
    if on {
        crate::remote_runtime::spawn_autostart(app);
    } else {
        app.state::<crate::remote_runtime::RemoteRuntime>().shutdown();
    }
}

/// Pair this Mac with a code from masonlee.build/remote. On success, tries to
/// start the runtime immediately (it may wait for the browser key — fine).
#[tauri::command]
pub async fn remote_pair(app: tauri::AppHandle, code: String) -> Result<(), String> {
    let site = std::env::var("AYGENT_REMOTE_SITE")
        .unwrap_or_else(|_| "https://www.masonlee.build".to_string());
    let device_name = hostname_or_default();
    crate::remote::pair(&site, &code, &device_name).await?;
    // The browser publishes its key AFTER this claim succeeds — a one-shot
    // start would always miss on first pair. The autostart loop retries
    // until the key appears and the session is up.
    crate::remote_runtime::spawn_autostart(app);
    Ok(())
}

/// Unpair: tell the site to drop the device row (best effort), stop the
/// runtime, wipe local keys. Local wipe happens even if the site is down.
#[tauri::command]
pub async fn remote_unpair(app: tauri::AppHandle) -> Result<(), String> {
    if let (Some(meta), Some(jwt)) = (crate::remote::load_meta(), crate::remote::load_jwt()) {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            let _ = client
                .delete(format!("{}/api/remote/device", meta.site))
                .bearer_auth(&jwt)
                .send()
                .await;
        }
    }
    app.state::<crate::remote_runtime::RemoteRuntime>().shutdown();
    crate::remote::unpair();
    Ok(())
}

/// (Re)start the Realtime session — Settings' "Connect now" after the browser
/// side has published its key, without a full re-pair.
#[tauri::command]
pub async fn remote_connect(app: tauri::AppHandle) -> Result<bool, String> {
    crate::remote_runtime::start_if_paired(app).await
}

fn hostname_or_default() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "My Mac".to_string())
}
