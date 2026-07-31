// Prevents an extra console window on Windows in release; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // ENGINE-CEF: install our CefAppProtocol-conforming NSApplication subclass
    // as the process's shared app BEFORE anything else. Whoever first calls
    // +[NSApplication sharedApplication] fixes the app class; if tao does it
    // first we can't swap it. This is what makes CEF's native browser view
    // actually render on macOS (the sendEvent: routing CEF requires).
    #[cfg(all(target_os = "macos", feature = "engine-cef"))]
    aygent_lib::cef_app_mac::install();

    aygent_lib::run()
}
