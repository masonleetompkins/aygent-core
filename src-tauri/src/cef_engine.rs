//! ============================================================================
//! AYGENT — CEF engine (Phase 1). The VISIBLE browser surface, native Chromium.
//! ============================================================================
//!
//! Compiled ONLY with `--features engine-cef`. When the feature is off this
//! whole module is excluded (see `mod cef_engine` gate in lib.rs) and the
//! WKWebView path in browser.rs is used verbatim — that is the kill-switch.
//!
//! WHAT THIS MODULE OWNS
//! ---------------------
//! * `init()` — one-time CEF bring-up (framework load, execute_process for the
//!   browser process, initialize with our Settings, remote-debugging port).
//!   Called from lib.rs's Tauri `setup` BEFORE any browser is created.
//! * `AygentClient` + handler wrappers (life-span / display / download /
//!   request) built with cef-rs's `wrap_*!` macros — the EXACT macro API the
//!   Phase-0 spike used (see cefsimple/shared/simple_handler).
//! * A global registry of one CEF `Browser` per TAB, keyed by tab id, so
//!   multi-tab works like the WKWebView version (open / navigate / back /
//!   forward / reload / close / switch).
//! * Native downloads via `CefDownloadHandler` → land in <agentFolder>/downloads
//!   and emit `browser:download` to the FE. This is the concrete fix for the
//!   whole WKWebView right-click-Save-Image download saga.
//! * Title/address change → persistent history (history.rs) + `browser:tab-navigated`.
//!
//! WHY WINDOWED, NOT OSR
//! ---------------------
//! We use `WindowInfo::set_as_child(parent_nsview, bounds)` — CEF renders into
//! a child NSView of an AYGENT-owned wrapper view (see cef_geometry.rs). This is
//! native-quality GPU rendering (real Chromium compositor), NOT off-screen
//! rendering. `set_as_windowless` would force OSR + a RenderHandler + manual
//! paint; we deliberately do NOT do that (mission: native quality).
//!
//! THE HARD SEAM (documented for Mason): CEF on macOS wants NSApp to be a
//! `CefAppProtocol`-conforming NSApplication subclass (cefsimple's
//! SimpleApplication). Tauri/Wry ALREADY create + own NSApp before our setup
//! runs, so we CANNOT swap the app class. The workaround we use: run CEF with
//! `multi_threaded_message_loop = true` so CEF spins its OWN internal message
//! loop/thread and does NOT require ownership of the main Cocoa run loop or the
//! NSApplication subclass event pumping. All CEF object calls that must run on
//! CEF's UI thread are posted via `post_task(ThreadId::UI, ...)`. This is the
//! single most likely place to hit a first-run crash; see README-CEF risks.

#![cfg(all(target_os = "macos", feature = "engine-cef"))]

use cef::{args::Args, rc::*, *};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Emitter};

/// The CEF DevTools remote-debugging port the agent-automation CDP client
/// attaches to. Chosen once at init from a free ephemeral port so human + agent
/// share ONE Chromium instance (the desync fix). 0 = not initialized.
static CEF_CDP_PORT: AtomicU16 = AtomicU16::new(0);

/// Set once CEF `initialize` succeeded, so double-init is a no-op and the rest
/// of the app can query "is the CEF engine live?".
static CEF_READY: AtomicBool = AtomicBool::new(false);

/// The live CEF library loader + delegate must outlive the process; parked here.
/// We never touch them again after init — they just must not Drop.
static CEF_KEEPALIVE: OnceLock<CefKeepAlive> = OnceLock::new();

struct CefKeepAlive {
    _library: library_loader::LibraryLoader,
}
// SAFETY: the loader is only ever created once, on the main thread at setup,
// and never moved/accessed again — it exists solely to keep the framework
// mapped for the process lifetime.
unsafe impl Send for CefKeepAlive {}
unsafe impl Sync for CefKeepAlive {}

/// The CDP port the agent CDP client should connect to. `None` until CEF init.
pub fn cdp_port() -> Option<u16> {
    match CEF_CDP_PORT.load(Ordering::SeqCst) {
        0 => None,
        p => Some(p),
    }
}

/// Is the CEF engine initialized + ready to host browsers?
pub fn is_ready() -> bool {
    CEF_READY.load(Ordering::SeqCst)
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9222)
}

// ============================================================================
// GLOBAL BROWSER REGISTRY — one CEF Browser per tab id.
// ============================================================================
//
// CEF `Browser` is a ref-counted handle (`Rc`-flavored via cef-rs). We hold one
// per tab so navigation/back/forward/reload/close target the right tab. All
// mutation of a Browser MUST happen on CEF's UI thread; the public commands in
// this module post to that thread. The map itself is guarded by a Mutex.
//
// We also stash the app-owned wrapper NSView pointer per tab (see cef_geometry)
// so we can resize/hide/hittest exactly that tab's surface.

thread_local! {
    // CEF Browser handles are NOT Send (they're CEF-UI-thread objects), so the
    // authoritative map lives thread-local ON THE CEF UI THREAD. Commands hop
    // onto that thread via post_task and operate here. `by_tab` maps tab id ->
    // Browser.
    static BROWSERS: RefCell<HashMap<i64, Browser>> = RefCell::new(HashMap::new());
}

/// A pending "create a browser for this tab" request, handed to the UI thread.
struct PendingCreate {
    tab_id: i64,
    url: String,
    parent_view: usize, // *mut NSView as usize (Send across the post_task hop)
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

// The AppHandle so handlers can emit events + write history. Set at init.
static APP: OnceLock<AppHandle> = OnceLock::new();

fn app() -> Option<&'static AppHandle> {
    APP.get()
}

// ============================================================================
// INIT — one-time bring-up. Call from lib.rs setup on the MAIN thread.
// ============================================================================
/// EARLY init — MUST be called at the very top of run(), BEFORE tauri::Builder
/// takes over the macOS app lifecycle. On macOS CefInitialize has to run before
/// NSApp's run loop starts; calling it from Tauri's setup (which fires inside
/// applicationDidFinishLaunching, AFTER tao created + started NSApp) makes
/// CefInitialize return 0 and panic in a non-unwinding Obj-C frame (the crash we
/// hit). This does the framework load + execute_process + CefInitialize; the
/// AppHandle-dependent wiring is done later in attach_app().
///
/// Returns true if CEF initialized (engine usable), false on failure — NEVER
/// panics, so a CEF failure degrades gracefully instead of aborting the app.
pub fn init_early() -> bool {
    if CEF_READY.load(Ordering::SeqCst) {
        return true;
    }
    // Load the CEF framework (browser-process resolver: ../Frameworks).
    let loader =
        library_loader::LibraryLoader::new(&std::env::current_exe().unwrap(), false);
    if !loader.load() {
        eprintln!("[aygent][cef] framework load FAILED — browser will fall back");
        return false;
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let port = free_port();
    CEF_CDP_PORT.store(port, Ordering::SeqCst);
    eprintln!("[aygent][cef] init_early: remote-debugging-port={port}");

    let args = Args::new();
    let mut app_obj = AygentApp::new(port);
    // Browser process: execute_process returns -1 for the browser process.
    let ret = execute_process(Some(args.as_main_args()), Some(&mut app_obj), std::ptr::null_mut());
    if ret != -1 {
        eprintln!("[aygent][cef] execute_process returned {ret} in main binary (unexpected)");
        return false;
    }

    // MACOS BUNDLE PATHS — THE FIX for silent CefInitialize -> 0. On a bundled
    // macOS CEF app, CEF cannot find its framework/resources/helper-subprocess
    // relative to the main binary automatically; unset paths => silent failure
    // (returns 0, no log). We resolve all five from the running executable's
    // location inside AYGENT.app. Layout (from cef-bundle.sh):
    //   AYGENT.app/Contents/MacOS/AYGENT                          <- current_exe
    //   AYGENT.app/Contents/Frameworks/
    //     Chromium Embedded Framework.framework/
    //       Chromium Embedded Framework                           <- fw binary
    //       Resources/                                            <- .pak/icudtl/locales
    //     AYGENT Helper.app/Contents/MacOS/AYGENT Helper          <- subprocess
    let exe = std::env::current_exe().unwrap_or_default();
    // .../Contents/MacOS/AYGENT -> .../Contents
    let contents = exe.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_default();
    let frameworks = contents.join("Frameworks");
    let fw_dir = frameworks.join("Chromium Embedded Framework.framework");
    let resources = fw_dir.join("Resources");
    let locales = resources.join("locales");
    let helper = frameworks
        .join("AYGENT Helper.app")
        .join("Contents").join("MacOS").join("AYGENT Helper");
    // .app bundle root = .../Contents/.. = AYGENT.app
    let main_bundle = contents.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let s = |p: &std::path::Path| CefString::from(p.to_string_lossy().as_ref());
    eprintln!("[aygent][cef] paths: bundle={} fw={} helper={} res={}",
        main_bundle.display(), fw_dir.display(), helper.display(), resources.display());

    let settings = Settings {
        no_sandbox: !cfg!(feature = "sandbox") as _,
        // macOS: multi_threaded_message_loop is Windows-only (setting it made
        // CefInitialize fail silently -> 0). We use EXTERNAL MESSAGE PUMP: CEF
        // calls on_schedule_message_pump_work() when it has queued work, and we
        // pump do_message_loop_work() on the main GCD queue (see the browser
        // process handler). This is what makes posted UI-thread tasks (browser
        // creation) actually run while tao owns the main run loop.
        multi_threaded_message_loop: 0,
        external_message_pump: 1,
        remote_debugging_port: port as _,
        browser_subprocess_path: s(&helper),
        framework_dir_path: s(&fw_dir),
        main_bundle_path: s(&main_bundle),
        resources_dir_path: s(&resources),
        locales_dir_path: s(&locales),
        ..Default::default()
    };

    let ok = initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app_obj),
        std::ptr::null_mut(),
    );
    if ok != 1 {
        eprintln!("[aygent][cef] CefInitialize returned {ok} (expected 1) — CEF unavailable, browser falls back to WKWebView path");
        return false;
    }

    let _ = CEF_KEEPALIVE.set(CefKeepAlive { _library: loader });
    CEF_READY.store(true, Ordering::SeqCst);
    eprintln!("[aygent][cef] initialize OK — engine ready (multi_threaded_message_loop)");
    true
}

/// LATE wiring — stash the AppHandle + downloads dir once Tauri is up (called
/// from setup). No CEF lifecycle calls here, so it's safe inside
/// applicationDidFinishLaunching.
pub fn attach_app(app_handle: AppHandle, downloads_dir: std::path::PathBuf) {
    let _ = APP.set(app_handle);
    let _ = DOWNLOADS_DIR.set(Mutex::new(downloads_dir));
    eprintln!("[aygent][cef] attach_app: AppHandle + downloads dir wired (cef_ready={})", CEF_READY.load(Ordering::SeqCst));
}

/// Shutdown CEF cleanly (best-effort). Called on app exit.
pub fn shutdown_engine() {
    if !CEF_READY.swap(false, Ordering::SeqCst) {
        return;
    }
    eprintln!("[aygent][cef] shutdown");
    shutdown();
}

// ============================================================================
// THE APP OBJECT — provides the browser-process handler which, on context
// init, we DON'T use to auto-create a browser (we create per-tab on demand).
// We DO use `on_before_command_line_processing` to inject anti-detection +
// download-friendly Chromium switches, mirroring the WKWebView-era CDP flags.
// ============================================================================
wrap_app! {
    pub struct AygentApp {
        cdp_port: u16,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cmd) = command_line else { return };
            // Anti-automation-detection (same intent as the WKWebView CDP flags).
            cmd.append_switch(Some(&CefString::from("disable-blink-features")));
            // The above appends a bare switch; set its value form too.
            cmd.append_switch_with_value(
                Some(&CefString::from("disable-blink-features")),
                Some(&CefString::from("AutomationControlled")),
            );
            // Remote debugging on our chosen port (belt; also set in Settings).
            cmd.append_switch_with_value(
                Some(&CefString::from("remote-debugging-port")),
                Some(&CefString::from(self.cdp_port.to_string().as_str())),
            );
            cmd.append_switch_with_value(
                Some(&CefString::from("remote-debugging-address")),
                Some(&CefString::from("127.0.0.1")),
            );
            eprintln!("[aygent][cef] command-line switches injected (cdp={})", self.cdp_port);
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(AygentBrowserProcessHandler::new())
        }
    }
}

wrap_browser_process_handler! {
    struct AygentBrowserProcessHandler {}

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            eprintln!("[aygent][cef] context initialized");
            // Nothing to create here — browsers are created per-tab on demand
            // from the FE (webview_open path -> create_browser_for_tab).
        }

        // EXTERNAL MESSAGE PUMP (the fix for the never-created browser). With
        // external_message_pump:1, CEF calls this whenever it has queued work
        // (including our post_task create-jobs), asking us to run
        // do_message_loop_work() on the MAIN thread after ~delay_ms. tao owns
        // the main run loop, so we can't call CefRunMessageLoop; instead we
        // schedule the pump onto the main GCD queue. `delay_ms` is an
        // optimization hint — pumping promptly is always correct, so we
        // dispatch async to the main queue (dispatch2, already a dep). This is
        // what makes create_or_navigate_ui actually execute.
        fn on_schedule_message_pump_work(&self, _delay_ms: i64) {
            use dispatch2::DispatchQueue;
            DispatchQueue::main().exec_async(|| {
                do_message_loop_work();
            });
        }
    }
}

// ============================================================================
// THE CLIENT — one shared client for all browsers. Provides the handlers.
// ============================================================================
wrap_client! {
    pub struct AygentClient {}

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(AygentLifeSpanHandler::new())
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(AygentDisplayHandler::new())
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(AygentDownloadHandler::new())
        }
    }
}

wrap_life_span_handler! {
    struct AygentLifeSpanHandler {}

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                eprintln!("[aygent][cef] browser created id={}", b.identifier());
            }
        }
        // Allow the OS/CEF close to proceed (we manage lifetime via close_browser).
        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 {
            0
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(b) = browser {
                eprintln!("[aygent][cef] browser closed id={}", b.identifier());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DISPLAY HANDLER: title + address changes -> tab label + persistent history.
// ---------------------------------------------------------------------------
wrap_display_handler! {
    struct AygentDisplayHandler {}

    impl DisplayHandler {
        fn on_address_change(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            // MAIN-FRAME ONLY (matches WKWebView on_page_load main-frame rule):
            // ignore sub-frame address changes so iframes/captcha widgets don't
            // pollute history or the tab label.
            let is_main = frame.map(|f| f.is_main() != 0).unwrap_or(true);
            if !is_main { return; }
            let url = url.map(|u| u.to_string()).unwrap_or_default();
            if !(url.starts_with("http://") || url.starts_with("https://")) { return; }
            let tab = browser.and_then(browser_tab_id);
            eprintln!("[aygent][cef] url loaded tab={tab:?} url={url}");
            if let Some(app) = app() {
                crate::history::record(app, &url, "");
                emit_tab_navigated(app, tab, &url, "");
                if let Some(id) = tab { crate::browser::note_active_tab_url(id, &url); }
            }
        }

        fn on_title_change(
            &self,
            browser: Option<&mut Browser>,
            title: Option<&CefString>,
        ) {
            let t = title.map(|t| t.to_string()).unwrap_or_default();
            let t = t.trim().to_string();
            if t.is_empty() { return; }
            let tab = browser.and_then(browser_tab_id);
            if let Some(app) = app() {
                emit_tab_title(app, tab, &t);
                // Upgrade the newest history entry's title in place.
                // history::record de-dupes same-url + upgrades title.
                if let Some(id) = tab {
                    let url = crate::browser::last_tab_url(id);
                    if !url.is_empty() { crate::history::record(app, &url, &t); }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DOWNLOAD HANDLER — THE native downloads win. CEF is NOT under WebKit's
// networking sandbox, so a right-click Save Image / a download link / a
// download button just works and lands in <agentFolder>/downloads.
// ---------------------------------------------------------------------------
static DOWNLOADS_DIR: OnceLock<Mutex<std::path::PathBuf>> = OnceLock::new();

fn downloads_dir() -> std::path::PathBuf {
    DOWNLOADS_DIR
        .get()
        .and_then(|m| m.lock().ok().map(|g| g.clone()))
        .unwrap_or_else(|| std::env::temp_dir())
}

/// Update the download destination when the active agent (and thus its folder)
/// changes. Called from browser.rs when the active agent switches.
pub fn set_downloads_dir(dir: std::path::PathBuf) {
    if let Some(m) = DOWNLOADS_DIR.get() {
        if let Ok(mut g) = m.lock() { *g = dir; }
    }
}

wrap_download_handler! {
    struct AygentDownloadHandler {}

    impl DownloadHandler {
        // Called when a download begins. We must call the callback with the
        // chosen destination + continue. `download_path` is our jailed dir.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let name = suggested_name.map(|n| n.to_string()).unwrap_or_else(|| "download".into());
            let dir = downloads_dir();
            let _ = std::fs::create_dir_all(&dir);
            let dest = dir.join(sanitize(&name));
            eprintln!("[aygent][cef] download begin name={name:?} -> {}", dest.display());
            if let Some(app) = app() {
                // `state:"begin"` makes the FE auto-open the Downloads panel
                // (matches the WKWebView path's browser:download shape).
                let _ = app.emit("browser:download", &serde_json::json!({
                    "state": "begin", "name": name,
                }));
            }
            if let Some(cb) = callback {
                let dest_s = CefString::from(dest.to_string_lossy().as_ref());
                // show_dialog = 0 (no Save panel — we chose the path).
                cb.cont(Some(&dest_s), 0);
            }
            let _ = download_item;
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else { return };
            let complete = item.is_complete() != 0;
            let received = item.received_bytes();
            let total = item.total_bytes();
            if complete {
                let full = item.full_path();
                let path = CefString::from(&full).to_string();
                eprintln!("[aygent][cef] download complete -> {path} ({received}/{total})");
                if let Some(app) = app() {
                    let name = std::path::Path::new(&path)
                        .file_name().map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let _ = app.emit("browser:download", &serde_json::json!({
                        "state": "complete", "name": name, "path": path, "done": true,
                    }));
                }
            }
        }
    }
}

fn sanitize(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') { c } else { '_' })
        .collect();
    s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { s = "download".into(); }
    s
}

// ============================================================================
// BROWSER TAB PLUMBING
// ============================================================================
//
// We stash the tab id on each Browser via a side map keyed by CEF browser
// identifier (an i32 per browser). The display handler receives a Browser and
// we look up which tab it belongs to.

// CEF Browser::identifier() is a u64.
// Key = CEF Browser::identifier(), which returns i32.
static BROWSER_TAB: OnceLock<Mutex<HashMap<i32, i64>>> = OnceLock::new();

fn browser_tab_map() -> &'static Mutex<HashMap<i32, i64>> {
    BROWSER_TAB.get_or_init(|| Mutex::new(HashMap::new()))
}

fn browser_tab_id(b: &mut Browser) -> Option<i64> {
    let id = b.identifier();
    browser_tab_map().lock().ok().and_then(|m| m.get(&id).copied())
}

fn emit_tab_navigated(app: &AppHandle, tab: Option<i64>, url: &str, title: &str) {
    let _ = app.emit("browser:tab-navigated", &serde_json::json!({
        "tabId": tab, "url": url, "title": title,
    }));
}
fn emit_tab_title(app: &AppHandle, tab: Option<i64>, title: &str) {
    let _ = app.emit("browser:tab-navigated", &serde_json::json!({
        "tabId": tab, "title": title,
    }));
}

// ---------------------------------------------------------------------------
// PUBLIC OPERATIONS — all hop onto CEF's UI thread via post_task.
// ---------------------------------------------------------------------------

/// Create (or reuse) the CEF browser for `tab_id`, parented into `parent_view`
/// (the app-owned wrapper NSView from cef_geometry) at the given bounds. If the
/// tab already has a browser, navigate it instead.
pub fn create_or_navigate(tab_id: i64, url: String, parent_view: *mut std::ffi::c_void, x: i32, y: i32, w: i32, h: i32) {
    let pending = PendingCreate {
        tab_id, url, parent_view: parent_view as usize, x, y, w, h,
    };
    run_on_ui(move || create_or_navigate_ui(pending));
}

fn create_or_navigate_ui(p: PendingCreate) {
    // Reuse existing browser for this tab?
    let existing = BROWSERS.with(|m| m.borrow().get(&p.tab_id).cloned());
    if let Some(mut b) = existing {
        if let Some(frame) = b.main_frame() {
            let u = CefString::from(p.url.as_str());
            frame.load_url(Some(&u));
        }
        return;
    }

    // usize -> raw pointer -> CEF's window-handle typedef (an NSView* on macOS).
    // Two-step cast: int->*mut c_void is allowed, then pointer->pointer to the
    // exact typedef is allowed (identity if the typedef IS *mut c_void).
    let parent_handle = p.parent_view as *mut std::ffi::c_void as sys::cef_window_handle_t;
    // ALLOY runtime style: windowed child-view embedding (set_as_child into a
    // parent NSView) is an Alloy-style capability. Chrome runtime style is
    // geared to top-level Views windows; for a browser hosted inside our own
    // NSView we use Alloy so the child-window path is supported. If a first-run
    // build shows a blank/failed child, this field is the first thing to flip.
    let window_info = WindowInfo {
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    }
    .set_as_child(
        parent_handle,
        &Rect { x: p.x, y: p.y, width: p.w, height: p.h },
    );
    let url = CefString::from(p.url.as_str());
    let settings = BrowserSettings::default();
    let mut client = AygentClient::new();

    // Create the browser synchronously (we're on the UI thread) so we get the
    // Browser handle back to register it against the tab id.
    let browser = browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut client),
        Some(&url),
        Some(&settings),
        None,
        None,
    );
    if let Some(mut b) = browser {
        let ident = b.identifier();
        if let Ok(mut tm) = browser_tab_map().lock() { tm.insert(ident, p.tab_id); }
        BROWSERS.with(|m| { m.borrow_mut().insert(p.tab_id, b); });
        eprintln!("[aygent][cef] tab {} bound to browser id {}", p.tab_id, ident);
    } else {
        eprintln!("[aygent][cef] create_browser_sync returned None for tab {}", p.tab_id);
    }
}

pub fn navigate(tab_id: i64, url: String) {
    run_on_ui(move || {
        if let Some(mut b) = BROWSERS.with(|m| m.borrow().get(&tab_id).cloned()) {
            if let Some(frame) = b.main_frame() {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
        }
    });
}

pub fn go_back(tab_id: i64) {
    run_on_ui(move || {
        if let Some(mut b) = BROWSERS.with(|m| m.borrow().get(&tab_id).cloned()) {
            if b.can_go_back() != 0 { b.go_back(); }
        }
    });
}
pub fn go_forward(tab_id: i64) {
    run_on_ui(move || {
        if let Some(mut b) = BROWSERS.with(|m| m.borrow().get(&tab_id).cloned()) {
            if b.can_go_forward() != 0 { b.go_forward(); }
        }
    });
}
pub fn reload(tab_id: i64) {
    run_on_ui(move || {
        if let Some(mut b) = BROWSERS.with(|m| m.borrow().get(&tab_id).cloned()) {
            b.reload();
        }
    });
}

pub fn close_tab(tab_id: i64) {
    run_on_ui(move || {
        if let Some(mut b) = BROWSERS.with(|m| m.borrow_mut().remove(&tab_id)) {
            let ident = b.identifier();
            if let Ok(mut tm) = browser_tab_map().lock() { tm.remove(&ident); }
            if let Some(host) = b.host() {
                host.close_browser(1); // force
            }
            eprintln!("[aygent][cef] close tab {tab_id}");
        }
    });
}

// ---------------------------------------------------------------------------
// UI-THREAD DISPATCH. CEF exposes post_task(ThreadId::UI, Task). We wrap an
// FnOnce in a Task via the wrap_task! macro. Because closures aren't 'static
// Send trivially, we box them and shuttle through a queue drained by the task.
// ---------------------------------------------------------------------------

type UiJob = Box<dyn FnOnce() + Send + 'static>;

static UI_QUEUE: OnceLock<Mutex<Vec<UiJob>>> = OnceLock::new();
fn ui_queue() -> &'static Mutex<Vec<UiJob>> {
    UI_QUEUE.get_or_init(|| Mutex::new(Vec::new()))
}

fn run_on_ui<F: FnOnce() + Send + 'static>(f: F) {
    if !is_ready() {
        eprintln!("[aygent][cef] run_on_ui dropped (engine not ready)");
        return;
    }
    // If we're already on the CEF UI thread, run inline.
    if currently_on(ThreadId::UI) != 0 {
        f();
        return;
    }
    if let Ok(mut q) = ui_queue().lock() { q.push(Box::new(f)); }
    let mut task = UiPump::new();
    post_task(ThreadId::UI, Some(&mut task));
}

wrap_task! {
    struct UiPump {}

    impl Task {
        fn execute(&self) {
            // Drain everything queued (on the UI thread now).
            loop {
                let job = ui_queue().lock().ok().and_then(|mut q| if q.is_empty() { None } else { Some(q.remove(0)) });
                match job {
                    Some(j) => j(),
                    None => break,
                }
            }
        }
    }
}
