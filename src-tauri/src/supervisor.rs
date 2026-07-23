// AYGENT — daemon supervisor.
// The ONLY code allowed to launch the Node daemon (Atlas C1). On macOS it
// launches the daemon UNDER a sandbox-exec profile that denies file+exec, so
// the daemon physically cannot bypass the Rust path broker. In dev (or before
// the Seatbelt profile is finalized in M0.2) a plain launch is used so the
// UI<->daemon WS loop can be built and tested first.
//
// M0.1: launch the daemon, capture the WS port it prints on stderr, store it
// in shared state so the `daemon_info` Tauri command can hand it to the UI.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct DaemonState {
    pub ws_token: String,
    pub ws_port: Mutex<Option<u16>>,
}

/// Launch the daemon. `jailed` selects Seatbelt (true, macOS Folder Mode) vs a
/// plain dev launch (false). Captures the `AYGENT_WS_PORT=NNNN` line the daemon
/// prints and stores it in state.
pub fn spawn_daemon(state: Arc<DaemonState>, jailed: bool) -> Result<(), Box<dyn std::error::Error>> {
    let daemon_entry = std::env::var("AYGENT_DAEMON_ENTRY")
        .unwrap_or_else(|_| "../daemon/dist/index.js".to_string());

    let mut cmd = if jailed {
        // macOS Folder Mode: node runs INSIDE the Seatbelt jail (deny file+exec).
        let profile = std::env::var("AYGENT_SEATBELT_PROFILE")
            .unwrap_or_else(|_| "../seatbelt/folder-mode.sb".to_string());
        let mut c = Command::new("sandbox-exec");
        c.arg("-f").arg(&profile).arg("node").arg(&daemon_entry);
        c
    } else {
        // Dev / pre-M0.2: plain launch so we can build the WS loop first.
        let mut c = Command::new("node");
        c.arg(&daemon_entry);
        c
    };

    let mut child = cmd
        .env("AYGENT_WS_TOKEN", &state.ws_token)
        .stderr(Stdio::piped())
        .stdout(Stdio::inherit())
        .spawn()?;

    // Read the daemon's stderr to catch the WS port line, then keep draining.
    if let Some(stderr) = child.stderr.take() {
        let state = state.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(idx) = line.find("AYGENT_WS_PORT=") {
                    let rest = &line[idx + "AYGENT_WS_PORT=".len()..];
                    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(p) = num.parse::<u16>() {
                        *state.ws_port.lock().unwrap() = Some(p);
                    }
                }
                eprintln!("[daemon] {line}");
            }
        });
    }

    eprintln!(
        "[aygent] daemon spawned (jailed={jailed}) entry={daemon_entry}"
    );
    // Intentionally not waiting; daemon runs for the app lifetime.
    std::mem::forget(child);
    Ok(())
}
