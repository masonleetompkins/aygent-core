// AYGENT — daemon supervisor.
// The ONLY code allowed to launch the Node daemon (Atlas C1). On macOS it
// launches the daemon UNDER a sandbox-exec profile that denies file+exec, so
// the daemon physically cannot bypass the Rust path broker.
//
// M0.2(e): the Seatbelt profile has <<NODE_BIN>> and <<APP_BUNDLE_SUBPATH>>
// templates. We resolve the real node binary + daemon dir and fill them in,
// writing the concrete profile to a temp file before launching. This is what
// prevents the `execvp of node failed` gotcha (the profile must allow reading
// + exec of the node binary while still denying the user's folders).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::Manager; // for app.path().resource_dir() (bundled daemon/seatbelt lookup)

#[derive(Default)]
pub struct DaemonState {
    pub ws_token: String,
    pub ws_port: Mutex<Option<u16>>,
}

/// Find the absolute path to `node` (Seatbelt needs the concrete binary path;
/// `execvp` inside the jail can't do a PATH search once fs is denied).
///
/// BUNDLED-APP FIX (2026-07-31): a Finder-launched .app inherits an EMPTY/minimal
/// PATH, so `/usr/bin/which node` returns nothing — that's why the production app
/// never spawned the daemon. We now probe the common absolute install locations
/// directly (homebrew arm64/intel, /usr/local, /usr/bin, nvm) before falling back
/// to `which`. AYGENT_NODE_BIN still overrides everything (dev/CI).
fn resolve_node_bin() -> Option<String> {
    if let Ok(explicit) = std::env::var("AYGENT_NODE_BIN") {
        return Some(explicit);
    }
    // Probe concrete absolute paths first — works even with no PATH (Finder launch).
    let candidates = [
        "/opt/homebrew/bin/node",   // Apple Silicon homebrew
        "/usr/local/bin/node",      // Intel homebrew / manual
        "/usr/bin/node",            // system
    ];
    for c in candidates {
        if std::path::Path::new(c).exists() {
            return std::fs::canonicalize(c).ok().map(|p| p.to_string_lossy().to_string()).or_else(|| Some(c.to_string()));
        }
    }
    // nvm: newest installed version under ~/.nvm/versions/node/*/bin/node.
    if let Some(home) = std::env::var_os("HOME") {
        let nvm = std::path::Path::new(&home).join(".nvm/versions/node");
        if let Ok(rd) = std::fs::read_dir(&nvm) {
            let mut versions: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            versions.sort();
            if let Some(latest) = versions.last() {
                let n = latest.join("bin/node");
                if n.exists() { return Some(n.to_string_lossy().to_string()); }
            }
        }
    }
    // Last resort: `which node` (works in dev / a shell-launched app).
    let out = Command::new("/usr/bin/which").arg("node").output().ok()?;
    if out.status.success() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !p.is_empty() {
            return std::fs::canonicalize(&p).ok().map(|c| c.to_string_lossy().to_string()).or(Some(p));
        }
    }
    None
}

/// Resolve the DAEMON ENTRY (index.js) + SEATBELT PROFILE, working in BOTH a
/// bundled .app and dev. Priority: env override → bundled resource dir → dev path.
///
/// BUNDLED-APP FIX: in a .app, resources live under `<App>.app/Contents/Resources/`
/// (Tauri's resource dir). We bundle `daemon/` + `seatbelt/folder-mode.sb` there
/// (see tauri.conf.json). Dev keeps the old `../daemon`, `../seatbelt` relative
/// paths. This is why the production app couldn't find/spawn the daemon.
fn resolve_daemon_entry(app: &tauri::AppHandle) -> String {
    if let Ok(explicit) = std::env::var("AYGENT_DAEMON_ENTRY") {
        return explicit;
    }
    if let Ok(res) = app.path().resource_dir() {
        let bundled = res.join("daemon").join("dist").join("index.js");
        if bundled.exists() {
            return bundled.to_string_lossy().to_string();
        }
    }
    "../daemon/dist/index.js".to_string()
}

fn resolve_seatbelt_profile(app: &tauri::AppHandle) -> String {
    if let Ok(explicit) = std::env::var("AYGENT_SEATBELT_PROFILE") {
        return explicit;
    }
    if let Ok(res) = app.path().resource_dir() {
        let bundled = res.join("seatbelt").join("folder-mode.sb");
        if bundled.exists() {
            return bundled.to_string_lossy().to_string();
        }
    }
    "../seatbelt/folder-mode.sb".to_string()
}

/// Build a concrete Seatbelt profile from the template, filling in the resolved
/// node binary + daemon dir, and write it to a temp file. Returns its path.
fn materialize_profile(app: &tauri::AppHandle, node_bin: &str, daemon_dir: &str) -> std::io::Result<std::path::PathBuf> {
    let template_path = resolve_seatbelt_profile(app);
    let template = std::fs::read_to_string(&template_path)?;

    // node lives in a bin dir; allow reading that dir's tree (dylibs, ICU data).
    let node_dir = std::path::Path::new(node_bin)
        .parent()
        .and_then(|p| p.parent()) // .../bin/node -> allow the install prefix
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "/usr/local".to_string());

    let concrete = template
        .replace("<<NODE_BIN>>", node_bin)
        .replace("<<APP_BUNDLE_SUBPATH>>", daemon_dir)
        // extra: allow the node install prefix so its dylibs/ICU resolve
        .replace(
            "(allow process-exec (literal \"<<NODE_BIN>>\"))",
            &format!(
                "(allow process-exec (literal \"{node_bin}\"))\n(allow file-read* (subpath \"{node_dir}\"))"
            ),
        );

    let mut tmp = std::env::temp_dir();
    tmp.push(format!("aygent-folder-mode-{}.sb", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(concrete.as_bytes())?;
    Ok(tmp)
}

/// Launch the daemon. `jailed` selects Seatbelt (true, macOS Folder Mode) vs a
/// plain dev launch (false). Captures the `AYGENT_WS_PORT=NNNN` line + drains stderr.
pub fn spawn_daemon(
    app: &tauri::AppHandle,
    state: Arc<DaemonState>,
    jailed: bool,
    broker_port: u16,
    broker_token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // BUNDLED-APP FIX: resolve from the .app resource dir when installed, dev
    // path otherwise. This is why the production app couldn't spawn the daemon.
    let daemon_entry = resolve_daemon_entry(app);
    // daemon dir = the tree the jailed node is allowed to READ (its own code).
    // MUST be the whole daemon/ package (dist/ + node_modules/), NOT just dist/,
    // or node can't load its own deps (e.g. ws/index.js) -> EPERM at boot.
    // dist/index.js -> parent=dist -> parent=daemon/  (the package root).
    let daemon_dir = std::path::Path::new(&daemon_entry)
        .parent()                       // .../daemon/dist
        .and_then(|p| p.parent())       // .../daemon
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "../daemon".to_string());

    let mut cmd = if jailed {
        let node_bin = resolve_node_bin()
            .ok_or("could not resolve node binary for Seatbelt launch")?;
        let profile = materialize_profile(app, &node_bin, &daemon_dir)?;
        eprintln!("[aygent] jailed launch: node={node_bin} profile={}", profile.display());
        let mut c = Command::new("sandbox-exec");
        c.arg("-f").arg(&profile).arg(&node_bin).arg(&daemon_entry);
        c
    } else {
        let mut c = Command::new("node");
        c.arg(&daemon_entry);
        c
    };

    let mut child = cmd
        // Set cwd to the daemon dir (which the jail ALLOWS). Otherwise node's
        // process.cwd() at boot hits EPERM on uv_cwd (the launch cwd, src-tauri/,
        // is denied by the Seatbelt profile). Fixes: uv_cwd EPERM at boot.
        .current_dir(&daemon_dir)
        .env("AYGENT_WS_TOKEN", &state.ws_token)
        .env("AYGENT_BROKER_PORT", broker_port.to_string())
        .env("AYGENT_BROKER_TOKEN", broker_token)
        // PRO MODE (2026-07-31): grant the daemon session shell.exec so the exec
        // broker will accept exec.* ops. This is Mason's PERSONAL harness (not a
        // shipping user feature) — the daemon is allowed to exec; the REAL switch
        // is the per-folder pro_mode flag, which decides whether the shell_* tools
        // are ever exposed to the model. Without this env the broker binds no exec
        // grant and every exec op is refused (the gap we flagged). The child-env
        // scrub in exec.rs still strips this from any SPAWNED process.
        .env("AYGENT_AGENT_CAPS", "shell.exec")
        .stderr(Stdio::piped())
        .stdout(Stdio::inherit())
        .spawn()?;

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

    eprintln!("[aygent] daemon spawned (jailed={jailed}) entry={daemon_entry}");
    std::mem::forget(child);
    Ok(())
}
