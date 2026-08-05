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
    /// Realtime session currently up (browser may still be offline).
    pub running: bool,
    /// Site the pairing points at (for display; empty when unpaired).
    pub site: String,
    /// 6-digit SAS to compare against the browser, once both keys exist.
    pub sas: Option<String>,
    /// Browser has published its key (SAS is meaningful).
    pub browser_linked: bool,
}

#[tauri::command]
pub async fn remote_status(app: tauri::AppHandle) -> Result<RemoteStatus, String> {
    let meta = crate::remote::load_meta();
    let paired = crate::remote::is_paired();
    let running = app.state::<crate::remote_runtime::RemoteRuntime>().is_running();

    // SAS needs the browser pubkey from the device row; fetch when paired.
    let (mut sas, mut browser_linked) = (None, false);
    if let (Some(m), Some(jwt)) = (&meta, crate::remote::load_jwt()) {
        if let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            if let Ok(resp) = client
                .get(format!("{}/api/remote/device", m.site))
                .bearer_auth(&jwt)
                .send()
                .await
            {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    let dev = v.get("device").cloned().unwrap_or_default();
                    let dev_pub = dev.get("device_pubkey").and_then(|k| k.as_str()).unwrap_or("");
                    let web_pub = dev.get("browser_pubkey").and_then(|k| k.as_str()).unwrap_or("");
                    if !web_pub.is_empty() && !dev_pub.is_empty() {
                        browser_linked = true;
                        sas = Some(crate::remote::sas_code(dev_pub, web_pub));
                    }
                }
            }
        }
    }

    Ok(RemoteStatus {
        paired,
        running,
        site: meta.map(|m| m.site).unwrap_or_default(),
        sas,
        browser_linked,
    })
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
