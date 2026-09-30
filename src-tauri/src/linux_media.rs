// AYGENT — Linux: the microphone for voice input.
//
// WebKitGTK hands every media permission request to the app embedding it
// (on macOS the OS asks the user instead). Unanswered, getUserMedia() never
// resolves and the Chat mic button silently does nothing. The app's own UI
// (its only content) asking for audio is allowed; camera and screen requests
// are refused.

use tauri::Manager;
use webkit2gtk::glib::object::Cast;
use webkit2gtk::{PermissionRequestExt, SettingsExt, UserMediaPermissionRequest, UserMediaPermissionRequestExt, WebViewExt};

pub fn install(app: &tauri::AppHandle) {
    for (_, win) in app.webview_windows() {
        let _ = win.with_webview(|wv| {
            let view = wv.inner();
            // WebKitGTK ships with getUserMedia switched off.
            if let Some(settings) = view.settings() {
                settings.set_enable_media_stream(true);
            }
            view.connect_permission_request(|_, req| {
                if let Some(media) = req.downcast_ref::<UserMediaPermissionRequest>() {
                    if media.is_for_audio_device() && !media.is_for_video_device() {
                        req.allow();
                    } else {
                        req.deny();
                    }
                    return true;
                }
                false // anything else: WebKit's default
            });
        });
    }
}
