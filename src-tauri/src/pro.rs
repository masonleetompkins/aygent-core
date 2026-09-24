// PRO HOOK (open core) — free tier, always.
//
// This file is a PUBLIC STUB. Pro builds (aygent-pro, private) REPLACE it at
// assemble time with the real entitlement implementation (license check vs the
// billing backend, tier unlocks, managed-model routing). Nothing proprietary
// ever lives in this repo; CI byte-verifies the stub is intact on main.
//
// Contract both sides honor: module path `crate::pro`, functions `is_pro()`
// and `tier_label()` with exactly these signatures.

/// Core builds are never Pro.
pub fn is_pro() -> bool {
    false
}

/// Human-readable tier for UI badges and logs.
pub fn tier_label() -> &'static str {
    "Core"
}

/// UI-callable tier snapshot: `{ tier, pro }`. Always Core here.
#[tauri::command]
pub fn pro_status() -> serde_json::Value {
    serde_json::json!({ "tier": tier_label(), "pro": is_pro() })
}

/// UI-callable refresh. Core has no backend: always false, never blocks.
#[tauri::command]
pub async fn pro_refresh() -> bool {
    is_pro()
}

// Keychain slot for the site session token. The OVERLAY agrees on this exact
// string (its own const) — sign-in writes here, entitlement reads here.
pub const SESSION_KEY: &str = "session";

/// Where the desktop sign-in code comes from (UI opens it in the browser).
#[tauri::command]
pub fn signin_start() -> serde_json::Value {
    serde_json::json!({ "url": "https://masonlee.build/desktop-signin" })
}

/// Complete sign-in with the one-time code from /desktop-signin. Exchanges it
/// for a virtual key and stores it in the OS keychain. Same code path in Pro
/// builds (overlay keeps these commands; only refresh_entitlement differs).
#[tauri::command]
pub async fn signin_complete(code: String) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let v: serde_json::Value = client
        .post("https://masonlee.build/api/desktop-signin/exchange")
        .json(&serde_json::json!({ "code": code }))
        .send()
        .await
        .map_err(|e| format!("exchange: {e}"))?
        .json()
        .await
        .map_err(|e| format!("exchange body: {e}"))?;
    if v.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        return Err(v
            .get("error")
            .and_then(|s| s.as_str())
            .unwrap_or("exchange rejected")
            .to_string());
    }
    let key = v
        .get("api_key")
        .and_then(|s| s.as_str())
        .ok_or("no api_key in response")?;
    crate::keychain::set_key(SESSION_KEY, key)?;
    Ok(serde_json::json!({ "ok": true }))
}

/// Sign out: forget the session token locally. (Server revocation = delete the
/// key row; a revoke-all affordance belongs on the account page, post-launch.)
#[tauri::command]
pub fn signout() -> Result<serde_json::Value, String> {
    crate::keychain::delete_key(SESSION_KEY)?;
    Ok(serde_json::json!({ "ok": true }))
}


/// Open a URL in the system browser (used by sign-in; WebView must not navigate away).
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://masonlee.build/") || url.starts_with("https://aygent.masonlee.build/")) {
        return Err("refusing to open off-site URL".into());
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&url).status();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .status();
    #[cfg(target_os = "linux")]
    let r = std::process::Command::new("xdg-open").arg(&url).status();
    r.map(|_| ()).map_err(|e| format!("open failed: {e}"))
}