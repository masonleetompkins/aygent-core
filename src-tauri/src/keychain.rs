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
    let entry = Entry::new(SERVICE, provider).map_err(|e| format!("keychain entry: {e}"))?;
    entry.get_password().map_err(|e| format!("keychain get [{SERVICE}/{provider}]: {e}"))
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
