//! ============================================================================
//! AYGENT — CEF-aware NSApplication subclass (macOS, engine-cef only).
//! ============================================================================
//!
//! THE FIX for "CEF browser loads but renders nothing in our pane."
//!
//! On macOS, CEF requires the process's `NSApplication` to be a subclass that
//! conforms to `CefAppProtocol` and routes `-sendEvent:` through CEF (setting
//! an `isHandlingSendEvent` flag). Without this, CEF's native browser view does
//! not composite / handle events — the browser is created and loads a page, but
//! never draws into the host window. This is exactly what cefsimple's
//! `SimpleApplication` provides (see cef-rs examples/cefsimple/src/mac/mod.rs).
//!
//! aygent normally lets tao/Tauri create the default `NSApplication`. We can't
//! let that happen: whoever calls `+[NSApplication sharedApplication]` FIRST
//! fixes the app's class for the process. So `install()` MUST run at the very
//! top of main()/run(), BEFORE any Tauri/tao code touches NSApp. It force-
//! instantiates our `AygentApplication` subclass as the shared application; when
//! tao later asks for NSApp it receives OUR CEF-aware instance.
//!
//! Mirrors cefsimple's SimpleApplication exactly (the proven reference), minus
//! the app-delegate/menu bits tao provides.

#![cfg(all(target_os = "macos", feature = "engine-cef"))]

use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::runtime::{AnyObject, Bool};
use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSEvent};
use std::cell::Cell;

/// Instance variables: the `handling_send_event` flag CEF's protocol reads.
#[derive(Default)]
pub struct AygentApplicationIvars {
    handling_send_event: Cell<Bool>,
}

define_class!(
    /// An NSApplication subclass conforming to the CEF app protocols so CEF's
    /// native browser view composites + handles events. Identical shape to
    /// cefsimple's SimpleApplication.
    #[unsafe(super(NSApplication))]
    #[ivars = AygentApplicationIvars]
    pub struct AygentApplication;

    impl AygentApplication {
        // Route every event through CEF: set the handling flag around the
        // superclass sendEvent:, exactly as CEF requires on macOS.
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            let was = self.ivars().handling_send_event.get();
            if was == Bool::NO {
                self.ivars().handling_send_event.set(Bool::YES);
            }
            let _: () = msg_send![super(self), sendEvent: event];
            if was == Bool::NO {
                self.ivars().handling_send_event.set(Bool::NO);
            }
        }
    }

    // CrAppControlProtocol: CEF sets the handling flag.
    unsafe impl CrAppControlProtocol for AygentApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn set_handling_send_event(&self, handling: Bool) {
            self.ivars().handling_send_event.set(handling);
        }
    }

    // CrAppProtocol: CEF reads the handling flag.
    unsafe impl CrAppProtocol for AygentApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn is_handling_send_event(&self) -> Bool {
            self.ivars().handling_send_event.get()
        }
    }

    // The marker protocol that tells CEF this app is CEF-aware.
    unsafe impl CefAppProtocol for AygentApplication {}
);

/// Install AygentApplication as the process's shared NSApplication. MUST be
/// called at the very top of main(), BEFORE any Tauri/tao/NSApp access. Returns
/// true if our class is now the shared app (or was already), false if some
/// other NSApplication was created first (in which case CEF rendering won't work
/// and we should stay on the WKWebView path).
pub fn install() -> bool {
    // Force our subclass to become the shared application. `sharedApplication`
    // on our subclass creates an instance of OUR class and installs it as the
    // process-wide NSApp — but ONLY if nothing has created NSApp yet.
    let _: *mut AnyObject = unsafe { msg_send![AygentApplication::class(), sharedApplication] };
    // Verify NSApp is actually our class now.
    let shared: *mut AnyObject = unsafe { msg_send![objc2_app_kit::NSApplication::class(), sharedApplication] };
    if shared.is_null() {
        eprintln!("[aygent][cef-app] install: sharedApplication is null");
        return false;
    }
    let is_ours: Bool = unsafe { msg_send![shared, isKindOfClass: AygentApplication::class()] };
    let ok = is_ours == Bool::YES;
    eprintln!("[aygent][cef-app] install: NSApp is AygentApplication = {ok}");
    ok
}
