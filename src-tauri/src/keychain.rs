// AYGENT — Keychain (M0.3, Atlas C1).
// API keys live in the macOS Keychain and NEVER cross into JS/WebView. The UI
// only ever sees "key set ✓". The daemon fetches secrets from the Rust broker
// at request time — but even the daemon never gets the raw key in Phase 0: the
// Rust side makes the provider call and streams tokens back. Keys stay Rust-side.

use keyring::Entry;

const SERVICE: &str = "build.masonlee.aygent";

/// Store a provider API key under `provider` (e.g. "anthropic"). Overwrites.
pub fn set_key(provider: &str, key: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, provider).map_err(|e| e.to_string())?;
    entry.set_password(key).map_err(|e| e.to_string())
}

/// Fetch a stored key (Rust-side only — used to make provider calls).
/// Surfaces the RAW keyring error (NoEntry vs access/decrypt denied) so callers
/// can distinguish "genuinely absent" from "present but this app identity can't
/// read it" (a macOS Keychain ACL / code-signing-identity mismatch — common in
/// unsigned `cargo tauri dev` builds).
pub fn get_key(provider: &str) -> Result<String, String> {
    read_password_warm(provider)
}

/// The FIRST macOS Keychain access in a fresh process can fail spuriously with
/// "Attribute user is invalid: cannot be empty" until the security session is
/// warmed — the entry is stored perfectly (verified via `security` CLI), it's
/// the session that isn't ready. Empirically, a subsequent read succeeds. So we
/// rebuild the Entry and retry a few times with a short blocking sleep before
/// giving up. This is why switching provider (a second keychain touch) "fixed"
/// Anthropic in the UI — the retry now lives here instead of racing in JS.
fn read_password_warm(provider: &str) -> Result<String, String> {
    let mut last: String = String::new();
    for i in 0..6 {
        let entry = match Entry::new(SERVICE, provider) {
            Ok(e) => e,
            Err(e) => {
                last = format!("keychain entry: {e}");
                std::thread::sleep(std::time::Duration::from_millis(120 * (i + 1)));
                continue;
            }
        };
        match entry.get_password() {
            Ok(pw) => return Ok(pw),
            // NoEntry = genuinely absent: don't waste retries, fail fast.
            Err(keyring::Error::NoEntry) => {
                return Err(format!("keychain get [{SERVICE}/{provider}]: no entry"));
            }
            Err(e) => {
                last = format!("keychain get [{SERVICE}/{provider}]: {e}");
                std::thread::sleep(std::time::Duration::from_millis(120 * (i + 1)));
            }
        }
    }
    Err(last)
}

/// Whether a key exists (safe for the UI — returns bool, never the secret).
pub fn has_key(provider: &str) -> bool {
    match Entry::new(SERVICE, provider) {
        Ok(e) => e.get_password().is_ok(),
        Err(_) => false,
    }
}

/// Delete a stored key.
pub fn delete_key(provider: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE, provider).map_err(|e| e.to_string())?;
    entry.delete_credential().map_err(|e| e.to_string())
}
