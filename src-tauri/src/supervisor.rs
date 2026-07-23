// AYGENT — Seatbelt supervisor (Atlas C1).
// The ONLY code allowed to launch the Node daemon. It launches it UNDER a
// macOS sandbox-exec profile that denies file+exec, so the daemon physically
// cannot bypass the Rust path broker. This is what makes Folder Mode honest.

#[cfg(target_os = "macos")]
use std::process::Command;

/// Launch the daemon under seatbelt/folder-mode.sb with the WS token in env.
/// Phase 0 (M0.1): minimal launcher; the profile path + APP_BUNDLE_SUBPATH
/// templating is finalized in M0.2. For `tauri dev` we resolve paths relative
/// to the repo; for a bundled .app they resolve inside Resources/.
#[cfg(target_os = "macos")]
pub fn spawn_daemon(ws_token: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Resolve the compiled profile + daemon entry. In dev these live in the
    // repo; in a bundle, under the .app Resources. M0.2 wires bundle-aware
    // resolution; for now use env override or repo-relative dev defaults.
    let profile = std::env::var("AYGENT_SEATBELT_PROFILE")
        .unwrap_or_else(|_| "../seatbelt/folder-mode.sb".to_string());
    let daemon_entry = std::env::var("AYGENT_DAEMON_ENTRY")
        .unwrap_or_else(|_| "../daemon/dist/index.js".to_string());

    // sandbox-exec -f <profile> node <daemon>
    // NOTE (Atlas N6): validate this composes with hardened-runtime signing
    // before we notarize. Test early.
    let child = Command::new("sandbox-exec")
        .arg("-f")
        .arg(&profile)
        .arg("node")
        .arg(&daemon_entry)
        .env("AYGENT_WS_TOKEN", ws_token)
        // no AYGENT_WS_SOCKET yet -> daemon uses loopback+token in Phase 0
        .spawn();

    match child {
        Ok(_) => {
            eprintln!("[aygent] daemon spawned under Seatbelt profile: {profile}");
            Ok(())
        }
        Err(e) => {
            eprintln!("[aygent] FAILED to spawn daemon under sandbox-exec: {e}");
            Err(Box::new(e))
        }
    }
}
