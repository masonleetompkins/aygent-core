// AYGENT — Persistent browsing history (Atlas).
//
// THE PROBLEM this replaces: the Browser view's History was an IN-MEMORY,
// frontend-only list built off `go()` + a title poll. It (1) missed most real
// navigations (agent navigations, in-page link clicks, redirects — anything the
// FE didn't route through `go()`), so it showed EMPTY; (2) evaporated on every
// app restart; (3) had no way to clear it.
//
// THE FIX — capture at the SOURCE and persist:
//   * CAPTURE: every navigation is recorded from the point where the navigation
//     is actually OBSERVED/CONFIRMED, not from FE intent. The single robust
//     choke point is the CDP/webview layer:
//       - `mirror_visible_to_cdp` (browser.rs) runs after EVERY agent navigation
//         (browser_open/click/type) AND on the visible-tab url-change path, and
//         it already reads the authoritative CDP page url — so we record there.
//       - `webview_page_info` (browser.rs) is polled after every human `go()`
//         and every back/forward/reload, and returns the REAL committed
//         title+url — so we record there too (covers in-page nav + redirects a
//         human triggers, and backfills titles).
//       - `webview_navigate` / `browser_navigate` record their explicit target.
//     Consecutive identical URLs are de-duped (a reload / title backfill / a
//     re-poll shouldn't spam), and a bare title backfill for the same URL just
//     upgrades the stored title in place.
//   * PERSIST: a single JSON file at <app_data>/browser/history.json — the SAME
//     "app state lives in app_data, not the user's folder" rule settings.rs and
//     conversations follow (so nothing leaks into the vault / Save Points). It's
//     capped at MAX_ENTRIES (1000) newest-kept so the file can't grow unbounded.
//     It's loaded lazily on first access and re-read defensively, so reopening
//     the app shows prior history with no explicit boot step required.
//
// EVERY recorded entry logs `[aygent][browser][HIST]` so Mason can confirm in
// the terminal that each page he visits is captured.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Cap so history.json never grows unbounded. Newest entries are kept.
const MAX_ENTRIES: usize = 1000;

/// One visited page. `ts` is unix SECONDS (matches downloads' mtime unit so the
/// FE formats both the same way).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistEntry {
    pub url: String,
    #[serde(default)]
    pub title: String,
    pub ts: u64,
}

/// Process-wide in-memory cache of the history, guarded so concurrent
/// navigations (agent + human polls fire from different async tasks) never
/// race the file. `None` = not yet loaded from disk this run.
static HISTORY: Mutex<Option<Vec<HistEntry>>> = Mutex::new(None);

/// <app_data>/browser/history.json — app state, NOT the user's folder.
fn history_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(crate::browser::browser_dir(app)?.join("history.json"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Load history from disk into a Vec (oldest-first, as stored). Missing/corrupt
/// file = empty (never errors the UI over a history read).
fn load_from_disk(app: &tauri::AppHandle) -> Vec<HistEntry> {
    let path = match history_path(app) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<HistEntry>>(&t).ok())
        .unwrap_or_default()
}

/// Persist the current Vec to disk (whole-file write; it's small and capped).
fn save_to_disk(app: &tauri::AppHandle, entries: &[HistEntry]) {
    let path = match history_path(app) {
        Ok(p) => p,
        Err(e) => { eprintln!("[aygent][browser][HIST] path error: {e}"); return; }
    };
    match serde_json::to_string(entries) {
        Ok(text) => {
            if let Err(e) = std::fs::write(&path, text) {
                eprintln!("[aygent][browser][HIST] write error: {e}");
            }
        }
        Err(e) => eprintln!("[aygent][browser][HIST] serialize error: {e}"),
    }
}

/// Ensure the in-memory cache is populated from disk, then run `f` on it. The
/// lock is held across the closure so a read and a subsequent write can't
/// interleave with another navigation.
fn with_history<T>(app: &tauri::AppHandle, f: impl FnOnce(&mut Vec<HistEntry>) -> T) -> T {
    let mut guard = HISTORY.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        *guard = Some(load_from_disk(app));
    }
    f(guard.as_mut().expect("history cache populated above"))
}

/// RECORD a visited page. Called from every navigation-confirmation point in
/// browser.rs. De-dupes consecutive identical URLs; a title-only change for the
/// same latest URL upgrades the stored title in place instead of appending.
/// Non-http(s) urls (about:blank, chrome://, data:) are ignored. Best-effort:
/// a persistence failure is logged, never surfaced to the caller.
pub fn record(app: &tauri::AppHandle, url: &str, title: &str) {
    let url = url.trim();
    if url.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let title = title.trim().to_string();

    with_history(app, |entries| {
        // Consecutive-dupe handling against the NEWEST entry (stored last).
        if let Some(last) = entries.last_mut() {
            if last.url == url {
                // Same page again (reload / re-poll / mirror echo). If we now
                // have a better (non-empty, different) title, upgrade it in
                // place and persist; otherwise nothing changed — skip the write.
                if !title.is_empty() && title != last.title {
                    last.title = title.clone();
                    let snapshot = entries.clone();
                    eprintln!("[aygent][browser][HIST] title update url={url} title={title:?}");
                    save_to_disk(app, &snapshot);
                }
                return;
            }
        }

        entries.push(HistEntry { url: url.to_string(), title: title.clone(), ts: now_secs() });
        // Cap: keep only the newest MAX_ENTRIES (drop from the front/oldest).
        if entries.len() > MAX_ENTRIES {
            let overflow = entries.len() - MAX_ENTRIES;
            entries.drain(0..overflow);
        }
        let total = entries.len();
        let snapshot = entries.clone();
        eprintln!("[aygent][browser][HIST] record url={url} title={title:?} (total={total})");
        save_to_disk(app, &snapshot);
    });
}

/// LIST history NEWEST-FIRST for the History pane. Cheap: clones the cache
/// (loading from disk on first call this run) and reverses it.
pub fn list(app: &tauri::AppHandle) -> Vec<HistEntry> {
    with_history(app, |entries| {
        let mut out = entries.clone();
        out.reverse(); // stored oldest-first -> return newest-first
        out
    })
}

/// CLEAR all history (in-memory cache + on-disk file).
pub fn clear(app: &tauri::AppHandle) {
    with_history(app, |entries| {
        entries.clear();
        save_to_disk(app, entries);
    });
    eprintln!("[aygent][browser][HIST] cleared");
}
