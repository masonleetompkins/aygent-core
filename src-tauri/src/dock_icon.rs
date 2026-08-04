// AYGENT — DYNAMIC APP ICON (macOS).
//
// Mason 08-04: the Dock icon should follow the app's theme — a glowing "AY" in
// the current accent colour on a black or white ground depending on light/dark
// mode. Rather than pre-rendering a matrix of PNG variants (accent is a free
// colour picker, so that matrix is unbounded), the UI DRAWS the icon on a
// <canvas> whenever the theme changes and hands us the PNG bytes. This module
// installs those bytes as the running app's Dock icon.
//
// SCOPE: this changes the icon of the RUNNING process only. The .app bundle's
// on-disk icon (what Finder shows when AYGENT isn't running) still comes from
// icons/icon.icns — changing that would mean rewriting the bundle, which the
// exec broker deliberately forbids (an agent must not be able to rebuild its
// own host).

#[cfg(target_os = "macos")]
pub fn set_dock_icon_png(bytes: &[u8]) -> Result<(), String> {
    use objc2::rc::Retained;
    // `AnyThread` is what provides `alloc()` in objc2 0.6 (it is also re-exported
    // as `AllocAnyThread`). Without it in scope, NSImage::alloc doesn't resolve.
    use objc2::AnyThread;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{NSData, MainThreadMarker};

    // AppKit is main-thread-only. We're called from a Tauri command, which
    // Tauri dispatches on the main thread for us; assert rather than assume.
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "set_dock_icon must run on the main thread".to_string())?;

    if bytes.is_empty() {
        return Err("empty icon data".into());
    }

    let data = NSData::with_bytes(bytes);
    let image: Retained<NSImage> = NSImage::initWithData(NSImage::alloc(), &data)
        .ok_or_else(|| "could not decode icon PNG".to_string())?;

    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: standard AppKit setter; `image` is a valid NSImage we just made.
    unsafe { app.setApplicationIconImage(Some(&image)) };
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon_png(_bytes: &[u8]) -> Result<(), String> {
    // No-op off macOS: the Windows/Linux builds keep their bundled icon.
    Ok(())
}
