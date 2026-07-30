//! ============================================================================
//! AYGENT — CEF geometry / punchout (macOS, engine-cef only).
//! ============================================================================
//!
//! Implements the ATRIUM "punchout" embed (getatrium.dev/blog/embedding-real-
//! browser-tauri) that lets a native Chromium view sit behind a transparent
//! hole in the React (Wry) webview:
//!
//!   1. `set_main_webview_transparent()` — [wryWebView setDrawsBackground:NO]
//!      so wherever React paints nothing, the layers behind show through.
//!   2. `ensure_wrapper()` — one AYGENT-owned wrapper NSView per tab, wantsLayer
//!      + lowered zPosition, added as a subview of the window contentView BEHIND
//!      the main webview. CEF's child browser view lives inside this wrapper
//!      (cef_engine parents the browser into wrapper via set_as_child).
//!   3. `place_wrapper()` — sync the wrapper's frame to the React pane rect
//!      (same top-left CSS -> AppKit bottom-left flip logic as browser.rs's
//!      place_child_exact). Driven by the FE geometry manager each layout frame.
//!   4. `set_hittest_enabled()` — the AYGENT hitTest override. macOS routes
//!      clicks by SUBVIEW ORDER, not zPosition, so an overlay drawn in React
//!      would be click-through unless the CEF wrapper DECLINES the event. When
//!      an overlay is open we flip a flag; the override returns nil so clicks
//!      fall through to the React webview in front.
//!
//! All of this is `#[cfg(all(target_os = "macos", feature = "engine-cef"))]`.

#![cfg(all(target_os = "macos", feature = "engine-cef"))]

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, MainThreadOnly};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSView, NSWindow, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

/// GLOBAL overlay-open flag. When true, ALL CEF wrappers decline hit-testing so
/// React overlays (permission card, History/Downloads panel, agent pane, any
/// modal) receive clicks. Driven by `set_browser_hittest(enabled)` command,
/// which is driven by Browser.tsx's overlay REGISTRY (a Set — never a fragile
/// counter, per Atrium's hard-won lesson where a drifting counter permanently
/// broke clicks).
static HITTEST_DECLINE: AtomicBool = AtomicBool::new(false);

/// Set the global hit-test state. `enabled=true` means the browser RECEIVES
/// clicks normally; `enabled=false` means an overlay is open -> the browser
/// DECLINES so clicks fall through to React. (We store the DECLINE sense.)
pub fn set_hittest_enabled(enabled: bool) {
    HITTEST_DECLINE.store(!enabled, Ordering::SeqCst);
    eprintln!("[aygent][cef] hittest {}", if enabled { "ENABLED (browser gets clicks)" } else { "DISABLED (overlay open, click-through)" });
}

fn should_decline() -> bool {
    HITTEST_DECLINE.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// The AYGENT wrapper NSView subclass with the hitTest override.
// ---------------------------------------------------------------------------
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AygentCefWrapper"]
    pub struct AygentCefWrapper;

    impl AygentCefWrapper {
        // THE OVERLAY FIX. When an overlay is open we return nil (a null view*)
        // so AppKit walks the responder chain to the React webview in front.
        // Otherwise defer to the normal NSView hitTest (which routes to the CEF
        // child view inside us).
        #[unsafe(method(hitTest:))]
        unsafe fn hit_test(&self, point: NSPoint) -> *mut NSView {
            if should_decline() {
                return std::ptr::null_mut();
            }
            let sup: *mut NSView = msg_send![super(self), hitTest: point];
            sup
        }
    }
);

impl AygentCefWrapper {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm);
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        unsafe {
            this.setWantsLayer(true);
            if let Some(layer) = this.layer() {
                // Lowered zPosition so it composites BEHIND the main webview.
                let _: () = msg_send![&*layer, setZPosition: -1.0f64];
            }
        }
        this
    }
}

// SAFETY: Retained<AygentCefWrapper> is only ever created/used on the main
// thread (all callers assert MainThreadMarker). The map below stores raw
// pointers guarded by a Mutex; we never deref off-main-thread.
struct WrapperEntry {
    view: *mut NSView,
}
unsafe impl Send for WrapperEntry {}

static WRAPPERS: OnceLock<Mutex<HashMap<i64, WrapperEntry>>> = OnceLock::new();
fn wrappers() -> &'static Mutex<HashMap<i64, WrapperEntry>> {
    WRAPPERS.get_or_init(|| Mutex::new(HashMap::new()))
}

// Whether we've already set the main webview transparent (idempotent).
static MADE_TRANSPARENT: AtomicBool = AtomicBool::new(false);

/// [wryWebView setDrawsBackground:NO] on the MAIN window's web content view, so
/// the transparent React pane div lets the CEF wrapper show through. Best
/// effort; safe to call repeatedly. `webview` is any Tauri webview whose
/// with_webview gives us the WKWebView NSView.
pub fn set_main_webview_transparent(webview: &tauri::Webview) {
    if MADE_TRANSPARENT.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = webview.with_webview(|pw| unsafe {
        let raw: *mut NSView = pw.inner().cast();
        if raw.is_null() { return; }
        let view: &NSView = &*raw;
        // WKWebView exposes KVC-settable `drawsBackground` (BOOL). Set it via
        // setValue:forKey: with an NSNumber(false) so we don't depend on the
        // underscore-prefixed private selector directly. This is the documented
        // Atrium approach: [wryWebView setDrawsBackground:NO].
        let no = objc2_foundation::NSNumber::new_bool(false);
        let _: () = msg_send![view, setValue: &*no, forKey: objc2_foundation::ns_string!("drawsBackground")];
        // Belt: make the backing layer non-opaque so the transparency composites
        // through to the CEF wrapper behind.
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            let _: () = msg_send![&*layer, setOpaque: false];
        }
        eprintln!("[aygent][cef] main webview setDrawsBackground:NO");
    });
}

/// Ensure a wrapper NSView exists for `tab_id`, added behind the main webview.
/// Returns the wrapper's NSView pointer (as *mut c_void) for cef_engine to
/// parent the CEF browser into. Must be called on the main thread (Tauri
/// command handlers run on the main thread on macOS via run_on_main).
pub fn ensure_wrapper(window: &tauri::Window, tab_id: i64, frame: (f64, f64, f64, f64)) -> Option<*mut std::ffi::c_void> {
    let mtm = MainThreadMarker::new()?;
    // Already have one?
    if let Ok(map) = wrappers().lock() {
        if let Some(e) = map.get(&tab_id) {
            return Some(e.view as *mut std::ffi::c_void);
        }
    }
    let ns_window = window_nswindow(window)?;
    let content = unsafe { ns_window.contentView()? };
    let (x, y, w, h) = frame;
    let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)));
    let wrapper = AygentCefWrapper::new(mtm, rect);
    unsafe {
        // Add BEHIND everything else (Below the front-most main webview).
        let _: () = msg_send![
            &*content,
            addSubview: &*wrapper,
            positioned: NSWindowOrderingMode::Below,
            relativeTo: std::ptr::null::<AnyObject>()
        ];
    }
    let raw: *mut NSView = Retained::as_ptr(&wrapper) as *mut NSView;
    // Leak the Retained so the view lives for the tab's life; we drop it on
    // remove_wrapper via a manual release.
    std::mem::forget(wrapper);
    if let Ok(mut map) = wrappers().lock() {
        map.insert(tab_id, WrapperEntry { view: raw });
    }
    eprintln!("[aygent][cef] wrapper created tab={tab_id}");
    Some(raw as *mut std::ffi::c_void)
}

/// Reposition a tab's wrapper to the React pane rect. `content_h` is the
/// content-area height the FE measured against (for the Y flip), same as
/// place_child_exact.
pub fn place_wrapper(tab_id: i64, x: f64, y: f64, w: f64, h: f64, content_h: Option<f64>) {
    let Some(raw) = wrapper_ptr(tab_id) else { return };
    unsafe {
        let view: &NSView = &*raw;
        let Some(sup) = view.superview() else { return };
        let (flipped, parent_h) = (sup.isFlipped(), sup.bounds().size.height);
        let flip_h = content_h.filter(|v| *v > 0.0).unwrap_or(parent_h);
        let oy = if flipped { y } else { flip_h - y - h };
        view.setFrame(NSRect::new(NSPoint::new(x, oy), NSSize::new(w.max(1.0), h.max(1.0))));
        eprintln!("[aygent][cef] place wrapper tab={tab_id} -> ({x:.0},{oy:.0} {w:.0}x{h:.0})");
    }
}

/// Hide/show a tab's wrapper (tab switch). Hidden wrappers drop out of
/// compositing AND hit-testing.
pub fn set_wrapper_hidden(tab_id: i64, hidden: bool) {
    if let Some(raw) = wrapper_ptr(tab_id) {
        unsafe { (&*raw).setHidden(hidden); }
    }
}

/// Bring a tab's wrapper to the front among the CEF wrappers (still behind the
/// main webview) so the active tab is the visible one.
pub fn front_wrapper(tab_id: i64) {
    if let Some(raw) = wrapper_ptr(tab_id) {
        unsafe {
            let view: &NSView = &*raw;
            if let Some(sup) = view.superview() {
                // Keep it BELOW the main webview but ABOVE the other wrappers by
                // re-adding Below the main webview (which stays front-most).
                let _: () = msg_send![
                    &*sup,
                    addSubview: view,
                    positioned: NSWindowOrderingMode::Below,
                    relativeTo: std::ptr::null::<AnyObject>()
                ];
            }
        }
    }
}

/// Remove + release a tab's wrapper (tab close).
pub fn remove_wrapper(tab_id: i64) {
    let raw = if let Ok(mut map) = wrappers().lock() {
        map.remove(&tab_id).map(|e| e.view)
    } else { None };
    if let Some(raw) = raw {
        unsafe {
            let view: &NSView = &*raw;
            view.removeFromSuperview();
            // Balance the mem::forget from ensure_wrapper.
            let _: () = msg_send![view, release];
        }
        eprintln!("[aygent][cef] wrapper removed tab={tab_id}");
    }
}

/// Hide every tab's wrapper except `keep`, and front `keep`. Called on tab
/// switch (webview_hide_others). keep=None hides all (leaving the Browser
/// screen).
pub fn hide_others_and_front(keep: Option<i64>) {
    let ids: Vec<i64> = wrappers().lock().ok().map(|m| m.keys().copied().collect()).unwrap_or_default();
    for id in ids {
        let is_keep = Some(id) == keep;
        set_wrapper_hidden(id, !is_keep);
        if is_keep { front_wrapper(id); }
    }
}

fn wrapper_ptr(tab_id: i64) -> Option<*mut NSView> {
    wrappers().lock().ok().and_then(|m| m.get(&tab_id).map(|e| e.view))
}

fn window_nswindow(window: &tauri::Window) -> Option<Retained<NSWindow>> {
    // Tauri Window::ns_window() returns *mut c_void to the NSWindow.
    let ptr = window.ns_window().ok()? as *mut NSWindow;
    if ptr.is_null() { return None; }
    Some(unsafe { Retained::retain(ptr)? })
}
