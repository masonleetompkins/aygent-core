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
//
// Where the vault lives (mod store): the OS keychain item on macOS / Linux.
// On Windows, a file in %APPDATA%\build.masonlee.aygent sealed with DPAPI (only
// this Windows user can open it): Credential Manager caps an item at 2560
// bytes, which one Codex subscription's tokens alone outgrow.

use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "build.masonlee.aygent";
const VAULT_USER: &str = "vault";

/// In-memory vault cache. None = not yet loaded from the keychain.
static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

#[cfg(not(windows))]
mod store {
    use super::{SERVICE, VAULT_USER};
    use keyring::Entry;

    fn vault_entry() -> Result<Entry, String> {
        Entry::new(SERVICE, VAULT_USER).map_err(|e| format!("keychain entry: {e}"))
    }

    /// The vault JSON; None when nothing is stored yet.
    pub fn read() -> Result<Option<String>, String> {
        match vault_entry()?.get_password() {
            Ok(json) => Ok(Some(json)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("keychain get [{SERVICE}/{VAULT_USER}]: {e}")),
        }
    }

    pub fn write(json: &str) -> Result<(), String> {
        vault_entry()?
            .set_password(json)
            .map_err(|e| format!("keychain set [{SERVICE}/{VAULT_USER}]: {e}"))
    }

    /// A secret old builds stored as its own item (before the vault).
    pub fn legacy_get(name: &str) -> Result<String, String> {
        let legacy = Entry::new(SERVICE, name).map_err(|e| format!("keychain entry: {e}"))?;
        legacy.get_password().map_err(|e| format!("keychain get [{SERVICE}/{name}]: {e}"))
    }

    pub fn legacy_delete(name: &str) {
        if let Ok(legacy) = Entry::new(SERVICE, name) {
            let _ = legacy.delete_credential();
        }
    }
}

#[cfg(windows)]
mod store {
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    const ENTROPY: &[u8] = b"build.masonlee.aygent/vault";

    fn path() -> Result<PathBuf, String> {
        dirs::data_dir()
            .map(|d| d.join(super::SERVICE).join("vault.dpapi"))
            .ok_or_else(|| "vault: no AppData folder".to_string())
    }

    fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr() as *mut u8 }
    }

    /// Copy out a blob DPAPI allocated, and free it.
    fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        v
    }

    /// The vault JSON; None when nothing is stored yet.
    pub fn read() -> Result<Option<String>, String> {
        let p = path()?;
        let sealed = match std::fs::read(&p) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("vault read {}: {e}", p.display())),
        };
        let (input, entropy) = (blob(&sealed), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(format!("vault decrypt: {}", std::io::Error::last_os_error()));
        }
        String::from_utf8(take(out)).map(Some).map_err(|_| "vault decrypt: not UTF-8".to_string())
    }

    pub fn write(json: &str) -> Result<(), String> {
        let (input, entropy) = (blob(json.as_bytes()), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        };
        if ok == 0 {
            return Err(format!("vault encrypt: {}", std::io::Error::last_os_error()));
        }
        let sealed = take(out);
        let p = path()?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("vault dir {}: {e}", dir.display()))?;
        }
        // Whole file or nothing: write aside, then replace.
        let tmp = p.with_extension("dpapi.tmp");
        std::fs::write(&tmp, &sealed).map_err(|e| format!("vault write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("vault write {}: {e}", p.display()))
    }

    /// Windows builds never stored secrets outside the vault.
    pub fn legacy_get(name: &str) -> Result<String, String> {
        Err(format!("keychain get [{}/{name}]: not stored", super::SERVICE))
    }

    pub fn legacy_delete(_name: &str) {}
}

/// Load the vault into the cache (one keychain read per process). A missing
/// vault item is an empty map, NOT an error. Access-denied errors surface so
/// callers can distinguish "absent" from "locked out" (partition list).
fn load_cache(guard: &mut Option<HashMap<String, String>>) -> Result<(), String> {
    if guard.is_some() {
        return Ok(());
    }
    let map = match store::read()? {
        Some(json) => serde_json::from_str::<HashMap<String, String>>(&json)
            .map_err(|e| format!("vault parse: {e}"))?,
        None => HashMap::new(),
    };
    *guard = Some(map);
    Ok(())
}

fn persist(map: &HashMap<String, String>) -> Result<(), String> {
    let json = serde_json::to_string(map).map_err(|e| format!("vault encode: {e}"))?;
    store::write(&json)
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
    let secret = store::legacy_get(provider)?;
    map.insert(provider.to_string(), secret.clone());
    let _ = persist(map); // best effort; the read still succeeds
    store::legacy_delete(provider); // absorbed — retire the old item
    Ok(secret)
}

/// UI-visible vault state (Mason 10-08): "ready" (readable, N secrets),
/// "empty" (readable, nothing stored yet), or "locked" (macOS denied THIS
/// build access — every unsigned rebuild is a new app identity until we sign
/// with a stable Developer ID; allowing the OS prompt once heals it). The
/// read itself triggers the OS prompt when needed, so a "Try again" button
/// that re-invokes this is the whole fix. Fresh read, bypasses the cache.
#[tauri::command]
pub fn keychain_status() -> serde_json::Value {
    match store::read() {
        Err(e) => serde_json::json!({ "state": "locked", "error": e }),
        Ok(None) => serde_json::json!({ "state": "empty", "items": 0 }),
        Ok(Some(json)) => {
            let n = serde_json::from_str::<HashMap<String, String>>(&json)
                .map(|m| m.len())
                .unwrap_or(0);
            if n == 0 {
                serde_json::json!({ "state": "empty", "items": 0 })
            } else {
                serde_json::json!({ "state": "ready", "items": n })
            }
        }
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
    store::legacy_delete(provider);
    Ok(())
}

#[cfg(test)]
mod tests {
    // NOTE: these hit the real login keychain when run locally; they use a
    // dedicated test name and clean up after themselves.
    // On headless CI (no Secret Service on Linux) the platform store is
    // unreachable — probe first and skip gracefully instead of failing the
    // whole suite. A broken store on a machine that HAS one still fails loud.
    use super::*;

    fn store_available() -> bool {
        const PROBE: &str = "test:store-availability-probe";
        match set_key(PROBE, "x") {
            Ok(()) => {
                let _ = delete_key(PROBE);
                true
            }
            Err(e) => {
                let eu = e.to_lowercase();
                if eu.contains("dbus")
                    || eu.contains("secret service")
                    || eu.contains("platform secure storage")
                    || eu.contains("no such")
                {
                    eprintln!("keychain tests SKIPPED: no platform store ({e})");
                    false
                } else {
                    true
                }
            }
        }
    }

    #[test]
    fn vault_roundtrip_and_delete() {
        if !store_available() {
            return;
        }
        let name = "test:vault-roundtrip";
        set_key(name, "s3cret").expect("set");
        assert_eq!(get_key(name).expect("get"), "s3cret");
        assert!(has_key(name));
        delete_key(name).expect("delete");
        assert!(!has_key(name));
    }

    #[test]
    fn large_secret_roundtrip() {
        if !store_available() {
            return;
        }
        // Subscription tokens (JWTs) run to kilobytes: bigger than one
        // Windows Credential Manager item allows.
        let name = "test:vault-large";
        let big = "x".repeat(8000);
        set_key(name, &big).expect("set");
        *CACHE.lock().unwrap() = None; // read back from the store, not the cache
        assert_eq!(get_key(name).expect("get"), big);
        delete_key(name).expect("delete");
    }

    #[test]
    fn cache_survives_multiple_reads() {
        if !store_available() {
            return;
        }
        let name = "test:vault-cache";
        set_key(name, "v1").unwrap();
        for _ in 0..5 {
            assert_eq!(get_key(name).unwrap(), "v1");
        }
        delete_key(name).unwrap();
    }
}
