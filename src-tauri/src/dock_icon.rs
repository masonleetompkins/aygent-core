// AYGENT — DYNAMIC APP ICON (macOS).
//
// Mason 08-04: the Dock icon should follow the app's theme — an "AY" wordmark
// in the current accent colour on a black or white ground depending on
// light/dark mode (flat, no glow — same call as the in-app icons). Rather than
// pre-rendering a matrix of PNG variants (accent is a free colour picker, so
// that matrix is unbounded), the UI DRAWS the icon on a <canvas> whenever the
// theme changes and hands us the PNG bytes.
//
// PERSISTENCE (Mason 08-04, round 2): setting only
// NSApplication.applicationIconImage themes the RUNNING process — on quit the
// Dock/Finder fell back to the bundled icon.icns and the theme "reverted". So
// we now ALSO stamp the icon onto the .app bundle via NSWorkspace
// setIcon:forFile: — the same mechanism as Finder's paste-an-icon. That writes
// a resource-fork/FinderInfo xattr on the bundle DIRECTORY; it does NOT touch
// Contents/ or the code signature, so this is not "rebuilding our own host"
// (the thing the exec broker rightly forbids). Deleting the xattr restores the
// stock icon.

#[cfg(target_os = "macos")]
pub fn set_dock_icon_png(bytes: &[u8]) -> Result<(), String> {
    use objc2::rc::Retained;
    // `AnyThread` is what provides `alloc()` in objc2 0.6 (it is also re-exported
    // as `AllocAnyThread`). Without it in scope, NSImage::alloc doesn't resolve.
    use objc2::AnyThread;
    use objc2_app_kit::{NSApplication, NSImage, NSWorkspace, NSWorkspaceIconCreationOptions};
    use objc2_foundation::{NSData, NSString, MainThreadMarker};

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

    // 1) The RUNNING app's Dock tile (instant feedback while the app is open).
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: standard AppKit setter; `image` is a valid NSImage we just made.
    unsafe { app.setApplicationIconImage(Some(&image)) };

    // 2) PERSIST: stamp the same image onto the .app bundle so Finder + the
    // Dock keep showing it after quit. Best-effort — a dev binary run outside
    // a bundle has no .app to stamp, and that must not fail the command.
    if let Some(bundle) = bundle_app_path() {
        let ws = NSWorkspace::sharedWorkspace();
        let path = NSString::from_str(&bundle);
        // Documented NSWorkspace API; empty options = default behavior.
        let ok = ws.setIcon_forFile_options(Some(&image), &path, NSWorkspaceIconCreationOptions::empty());
        if !ok {
            // Non-fatal: the live icon still applied; persistence just didn't.
            eprintln!("[aygent][icon] setIcon:forFile: refused for {bundle}");
        }
    }
    Ok(())
}

/// The enclosing .app bundle path, if we're running from one
/// (…/AYGENT.app/Contents/MacOS/aygent → …/AYGENT.app). None under
/// `cargo run`/dev where there is no bundle.
#[cfg(target_os = "macos")]
fn bundle_app_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?; // MacOS/ → Contents/ → .app
    if app.extension().is_some_and(|e| e == "app") {
        Some(app.to_string_lossy().into_owned())
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon_png(_bytes: &[u8]) -> Result<(), String> {
    // No-op off macOS: the Windows/Linux builds keep their bundled icon.
    Ok(())
}
