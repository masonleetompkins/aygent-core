// AYGENT — Keychain (M0.3, Atlas C1) — VAULT EDITION (v1.0.1, Mason polish #1).
//
// API keys live in the macOS Keychain and NEVER cross into JS/WebView. The UI
// only ever sees "key set ✓". Keys stay Rust-side.
//
// WHY A VAULT: macOS pins each keychain item to the creating binary's cdhash;
// ad-hoc builds get a fresh cdhash every compile, so EVERY item re-prompts
// after EVERY build ("Always Allow" updates the app ACL, not the partition
// list). With ~10 separate items (providers + remote + connections) that was
// 4–5 password prompts per launch and prompts mid-shell-run. One item = ONE
// prompt per build; an in-memory cache makes it one prompt per PROCESS.
// (The per-build prompt itself dies when we sign with a stable Developer ID.)
//
// Shape: a single generic-password item (SERVICE/"vault") holding a JSON map
// { name: secret }. Legacy per-name items are migrated lazily: a get() that
// misses the vault falls back to the old item, absorbs it into the vault, and
// deletes the legacy entry — so each old secret prompts at most ONCE more.

use keyring::Entry;
use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "build.masonlee.aygent";
const VAULT_USER: &str = "vault";

/// In-memory vault cache. None = not yet loaded from the keychain.
static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn vault_entry() -> Result<Entry, String> {
    Entry::new(SERVICE, VAULT_USER).map_err(|e| format!("keychain entry: {e}"))
}

/// Load the vault into the cache (one keychain read per process). A missing
/// vault item is an empty map, NOT an error. Access-denied errors surface so
/// callers can distinguish "absent" from "locked out" (partition list).
fn load_cache(guard: &mut Option<HashMap<String, String>>) -> Result<(), String> {
    if guard.is_some() {
        return Ok(());
    }
    let map = match vault_entry()?.get_password() {
        Ok(json) => serde_json::from_str::<HashMap<String, String>>(&json)
            .map_err(|e| format!("vault parse: {e}"))?,
        Err(keyring::Error::NoEntry) => HashMap::new(),
        Err(e) => return Err(format!("keychain get [{SERVICE}/{VAULT_USER}]: {e}")),
    };
    *guard = Some(map);
    Ok(())
}

fn persist(map: &HashMap<String, String>) -> Result<(), String> {
    let json = serde_json::to_string(map).map_err(|e| format!("vault encode: {e}"))?;
    vault_entry()?
        .set_password(&json)
        .map_err(|e| format!("keychain set [{SERVICE}/{VAULT_USER}]: {e}"))
}

/// Store a secret under `provider` (e.g. "anthropic", "remote:meta"). Overwrites.
pub fn set_key(provider: &str, key: &str) -> Result<(), String> {
    let mut guard = CACHE.lock().map_err(|_| "vault lock poisoned".to_string())?;
    load_cache(&mut guard)?;
    let map = guard.as_mut().unwrap();
    map.insert(provider.to_string(), key.to_string());
    persist(map)
}

/// Fetch a stored secret (Rust-side only). Vault first; on miss, migrate any
/// legacy per-name item into the vault (prompts once, then never again).
pub fn get_key(provider: &str) -> Result<String, String> {
    let mut guard = CACHE.lock().map_err(|_| "vault lock poisoned".to_string())?;
    load_cache(&mut guard)?;
    let map = guard.as_mut().unwrap();
    if let Some(v) = map.get(provider) {
        return Ok(v.clone());
    }
    // Legacy migration: old builds stored one keychain item per name.
    let legacy = Entry::new(SERVICE, provider).map_err(|e| format!("keychain entry: {e}"))?;
    match legacy.get_password() {
        Ok(secret) => {
            map.insert(provider.to_string(), secret.clone());
            let _ = persist(map); // best effort; the read still succeeds
            let _ = legacy.delete_credential(); // absorbed — retire the old item
            Ok(secret)
        }
        Err(e) => Err(format!("keychain get [{SERVICE}/{provider}]: {e}")),
    }
}

/// Whether a key exists (safe for the UI — returns bool, never the secret).
pub fn has_key(provider: &str) -> bool {
    get_key(provider).is_ok()
}

/// Delete a stored secret (vault + any lingering legacy item).
pub fn delete_key(provider: &str) -> Result<(), String> {
    let mut guard = CACHE.lock().map_err(|_| "vault lock poisoned".to_string())?;
    load_cache(&mut guard)?;
    let map = guard.as_mut().unwrap();
    map.remove(provider);
    persist(map)?;
    if let Ok(legacy) = Entry::new(SERVICE, provider) {
        let _ = legacy.delete_credential();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // NOTE: these hit the real login keychain when run locally; they use a
    // dedicated test name and clean up after themselves.
    use super::*;

    #[test]
    fn vault_roundtrip_and_delete() {
        let name = "test:vault-roundtrip";
        set_key(name, "s3cret").expect("set");
        assert_eq!(get_key(name).expect("get"), "s3cret");
        assert!(has_key(name));
        delete_key(name).expect("delete");
        assert!(!has_key(name));
    }

    #[test]
    fn cache_survives_multiple_reads() {
        let name = "test:vault-cache";
        set_key(name, "v1").unwrap();
        for _ in 0..5 {
            assert_eq!(get_key(name).unwrap(), "v1");
        }
        delete_key(name).unwrap();
    }
}
