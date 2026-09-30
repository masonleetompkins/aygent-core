// Prevents an extra console window on Windows in release; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // LINUX JAIL: when the supervisor starts this binary as the daemon's
    // Landlock launcher, confine and exec node here (never returns then).
    #[cfg(target_os = "linux")]
    aygent_lib::linux_jail::maybe_launch();

    // LINUX: WebKitGTK's DMA-BUF renderer draws a blank (transparent) window
    // on many GPU/driver combinations (NVIDIA in particular: "Failed to create
    // GBM buffer"). Turn it off unless the user chose otherwise.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    // ENGINE-CEF: install our CefAppProtocol-conforming NSApplication subclass
    // as the process's shared app BEFORE anything else. Whoever first calls
    // +[NSApplication sharedApplication] fixes the app class; if tao does it
    // first we can't swap it. This is what makes CEF's native browser view
    // actually render on macOS (the sendEvent: routing CEF requires).
    #[cfg(all(target_os = "macos", feature = "engine-cef"))]
    aygent_lib::cef_app_mac::install();

    aygent_lib::run()
}
