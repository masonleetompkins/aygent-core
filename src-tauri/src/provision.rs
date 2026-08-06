// AYGENT — runtime provisioning (2026-08-06). Level A: bundle a PORTABLE Node +
// static FFmpeg + the HyperFrames npm package into AYGENT's OWN app-data space,
// so a user gets "make videos/graphics from the app" with ZERO terminal + zero
// pre-installed toolchain. Nothing touches the system; everything lives under
// <app_data>/runtime/ and is removed cleanly on disable.
//
// This is the foundation the HyperFrames Skill (and later MCP servers that need
// Node) stand on: ensure_node() / ensure_ffmpeg() are reusable. Each step emits
// a progress event on a channel so the UI narrates EXACTLY what's being
// installed + where (Mason's requirement: the user always knows what's happening).

use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

// Pinned Node LTS (Krypton). Portable tarball, no installer, no admin. We resolve
// the arch at runtime so Apple Silicon + Intel both work.
const NODE_VERSION: &str = "v24.19.0";
// Static macOS FFmpeg + FFprobe, ARCH-NATIVE. martin-riedl.de publishes separate
// arm64 and amd64 macOS static builds, each as a per-binary zip — so Apple Silicon
// gets a real arm64 binary (NOT x86_64-through-Rosetta, which flaked the render's
// -version probe), and we can fetch ffprobe too (HyperFrames needs BOTH). Fixed
// after Mason's live test surfaced "FFmpeg cannot start" + a missing ffprobe.
fn ffmpeg_base_url() -> &'static str {
    // martin-riedl arch tokens: arm64 | amd64.
    if cfg!(target_arch = "aarch64") {
        "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release"
    } else {
        "https://ffmpeg.martin-riedl.de/redirect/latest/macos/amd64/release"
    }
}
// HyperFrames npm package (the CLI + engine). Installed into runtime/hyperframes.
const HYPERFRAMES_PKG: &str = "hyperframes";

fn arch() -> &'static str {
    // Node's darwin tarball arch tokens: arm64 | x64.
    if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" }
}

/// <app_data>/runtime — the private toolchain root. Everything provisioned lives
/// here so uninstall = remove one subtree, and it never collides with the user's
/// own system Node/ffmpeg (we NEVER shell out to those; we use ours by abs path).
pub fn runtime_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| format!("app data dir: {e}"))?.join("runtime");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir runtime: {e}"))?;
    Ok(dir)
}

fn node_dir(app: &AppHandle) -> Result<PathBuf, String> { Ok(runtime_dir(app)?.join("node")) }
fn ffmpeg_dir(app: &AppHandle) -> Result<PathBuf, String> { Ok(runtime_dir(app)?.join("ffmpeg")) }
fn hyperframes_dir(app: &AppHandle) -> Result<PathBuf, String> { Ok(runtime_dir(app)?.join("hyperframes")) }

/// Abs path to the provisioned `node` binary (…/node/bin/node). None if absent.
pub fn node_bin(app: &AppHandle) -> Option<PathBuf> {
    let p = node_dir(app).ok()?.join("bin").join("node");
    p.is_file().then_some(p)
}
/// Abs path to the provisioned `npm` cli.js (run via node). None if absent.
pub fn npm_cli(app: &AppHandle) -> Option<PathBuf> {
    let p = node_dir(app).ok()?.join("lib").join("node_modules").join("npm").join("bin").join("npm-cli.js");
    p.is_file().then_some(p)
}
/// Abs path to the provisioned `ffmpeg` binary. None if absent.
pub fn ffmpeg_bin(app: &AppHandle) -> Option<PathBuf> {
    let p = ffmpeg_dir(app).ok()?.join("ffmpeg");
    p.is_file().then_some(p)
}
/// Abs path to the provisioned `ffprobe` binary. None if absent. HyperFrames
/// probes media with ffprobe, so both must be present for a render to work.
pub fn ffprobe_bin(app: &AppHandle) -> Option<PathBuf> {
    let p = ffmpeg_dir(app).ok()?.join("ffprobe");
    p.is_file().then_some(p)
}

fn emit(app: &AppHandle, channel: &str, phase: &str, note: &str, pct: Option<f64>) {
    let _ = app.emit(channel, &serde_json::json!({ "phase": phase, "note": note, "pct": pct }));
    eprintln!("[aygent][provision] {phase}: {note}");
}

/// Download a URL to `dest`, emitting progress on `channel` under `phase`.
async fn download_to(app: &AppHandle, channel: &str, phase: &str, label: &str, url: &str, dest: &Path) -> Result<(), String> {
    use futures_util::StreamExt;
    emit(app, channel, phase, &format!("Downloading {label}…"), Some(0.0));
    let client = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build().map_err(|e| format!("http: {e}"))?;
    let resp = client.get(url).header("Accept", "*/*").send().await
        .map_err(|e| format!("request {label}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download {label} failed: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?; }
    let mut file = std::fs::File::create(dest).map_err(|e| format!("create {label}: {e}"))?;
    let mut got: u64 = 0;
    let mut last = 0u64;
    let mut stream = resp.bytes_stream();
    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("stream {label}: {e}"))?;
        file.write_all(&bytes).map_err(|e| format!("write {label}: {e}"))?;
        got += bytes.len() as u64;
        if got - last > 2_000_000 {
            last = got;
            let pct = if total > 0 { Some(got as f64 / total as f64) } else { None };
            emit(app, channel, phase, &format!("Downloading {label}… {} MB", got / 1_000_000), pct);
        }
    }
    drop(file);
    Ok(())
}

/// Ensure a PORTABLE Node lives under runtime/node. Downloads the pinned LTS
/// darwin tarball for this arch + extracts it. Idempotent (returns fast if
/// node_bin already exists). We do NOT probe the system Node on purpose — a
/// pinned, known-good Node makes "it just works" reproducible across machines.
pub async fn ensure_node(app: &AppHandle, channel: &str) -> Result<PathBuf, String> {
    if let Some(p) = node_bin(app) { return Ok(p); }
    let a = arch();
    let stem = format!("node-{NODE_VERSION}-darwin-{a}");
    let url = format!("https://nodejs.org/dist/{NODE_VERSION}/{stem}.tar.gz");
    let rt = runtime_dir(app)?;
    let tarball = rt.join(format!("{stem}.tar.gz"));
    download_to(app, channel, "node", &format!("Node {NODE_VERSION} ({a})"), &url, &tarball).await?;

    emit(app, channel, "node", "Unpacking Node…", None);
    // Extract with the system `tar` (present on every macOS). Extract into rt,
    // then rename the versioned dir to runtime/node.
    let out = std::process::Command::new("tar")
        .arg("-xzf").arg(&tarball).arg("-C").arg(&rt)
        .output().map_err(|e| format!("tar node: {e}"))?;
    if !out.status.success() {
        return Err(format!("unpack node failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let extracted = rt.join(&stem);
    let node_home = node_dir(app)?;
    if node_home.exists() { let _ = std::fs::remove_dir_all(&node_home); }
    std::fs::rename(&extracted, &node_home).map_err(|e| format!("place node: {e}"))?;
    let _ = std::fs::remove_file(&tarball);
    node_bin(app).ok_or_else(|| "node binary missing after unpack".into())
}

/// Ensure BOTH `ffmpeg` and `ffprobe` (arch-native static builds) live under
/// runtime/ffmpeg. HyperFrames shells out to both; shipping only ffmpeg (or an
/// x86_64 binary that Rosetta chokes on) is the render failure Mason hit live.
/// Idempotent per-binary. Returns the ffmpeg path.
pub async fn ensure_ffmpeg(app: &AppHandle, channel: &str) -> Result<PathBuf, String> {
    // Fast path: both already present.
    if let (Some(mp), Some(_)) = (ffmpeg_bin(app), ffprobe_bin(app)) { return Ok(mp); }
    let dir = ffmpeg_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir ffmpeg: {e}"))?;
    let base = ffmpeg_base_url();
    let a = arch();

    // Each binary ships as its own zip containing a bare executable.
    for name in ["ffmpeg", "ffprobe"] {
        let bin = dir.join(name);
        if bin.is_file() { continue; } // idempotent per-binary
        let url = format!("{base}/{name}.zip");
        download_to(app, channel, "ffmpeg", &format!("{name} (macOS {a})"), &url, &dir.join(format!("{name}.zip"))).await?;
        emit(app, channel, "ffmpeg", &format!("Unpacking {name}…"), None);
        let zip = dir.join(format!("{name}.zip"));
        let out = std::process::Command::new("unzip")
            .arg("-o").arg(&zip).arg("-d").arg(&dir)
            .output().map_err(|e| format!("unzip {name}: {e}"))?;
        if !out.status.success() {
            return Err(format!("unpack {name} failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        if !bin.is_file() { return Err(format!("{name} binary missing after unpack")); }
        // chmod +x + strip quarantine so it launches without a Gatekeeper prompt.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&bin).map_err(|e| format!("stat {name}: {e}"))?.permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&bin, perm).map_err(|e| format!("chmod {name}: {e}"))?;
        }
        let _ = std::process::Command::new("xattr").arg("-dr").arg("com.apple.quarantine").arg(&bin).output();
        let _ = std::fs::remove_file(&zip);
    }
    ffmpeg_bin(app).ok_or_else(|| "ffmpeg missing after provisioning".into())
}

/// A PATH string with our provisioned node + ffmpeg dirs FIRST, so any npm/npx
/// run picks up OUR toolchain, not the user's system one (reproducible). Callers
/// pass this into the exec broker env when running hyperframes.
pub fn provisioned_path(app: &AppHandle) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(nd) = node_dir(app) { parts.push(nd.join("bin").to_string_lossy().to_string()); }
    if let Ok(fd) = ffmpeg_dir(app) { parts.push(fd.to_string_lossy().to_string()); }
    parts.push("/usr/bin".into());
    parts.push("/bin".into());
    parts.join(":")
}

/// Run `node <npm-cli> <args…>` in `cwd`, with our provisioned PATH, capturing
/// output. Used for `npm install`. We drive npm through OUR node so there is no
/// dependency on a system npm at all.
async fn run_npm(app: &AppHandle, channel: &str, cwd: &Path, args: &[&str]) -> Result<(), String> {
    let node = node_bin(app).ok_or("node not provisioned")?;
    let npm = npm_cli(app).ok_or("npm not provisioned")?;
    emit(app, channel, "npm", &format!("Running npm {}…", args.join(" ")), None);
    let mut cmd = std::process::Command::new(&node);
    cmd.arg(&npm).args(args).current_dir(cwd);
    cmd.env("PATH", provisioned_path(app));
    // Keep npm from trying to use a global prefix outside our tree.
    cmd.env("npm_config_prefix", node_dir(app)?);
    let out = cmd.output().map_err(|e| format!("spawn npm: {e}"))?;
    if !out.status.success() {
        let tail: String = String::from_utf8_lossy(&out.stderr).chars().rev().take(600).collect::<String>().chars().rev().collect();
        return Err(format!("npm {} failed: …{tail}", args.join(" ")));
    }
    Ok(())
}

// ── HyperFrames ─────────────────────────────────────────────────────────────

/// Is HyperFrames fully provisioned (node + ffmpeg + the hyperframes pkg)?
pub fn hyperframes_installed(app: &AppHandle) -> bool {
    node_bin(app).is_some()
        && ffmpeg_bin(app).is_some()
        && ffprobe_bin(app).is_some()
        && hyperframes_dir(app).map(|d| d.join("node_modules").join("hyperframes").is_dir()).unwrap_or(false)
}

/// Full HyperFrames provisioning: node → ffmpeg → npm install hyperframes. Emits
/// a running narration on `channel` (phase/note/pct) so the UI shows EXACTLY
/// what's installed + where. Idempotent at each step.
pub async fn hyperframes_install(app: &AppHandle, channel: &str) -> Result<serde_json::Value, String> {
    let rt = runtime_dir(app)?;
    emit(app, channel, "start", &format!("Installing into {} (nothing touches your system).", rt.display()), Some(0.0));

    ensure_node(app, channel).await?;
    emit(app, channel, "node-done", "Node ready.", Some(0.34));

    ensure_ffmpeg(app, channel).await?;
    emit(app, channel, "ffmpeg-done", "FFmpeg ready.", Some(0.60));

    // npm install hyperframes into runtime/hyperframes (a local, isolated install).
    let hf = hyperframes_dir(app)?;
    std::fs::create_dir_all(&hf).map_err(|e| format!("mkdir hyperframes: {e}"))?;
    // Seed a minimal package.json so npm installs LOCALLY into this dir.
    let pkg_json = hf.join("package.json");
    if !pkg_json.is_file() {
        std::fs::write(&pkg_json, "{\n  \"name\": \"aygent-hyperframes-host\",\n  \"private\": true\n}\n")
            .map_err(|e| format!("seed package.json: {e}"))?;
    }
    emit(app, channel, "hyperframes", "Installing HyperFrames (this can take a minute)…", Some(0.65));
    run_npm(app, channel, &hf, &["install", HYPERFRAMES_PKG, "--no-audit", "--no-fund"]).await?;

    emit(app, channel, "done", "HyperFrames is ready.", Some(1.0));
    Ok(serde_json::json!({
        "installed": true,
        "runtime_dir": rt.to_string_lossy(),
        "node": node_bin(app).map(|p| p.to_string_lossy().to_string()),
        "ffmpeg": ffmpeg_bin(app).map(|p| p.to_string_lossy().to_string()),
        "hyperframes_dir": hf.to_string_lossy(),
    }))
}

/// Uninstall HyperFrames. `keep_toolchain=true` removes only the hyperframes pkg
/// (leaves node/ffmpeg for other features, e.g. MCP servers); false removes the
/// WHOLE runtime dir (node + ffmpeg + hyperframes) to reclaim all the disk.
pub fn hyperframes_uninstall(app: &AppHandle, keep_toolchain: bool) -> Result<serde_json::Value, String> {
    let mut removed: Vec<String> = Vec::new();
    let hf = hyperframes_dir(app)?;
    if hf.is_dir() { std::fs::remove_dir_all(&hf).map_err(|e| format!("remove hyperframes: {e}"))?; removed.push(hf.to_string_lossy().to_string()); }
    if !keep_toolchain {
        for d in [node_dir(app)?, ffmpeg_dir(app)?] {
            if d.is_dir() { std::fs::remove_dir_all(&d).map_err(|e| format!("remove {}: {e}", d.display()))?; removed.push(d.to_string_lossy().to_string()); }
        }
    }
    eprintln!("[aygent][provision] hyperframes uninstalled (keep_toolchain={keep_toolchain}): {removed:?}");
    Ok(serde_json::json!({ "removed": removed }))
}

/// The bin path the HyperFrames Skill tells the agent to invoke, so the agent
/// runs OUR hyperframes with OUR node+ffmpeg on PATH. Returns the shell prefix
/// (env exports + the hyperframes bin) the Skill instructions reference.
pub fn hyperframes_invocation(app: &AppHandle) -> Option<(String, String)> {
    let hf = hyperframes_dir(app).ok()?;
    let bin = hf.join("node_modules").join(".bin").join("hyperframes");
    if !bin.is_file() { return None; }
    Some((provisioned_path(app), bin.to_string_lossy().to_string()))
}
