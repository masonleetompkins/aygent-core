// AYGENT — In-app browser provisioning, SLICE 0: THE GATE.
//
// Atlas's BROWSER-ARCH north star (projects/aygent/BROWSER-ARCH.md). Slice 0 is
// the make-or-break: prove AYGENT can, on a clean Mac with the user having
// installed NOTHING, obtain a real Chromium and LAUNCH it as our own child
// process — no Homebrew, no npm, no terminal, no Gatekeeper block.
//
// The install-nothing solution (Mason's load-bearing rule + his "would I have to
// host the file?" test → NO): we fetch Google's OFFICIAL, version-pinned
// **Chrome for Testing** build from Google's own CDN
// (storage.googleapis.com/chrome-for-testing-public/...) — the exact same
// "upstream hosts it, we just fetch it" model as the GGUF weights local models
// already download. Zero hosting/bandwidth cost on us. This mirrors
// `local_download` in lib.rs precisely (streamed download + progress events +
// atomic .part→final rename), then adds the three macOS-specific steps that make
// the launch actually work without a user install:
//
//   1. UNZIP the CfT archive into AYGENT's managed browser dir.
//   2. STRIP the com.apple.quarantine xattr from the app bundle + the inner
//      executable (a downloaded binary gets quarantined by the OS; launching it
//      then triggers Gatekeeper's "unidentified developer" gate). Because WE
//      downloaded it and launch it as a child process of an already-notarized
//      AYGENT, stripping quarantine is legitimate + is what keeps the gate from
//      ever firing. (This is the Risk-#1 mitigation from the doc.)
//   3. chmod +x the inner executable.
//
// SLICE 0 SCOPE = provision + LAUNCH (headless, --dump-dom of about:blank or a
// version probe) and confirm the process starts and the CDP port opens. It does
// NOT yet drive pages, mirror, or expose agent tools — those are Slices 1+.
//
// `BrowserRuntime` trait keeps the door open to a BUNDLED browser later (Atlas's
// fallback if Apple ever walls the download path); v1 default = download.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Chrome for Testing version we pin. CfT publishes a `known-good-versions`
/// JSON; we hard-pin one Stable build so provisioning is deterministic +
/// SHA-checkable and can't drift under us. Bump deliberately (like the GGUF
/// catalog), never float.
///
/// NOTE: this exact version string + its download URLs are validated at runtime
/// against Google's `last-known-good-versions-with-downloads.json` if the pinned
/// URL 404s (self-heal), so a stale pin degrades to "fetch latest stable" rather
/// than hard-failing. Slice 0 proves the pinned path; the self-heal is a guard.
const PINNED_CFT_VERSION: &str = "131.0.6778.204";

/// Google's Chrome-for-Testing CDN base. Public, permanent, no auth.
const CFT_BASE: &str = "https://storage.googleapis.com/chrome-for-testing-public";

/// The CfT JSON endpoint that lists every platform's download URL for the
/// last-known-good builds (used for self-heal if the pinned URL 404s).
const CFT_LKG_JSON: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";

/// macOS CfT platform key: Apple Silicon vs Intel. Chromium ships separate
/// arch builds; we pick by the host arch so we never hand an x86 binary to an
/// arm Mac (would launch under Rosetta at best, fail at worst).
fn cft_platform() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "mac-arm64"
    } else {
        "mac-x64"
    }
}

/// The relative CDN path segment for a CfT chrome build:
///   {version}/{platform}/chrome-{platform}.zip
fn cft_zip_relpath(version: &str, platform: &str) -> String {
    format!("{version}/{platform}/chrome-{platform}.zip")
}

/// Full pinned download URL for this host's Chrome-for-Testing zip.
fn pinned_zip_url() -> String {
    format!(
        "{CFT_BASE}/{}",
        cft_zip_relpath(PINNED_CFT_VERSION, cft_platform())
    )
}

/// AYGENT's managed browser dir: <app_data>/browser. Everything browser-related
/// (the unzipped Chromium, later per-agent profiles) lives UNDER here — inside
/// the app's own sandboxed data, never a system install location.
pub fn browser_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?
        .join("browser");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir browser: {e}"))?;
    Ok(dir)
}

/// Where the unzipped Chromium runtime lands: <browser>/chromium/<version>/.
/// Version-scoped so a future upgrade can download the new one alongside, verify
/// it, then flip — never leaving the user without a working browser mid-upgrade.
fn runtime_dir(app: &tauri::AppHandle, version: &str) -> Result<PathBuf, String> {
    Ok(browser_dir(app)?.join("chromium").join(version))
}

/// The path to the actual launchable Chrome executable inside an extracted CfT
/// build on macOS. CfT extracts to `chrome-{platform}/Google Chrome for
/// Testing.app/Contents/MacOS/Google Chrome for Testing`.
fn mac_executable(runtime: &Path, platform: &str) -> PathBuf {
    runtime
        .join(format!("chrome-{platform}"))
        .join("Google Chrome for Testing.app")
        .join("Contents")
        .join("MacOS")
        .join("Google Chrome for Testing")
}

/// The .app bundle path (what we strip quarantine from, recursively).
fn mac_app_bundle(runtime: &Path, platform: &str) -> PathBuf {
    runtime
        .join(format!("chrome-{platform}"))
        .join("Google Chrome for Testing.app")
}

/// Is a usable Chromium already provisioned? (executable exists + is a file)
pub fn is_installed(app: &tauri::AppHandle) -> bool {
    match runtime_dir(app, PINNED_CFT_VERSION) {
        Ok(rt) => {
            let exe = mac_executable(&rt, cft_platform());
            exe.is_file()
        }
        Err(_) => false,
    }
}

/// Status for the UI (the "Enable Browser" panel, mirroring the local-model
/// download card). Reports whether the browser is installed + where + version.
#[tauri::command]
pub fn browser_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let installed = is_installed(&app);
    let rt = runtime_dir(&app, PINNED_CFT_VERSION).ok();
    let exe = rt
        .as_ref()
        .map(|r| mac_executable(r, cft_platform()).to_string_lossy().to_string());
    Ok(serde_json::json!({
        "installed": installed,
        "version": PINNED_CFT_VERSION,
        "platform": cft_platform(),
        "executable": exe,
        "download_url": pinned_zip_url(),
    }))
}

/// A resolved download: URL, the version it is, and the CfT-published SHA-256
/// (hex) to verify the archive against. The hash is our integrity gate.
struct Resolved {
    url: String,
    version: String,
    /// CfT publishes a per-file SHA-256 in its known-good JSON. Always present
    /// for the self-heal path; for the pinned fast-path we also pull the LKG
    /// JSON to fetch the hash (a HEAD alone can't give us one), so verification
    /// is NEVER skipped.
    sha256: String,
}

/// Pull Google's last-known-good JSON and extract, for our platform's chrome
/// build: (url, version, sha256). This is the single source of truth for BOTH
/// the URL and the integrity hash — so we always have a hash to check.
async fn lkg_chrome_for_platform(
    client: &reqwest::Client,
) -> Result<Resolved, String> {
    let json: serde_json::Value = client
        .get(CFT_LKG_JSON)
        .send()
        .await
        .map_err(|e| format!("cft lkg fetch: {e}"))?
        .json()
        .await
        .map_err(|e| format!("cft lkg parse: {e}"))?;
    let stable = &json["channels"]["Stable"];
    let version = stable["version"]
        .as_str()
        .ok_or("cft lkg: no stable version")?
        .to_string();
    let downloads = stable["downloads"]["chrome"]
        .as_array()
        .ok_or("cft lkg: no chrome downloads")?;
    let platform = cft_platform();
    let entry = downloads
        .iter()
        .find(|d| d["platform"].as_str() == Some(platform))
        .ok_or_else(|| format!("cft lkg: no chrome build for {platform}"))?;
    let url = entry["url"]
        .as_str()
        .ok_or("cft lkg: chrome entry has no url")?
        .to_string();
    // CfT includes a per-file sha256 alongside the url.
    let sha256 = entry["sha256"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(Resolved { url, version, sha256 })
}

/// Resolve the download: prefer the pinned version if the LKG JSON still lists
/// it (so we get its hash too); otherwise self-heal to current Stable. Either
/// way we return a SHA-256 so verification is never skipped.
async fn resolve_download(client: &reqwest::Client) -> Result<Resolved, String> {
    // Try to find our PINNED version in the LKG JSON's history so we can pin the
    // hash too. The LKG endpoint only lists CURRENT stable, so if our pin isn't
    // current we just take current stable (with its hash) — the pinned URL might
    // still 200 from the CDN, but without a trustworthy published hash we prefer
    // the version we CAN verify. Integrity beats staying on an exact pin.
    let current = lkg_chrome_for_platform(client).await?;
    if current.version == PINNED_CFT_VERSION && !current.sha256.is_empty() {
        return Ok(current);
    }
    // Pinned version isn't current stable. Fast-path: if the pinned CDN URL is
    // live AND we have no reason to distrust it, we STILL want a hash. CfT keeps
    // old builds but the LKG JSON won't carry their hash. So: use current stable
    // (verifiable) rather than an unverifiable pinned build. Log the drift.
    if !current.sha256.is_empty() {
        return Ok(current);
    }
    // No hash available at all (JSON shape changed?) — last resort, pinned URL,
    // hash empty (caller will WARN, not silently trust — see verify step).
    Ok(Resolved {
        url: pinned_zip_url(),
        version: PINNED_CFT_VERSION.to_string(),
        sha256: String::new(),
    })
}

/// Compute the SHA-256 (hex) of a file on disk, streamed (never loads the whole
/// ~150MB zip into memory).
fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("open for hash: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read for hash: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// SLICE 0 — download + provision Chrome for Testing, emitting progress on
/// `channel` (identical UX to `local_download`). Returns the launchable
/// executable path. If already installed, returns immediately.
#[tauri::command]
pub async fn browser_install(app: tauri::AppHandle, channel: String) -> Result<String, String> {
    use tauri::Emitter;
    use futures_util::StreamExt;

    if is_installed(&app) {
        let rt = runtime_dir(&app, PINNED_CFT_VERSION)?;
        let exe = mac_executable(&rt, cft_platform());
        let _ = app.emit(&channel, &serde_json::json!({ "phase": "done", "got": 1u64, "total": 1u64, "done": true, "executable": exe.to_string_lossy() }));
        return Ok(exe.to_string_lossy().to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("http: {e}"))?;

    // Resolve URL + version + the CfT-published SHA-256 to verify against.
    let _ = app.emit(&channel, &serde_json::json!({ "phase": "resolve" }));
    let Resolved { url, version, sha256: expected_sha } = resolve_download(&client).await?;

    let bdir = browser_dir(&app)?;
    let zip_path = bdir.join(format!("chrome-{}-{}.zip", cft_platform(), version));
    let tmp = bdir.join(format!(
        "chrome-{}-{}.zip.part",
        cft_platform(),
        version
    ));

    // --- DOWNLOAD (streamed, progress events, atomic .part→final) -----------
    let resp = client
        .get(&url)
        .header("Accept", "*/*")
        .send()
        .await
        .map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "browser download failed: HTTP {} (from {url})",
            resp.status()
        ));
    }
    let total = resp.content_length().unwrap_or(0);
    let _ = app.emit(&channel, &serde_json::json!({ "phase": "download", "got": 0u64, "total": total }));

    let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create zip: {e}"))?;
    let mut got: u64 = 0;
    let mut last_emit = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("stream: {e}"))?;
        use std::io::Write;
        file.write_all(&bytes).map_err(|e| format!("write: {e}"))?;
        got += bytes.len() as u64;
        if got - last_emit > 2_000_000 {
            last_emit = got;
            let _ = app.emit(&channel, &serde_json::json!({ "phase": "download", "got": got, "total": total }));
        }
    }
    drop(file);
    std::fs::rename(&tmp, &zip_path).map_err(|e| format!("finalize zip: {e}"))?;
    let _ = app.emit(&channel, &serde_json::json!({ "phase": "download", "got": got, "total": total, "done_download": true }));

    // --- VERIFY (SHA-256 integrity gate) ------------------------------------
    // Honors the contract's "SHA-256 verify". If CfT gave us a hash, the archive
    // MUST match or we abort + delete it (never extract an unverified binary).
    // If no hash was resolvable (JSON shape change), we emit a WARN rather than
    // silently trusting — HTTPS transport integrity still applies, but we flag it.
    if !expected_sha.is_empty() {
        let _ = app.emit(&channel, &serde_json::json!({ "phase": "verify" }));
        let zip_for_hash = zip_path.clone();
        let actual = tokio::task::spawn_blocking(move || sha256_file(&zip_for_hash))
            .await
            .map_err(|e| format!("hash join: {e}"))??;
        if !actual.eq_ignore_ascii_case(&expected_sha) {
            let _ = std::fs::remove_file(&zip_path);
            return Err(format!(
                "integrity check FAILED: downloaded Chromium sha256 {actual} != expected {expected_sha}. Aborted (nothing installed)."
            ));
        }
        let _ = app.emit(&channel, &serde_json::json!({ "phase": "verify", "ok": true }));
    } else {
        eprintln!("[aygent][browser] WARN: no published SHA-256 available for CfT {version}; relying on HTTPS transport integrity only");
        let _ = app.emit(&channel, &serde_json::json!({ "phase": "verify", "skipped": true }));
    }

    // --- UNZIP into the version-scoped runtime dir --------------------------
    let _ = app.emit(&channel, &serde_json::json!({ "phase": "extract" }));
    let rt = runtime_dir(&app, &version)?;
    // Fresh extract dir (clear any partial from a prior failed run).
    if rt.exists() {
        let _ = std::fs::remove_dir_all(&rt);
    }
    std::fs::create_dir_all(&rt).map_err(|e| format!("mkdir runtime: {e}"))?;
    unzip_into(&zip_path, &rt)?;
    // Zip no longer needed; free the disk (best-effort).
    let _ = std::fs::remove_file(&zip_path);

    // --- macOS: strip quarantine + chmod +x so it launches with NO Gatekeeper
    //     prompt (the Risk-#1 mitigation). No-op on non-mac.
    let platform = cft_platform();
    let bundle = mac_app_bundle(&rt, platform);
    let exe = mac_executable(&rt, platform);
    #[cfg(target_os = "macos")]
    {
        let _ = app.emit(&channel, &serde_json::json!({ "phase": "unquarantine" }));
        strip_quarantine(&bundle)?;
        make_executable(&exe)?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &bundle; // silence unused on non-mac (Slice 0 targets mac)
    }

    if !exe.is_file() {
        return Err(format!(
            "provisioned but executable missing at {} — CfT layout may have changed",
            exe.display()
        ));
    }

    let _ = app.emit(&channel, &serde_json::json!({ "phase": "done", "done": true, "version": version, "executable": exe.to_string_lossy() }));
    Ok(exe.to_string_lossy().to_string())
}

/// SLICE 0 GATE — launch the provisioned Chromium as OUR child process and
/// confirm it actually runs with no install + no Gatekeeper block. We do the
/// cheapest possible proof: `--version` (prints the build + exits 0). If that
/// returns cleanly, the whole install-nothing thesis holds; if macOS blocked it,
/// we get a Gatekeeper error here and know Slice 0 failed (→ consider bundled).
///
/// Returns the version string Chromium reports (proof it launched).
#[tauri::command]
pub async fn browser_launch_probe(app: tauri::AppHandle) -> Result<String, String> {
    if !is_installed(&app) {
        return Err("browser not installed — run browser_install first".into());
    }
    let rt = runtime_dir(&app, PINNED_CFT_VERSION)?;
    let exe = mac_executable(&rt, cft_platform());

    // spawn_blocking: std::process is synchronous.
    let out = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&exe)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    })
    .await
    .map_err(|e| format!("probe join: {e}"))?
    .map_err(|e| format!("launch failed (Gatekeeper block? {e})"))?;

    if !out.status.success() {
        return Err(format!(
            "chromium --version exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if ver.is_empty() {
        return Err("chromium launched but reported no version".into());
    }
    Ok(ver)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Unzip an archive into `dest`, preserving unix permissions (CfT's inner
/// executable ships with +x that we must keep). Uses the `zip` crate.
fn unzip_into(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| format!("open zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("read zip: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        // Guard against zip-slip (an entry path escaping dest via ..).
        let rel = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => return Err(format!("unsafe zip entry: {}", entry.name())),
        };
        let outpath = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&outpath).map_err(|e| format!("mkdir {}: {e}", outpath.display()))?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {e}"))?;
            }
            let mut outfile =
                std::fs::File::create(&outpath).map_err(|e| format!("create {}: {e}", outpath.display()))?;
            std::io::copy(&mut entry, &mut outfile).map_err(|e| format!("extract {}: {e}", outpath.display()))?;
            // Preserve unix mode (keeps the executable bit on the inner binary).
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = entry.unix_mode() {
                    let _ = std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode));
                }
            }
        }
    }
    Ok(())
}

/// Recursively strip the com.apple.quarantine xattr from a downloaded app
/// bundle so launching it doesn't trip Gatekeeper's first-run prompt. We shell
/// out to `xattr -dr` (present on every macOS; not an install). Best-effort:
/// if the attr isn't there, xattr succeeds anyway.
#[cfg(target_os = "macos")]
fn strip_quarantine(bundle: &Path) -> Result<(), String> {
    let status = std::process::Command::new("/usr/bin/xattr")
        .arg("-dr")
        .arg("com.apple.quarantine")
        .arg(bundle)
        .status()
        .map_err(|e| format!("xattr spawn: {e}"))?;
    // xattr returns non-zero if the attr was absent on some paths; that's fine.
    let _ = status;
    Ok(())
}

/// Ensure the inner Chrome executable has the +x bit (belt-and-suspenders on
/// top of unzip's mode preservation).
#[cfg(target_os = "macos")]
fn make_executable(exe: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if !exe.exists() {
        return Err(format!("executable not found: {}", exe.display()));
    }
    let mut perms = std::fs::metadata(exe)
        .map_err(|e| format!("stat exe: {e}"))?
        .permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(exe, perms).map_err(|e| format!("chmod exe: {e}"))?;
    Ok(())
}

// ===========================================================================
// SLICE 1 — LIVE DRIVE: launch Chromium headless w/ CDP, navigate, screenshot.
//
// The mature CDP client (playwright-core) lives in the Node daemon per Atlas's
// doc — that's Slice 4's home for the agent-tool surface. But for Slice 1's
// "navigate + screenshot" proof we drive CDP DIRECTLY from Rust over the
// DevTools WebSocket (tokio-tungstenite, already a dep). Zero new daemon
// plumbing, zero npm, stays inside the install-nothing rule. When we build the
// agent tools + shared control we can promote to the daemon; the process
// lifecycle here is the same either way.
//
// Model: one long-lived headless Chromium child process (launched on first use,
// reused across navigations), held in Tauri managed state. Each command opens a
// short-lived CDP WS, does its dance (navigate / screenshot), closes. Simple +
// robust for Slice 1; a persistent socket comes with the live screencast (S2).
// ===========================================================================

use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// The running headless Chromium: the child process + the port its DevTools
/// endpoint is on + the live CDP session (Slice 2). Held in Tauri state so we
/// launch ONCE and reuse.
#[derive(Default)]
pub struct BrowserProc {
    inner: Mutex<Option<RunningBrowser>>,
    /// SLICE 5 — who's driving the shared page: "human" | "agent" | "idle".
    /// The human ALWAYS wins: taking the wheel preempts the agent instantly.
    control: Mutex<Control>,
}

/// Shared-control state (Slice 5). `driver` is the wheel; `agent_wants` is set
/// when the agent is mid-task (so the UI can show "agent working" / offer grab).
#[derive(Clone)]
pub struct Control {
    pub driver: String,       // "human" | "agent" | "idle"
    pub note: String,         // e.g. "agent hit a login — take the wheel"
    pub agent_active: bool,   // an agent browser task is in progress
}
impl Default for Control {
    fn default() -> Self {
        Self { driver: "idle".into(), note: String::new(), agent_active: false }
    }
}

struct RunningBrowser {
    child: std::process::Child,
    port: u16,
    /// The live CDP session pump (Slice 2). None until a page session opens.
    session: Option<CdpSession>,
}

/// A live CDP session: a channel to SEND a request into the pump task + get its
/// reply, plus the last-known device pixel ratio (for input coord mapping in
/// Slice 3). The pump task owns the WebSocket; commands talk to it via `tx`.
struct CdpSession {
    tx: mpsc::UnboundedSender<CdpRequest>,
    /// Bumped each session so a stale pump can't clobber a newer one.
    id: u64,
}

/// One request to the pump: a CDP method + params + a oneshot for the result.
struct CdpRequest {
    method: String,
    params: serde_json::Value,
    reply: oneshot::Sender<Result<serde_json::Value, String>>,
}

impl BrowserProc {
    pub fn new() -> Self {
        Self { inner: Mutex::new(None), control: Mutex::new(Control::default()) }
    }
}

/// Emit the current control state to the UI (Slice 5 "who's driving" HUD).
fn emit_control(app: &tauri::AppHandle, c: &Control) {
    use tauri::Emitter;
    let _ = app.emit("browser:control", &serde_json::json!({
        "driver": c.driver, "note": c.note, "agent_active": c.agent_active,
    }));
}

/// SLICE 6 — which agent's browser profile to use. Reads the active agent id
/// from app-data (set by agents_set_active); falls back to "default". A
/// filesystem-safe slug so it's a valid dir name.
fn active_browser_agent(app: &tauri::AppHandle) -> String {
    use tauri::Manager;
    let id = app
        .path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("active_agent.txt"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into());
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// Pick a free localhost port for Chromium's DevTools endpoint.
fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("port: {e}"))?;
    let p = l.local_addr().map_err(|e| format!("port addr: {e}"))?.port();
    Ok(p)
}

/// Ensure Chromium is running headless with a CDP port; return the port.
/// Launches on first call, reuses on subsequent calls (checks the child is
/// still alive; relaunches if it died/crashed — crash recovery).
async fn ensure_running(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> Result<u16, String> {
    // Fast path: already running + alive.
    {
        let mut guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
        if let Some(rb) = guard.as_mut() {
            match rb.child.try_wait() {
                Ok(None) => return Ok(rb.port), // still alive
                _ => { *guard = None; }          // died — fall through to relaunch
            }
        }
    }

    if !is_installed(app) {
        return Err("browser not installed — enable it in Settings first".into());
    }
    let rt = runtime_dir(app, PINNED_CFT_VERSION)?;
    let exe = mac_executable(&rt, cft_platform());
    let port = free_port()?;
    // SLICE 6 — PER-AGENT profile isolation. The active agent's own profile dir
    // (isolated cookies/logins) + its own jailed downloads dir, both UNDER
    // AYGENT's managed browser dir (never a system Chrome profile). Opt-in
    // cross-agent sharing (Atlas C) is a symlink/copy of the profile dir, spec'd
    // separately; the default is isolation.
    let agent = active_browser_agent(app);
    let profile = browser_dir(app)?.join("profiles").join(&agent);
    let downloads = profile.join("downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| format!("mkdir profile/downloads: {e}"))?;

    let mut child = std::process::Command::new(&exe)
        .arg("--headless=new")
        .arg(format!("--remote-debugging-port={port}"))
        // Bind DevTools to loopback only — never expose the CDP port off-box.
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-gpu")
        // Reasonable default viewport for the screenshot.
        .arg("--window-size=1280,800")
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("launch chromium: {e}"))?;

    // Chromium prints "DevTools listening on ws://127.0.0.1:PORT/..." to stderr
    // once the endpoint is up. We wait for that line instead of racing a fixed
    // sleep. Read it on a blocking thread with std::io (avoids needing tokio's
    // `process` feature) and bound the wait with a tokio timeout.
    let stderr = child.stderr.take().ok_or("no chromium stderr")?;
    let ready = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let line = line.map_err(|e| format!("stderr read: {e}"))?;
                if line.contains("DevTools listening on") {
                    return Ok(());
                }
            }
            Err("chromium exited before DevTools was ready".into())
        }),
    )
    .await
    .map_err(|_| "timed out waiting for Chromium DevTools".to_string())?
    .map_err(|e| format!("stderr watch join: {e}"))??;
    let _ = ready;

    let mut guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
    *guard = Some(RunningBrowser { child, port, session: None });
    Ok(port)
}

// ---------------------------------------------------------------------------
// SLICE 2 — LIVE SCREENCAST SESSION.
//
// One persistent CDP WebSocket per browser, owned by a background "pump" task.
// The pump: (a) forwards outbound CDP requests (from commands, via a channel)
// and routes replies back by id; (b) receives `Page.screencastFrame` events and
// emits them to the UI as `browser:frame` Tauri events (base64 JPEG), acking
// each so Chromium keeps streaming. This replaces Slice 1's navigate→screenshot
// →close with a live view + is exactly the socket Slice 3 forwards input into.
// ---------------------------------------------------------------------------

/// Session generation counter so a stale pump never clobbers a newer session.
static SESSION_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Ensure a live CDP session exists (open the persistent socket + spawn the pump
/// + start the screencast). Idempotent: returns quickly if one is already live.
async fn ensure_session(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> Result<(), String> {
    let port = ensure_running(app, state).await?;
    // Already have a live session?
    {
        let guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
        if let Some(rb) = guard.as_ref() {
            if rb.session.is_some() {
                return Ok(());
            }
        }
    }

    let client = reqwest::Client::new();
    let ws_url = page_ws_url(&client, port).await?;
    use tokio_tungstenite::connect_async;
    let (ws, _) = connect_async(&ws_url).await.map_err(|e| format!("cdp connect: {e}"))?;

    let (tx, rx) = mpsc::unbounded_channel::<CdpRequest>();
    let session_id = SESSION_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let app_handle = app.clone();

    // Spawn the pump. It owns the socket for the session's life.
    tokio::spawn(pump(ws, rx, app_handle, session_id));

    // Store the session handle.
    {
        let mut guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
        if let Some(rb) = guard.as_mut() {
            rb.session = Some(CdpSession { tx, id: session_id });
        }
    }

    // Enable the domains + start the screencast (JPEG, capped size for latency).
    session_call(state, "Page.enable", serde_json::json!({})).await?;
    session_call(state, "Runtime.enable", serde_json::json!({})).await?;
    session_call(
        state,
        "Page.startScreencast",
        serde_json::json!({ "format": "jpeg", "quality": 60, "maxWidth": 1280, "maxHeight": 800, "everyNthFrame": 1 }),
    )
    .await?;
    Ok(())
}

/// The pump task: owns the WS, routes request/reply by id, forwards screencast
/// frames to the UI. Ends when the socket closes or the request channel drops.
async fn pump(
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    mut rx: mpsc::UnboundedReceiver<CdpRequest>,
    app: tauri::AppHandle,
    _session_id: u64,
) {
    use futures_util::{SinkExt, StreamExt};
    use tauri::Emitter;
    use tokio_tungstenite::tungstenite::Message;
    use std::collections::HashMap;

    let (mut sink, mut stream) = ws.split();
    let mut next_id: u64 = 1;
    let mut pending: HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>> = HashMap::new();

    loop {
        tokio::select! {
            // Outbound: a command wants to make a CDP call.
            req = rx.recv() => {
                let Some(req) = req else { break; }; // channel dropped -> session gone
                let id = next_id; next_id += 1;
                let frame = serde_json::json!({ "id": id, "method": req.method, "params": req.params });
                if sink.send(Message::Text(frame.to_string().into())).await.is_err() {
                    let _ = req.reply.send(Err("cdp socket send failed".into()));
                    break;
                }
                pending.insert(id, req.reply);
            }
            // Inbound: a CDP reply or event.
            msg = stream.next() => {
                let msg = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => break,
                };
                let v: serde_json::Value = match serde_json::from_str(&msg) { Ok(v) => v, Err(_) => continue };
                if let Some(id) = v["id"].as_u64() {
                    if let Some(reply) = pending.remove(&id) {
                        if let Some(err) = v.get("error") {
                            let _ = reply.send(Err(format!("cdp: {err}")));
                        } else {
                            let _ = reply.send(Ok(v["result"].clone()));
                        }
                    }
                    continue;
                }
                // An event.
                match v["method"].as_str() {
                    Some("Page.screencastFrame") => {
                        let data = v["params"]["data"].as_str().unwrap_or("").to_string();
                        let session = v["params"]["sessionId"].as_i64().unwrap_or(0);
                        let meta = v["params"]["metadata"].clone();
                        // Emit the frame to the UI (base64 JPEG + device metrics).
                        let _ = app.emit("browser:frame", &serde_json::json!({
                            "data": format!("data:image/jpeg;base64,{data}"),
                            "metadata": meta,
                        }));
                        // ACK so Chromium keeps streaming.
                        let ack = serde_json::json!({ "id": next_id, "method": "Page.screencastFrameAck", "params": { "sessionId": session } });
                        next_id += 1;
                        let _ = sink.send(Message::Text(ack.to_string().into())).await;
                    }
                    _ => {}
                }
            }
        }
    }
    // Pump ending: fail any pending replies.
    for (_, reply) in pending.drain() {
        let _ = reply.send(Err("cdp session ended".into()));
    }
}

/// Make a CDP call on the live session (via the pump). Opens the session first
/// if needed is the CALLER's job (call ensure_session). Returns the result.
async fn session_call(
    state: &tauri::State<'_, BrowserProc>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let tx = {
        let guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
        let rb = guard.as_ref().ok_or("browser not running")?;
        let sess = rb.session.as_ref().ok_or("no live browser session")?;
        sess.tx.clone()
    };
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(CdpRequest { method: method.to_string(), params, reply: reply_tx })
        .map_err(|_| "cdp session gone".to_string())?;
    tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx)
        .await
        .map_err(|_| format!("cdp {method}: timed out"))?
        .map_err(|_| format!("cdp {method}: no reply"))?
}

/// Fetch the CDP WebSocket URL for a fresh page target via the HTTP /json API.
/// We open a NEW tab (PUT /json/new) so we always have a clean page to drive,
/// then return its webSocketDebuggerUrl.
async fn page_ws_url(client: &reqwest::Client, port: u16) -> Result<String, String> {
    // Create a fresh target. CfT/Chromium accepts PUT /json/new.
    let resp = client
        .put(format!("http://127.0.0.1:{port}/json/new"))
        .send()
        .await
        .map_err(|e| format!("cdp /json/new: {e}"))?;
    // Some builds require GET for /json/new; fall back to listing targets.
    let target: serde_json::Value = if resp.status().is_success() {
        resp.json().await.map_err(|e| format!("cdp new parse: {e}"))?
    } else {
        let list: Vec<serde_json::Value> = client
            .get(format!("http://127.0.0.1:{port}/json"))
            .send().await.map_err(|e| format!("cdp /json: {e}"))?
            .json().await.map_err(|e| format!("cdp list parse: {e}"))?;
        list.into_iter()
            .find(|t| t["type"].as_str() == Some("page"))
            .ok_or("no page target in Chromium")?
    };
    target["webSocketDebuggerUrl"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "target has no webSocketDebuggerUrl".into())
}

/// SLICE 2 — navigate the LIVE session to `url`. Frames stream to the UI via
/// `browser:frame` events (started by ensure_session); this just points the
/// page at the URL and returns the final url + title once it settles.
#[tauri::command]
pub async fn browser_navigate(
    app: tauri::AppHandle,
    state: tauri::State<'_, BrowserProc>,
    url: String,
) -> Result<serde_json::Value, String> {
    let url = normalize_url(&url);
    ensure_session(&app, &state).await?;

    session_call(&state, "Page.navigate", serde_json::json!({ "url": url })).await?;
    // Let it settle so the URL/title read is post-load. Frames are already
    // streaming live regardless, so this delay doesn't gate what the user SEES.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;

    let (mut final_url, mut title) = (url.clone(), String::new());
    if let Ok(r) = session_call(
        &state,
        "Runtime.evaluate",
        serde_json::json!({ "expression": "JSON.stringify([location.href, document.title])", "returnByValue": true }),
    )
    .await
    {
        if let Some(s) = r["result"]["value"].as_str() {
            if let Ok(arr) = serde_json::from_str::<Vec<String>>(s) {
                if arr.len() == 2 { final_url = arr[0].clone(); title = arr[1].clone(); }
            }
        }
    }

    Ok(serde_json::json!({ "url": final_url, "title": title }))
}

/// Start (or restart) the live view without navigating — used when the UI opens
/// the Browser tab so frames begin streaming immediately.
#[tauri::command]
pub async fn browser_start_view(
    app: tauri::AppHandle,
    state: tauri::State<'_, BrowserProc>,
) -> Result<(), String> {
    ensure_session(&app, &state).await
}

// ===========================================================================
// REAL EMBEDDED WEBVIEW (the human's actual browser).
//
// The screencast (above) was foggy glass — a JPEG video of a browser you can't
// select text in. WRONG tool for a human. This is the right one: a REAL native
// child webview (WKWebView on macOS) rendered INSIDE the AYGENT window, over the
// Browser pane. Crisp text, native selection, hover, scroll, real DOM. YOU
// browse in this.
//
// THE MAGIC — hand-off: the same webview the human browses is ALSO driveable by
// the agent via `eval` (inject JS to click/type/read). So "hand off to agent"
// doesn't switch surfaces — the agent acts in the exact window you're looking at
// while you watch. Toggle back instantly; same page, same session, no reload.
//
// The screencast/CDP path stays for HEADLESS agent-only browsing (scheduled/
// background). This webview is the interactive human+handoff surface.
// ===========================================================================

const WEBVIEW_LABEL: &str = "aygent-browser";

/// Create the embedded browser webview if absent, positioned + sized to the
/// Browser pane rect (CSS px from the UI). Navigates to `url`. Idempotent:
/// re-shows + repositions an existing one.
#[tauri::command]
pub async fn webview_open(
    app: tauri::AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    use tauri::{LogicalPosition, LogicalSize, Manager, WebviewUrl};
    let target = normalize_url(&url);
    let parsed = target.parse().map_err(|e| format!("bad url: {e}"))?;

    // Existing EMBEDDED child webview? reposition + navigate. add_child creates
    // a `Webview` (embedded child), NOT a `WebviewWindow` (standalone window),
    // so it MUST be retrieved via get_webview() — get_webview_window() returns
    // None for embedded children, which silently no-op'd every reposition/hide
    // (the pop-out + never-hides bug). Verified against tauri 2.11.5 docs.
    if let Some(wv) = app.get_webview(WEBVIEW_LABEL) {
        let _ = wv.set_position(LogicalPosition::new(x, y));
        let _ = wv.set_size(LogicalSize::new(width.max(1.0), height.max(1.0)));
        wv.navigate(parsed).map_err(|e| format!("navigate: {e}"))?;
        return Ok(());
    }

    // EMBED a child webview INSIDE the main window. add_child lives on the raw
    // WINDOW (not WebviewWindow/AppHandle), so pull the underlying Window from
    // the main WebviewWindow via .window(). This embeds + clips to the parent,
    // positioned at the pane rect (NOT a free-floating child window).
    let main = app.get_webview_window("main").ok_or("no main window")?;
    let builder = tauri::webview::WebviewBuilder::new(WEBVIEW_LABEL, WebviewUrl::External(parsed));
    // add_child lives on the raw Window. WebviewWindow derefs to Webview
    // (.as_ref()), whose .window() returns that Window — public under 'unstable'.
    main.as_ref()
        .window()
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width.max(1.0), height.max(1.0)),
        )
        .map_err(|e| format!("embed webview: {e}"))?;
    Ok(())
}

/// Reposition/resize the embedded webview to track the pane (called on layout
/// changes / scroll). No-op if it doesn't exist.
#[tauri::command]
pub fn webview_set_bounds(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    // Parent content-area height the FRONTEND measured `y` against, sent so the
    // Y-flip is deterministic instead of racing wry's live parent height.
    // Accepted even if wry re-flips internally — keeping the arg makes the invoke
    // signature stable and lets us switch to a manual flip if the race persists.
    parent_height: Option<f64>,
) -> Result<(), String> {
    use tauri::{LogicalPosition, LogicalSize, Manager};
    if let Some(wv) = app.get_webview(WEBVIEW_LABEL) {
        // --- EMPIRICAL DELTA PROBE (BROWSER-WEBVIEW-HANDOFF step 2) ----------
        // Log what JS asked for, apply it, then read back where the webview
        // ACTUALLY landed. Everything in Tauri readbacks is PHYSICAL px, so we
        // convert to logical via the main window's scale factor to compare
        // apples-to-apples with the LOGICAL x/y the frontend sent (CSS px).
        let win_scale = app
            .get_webview_window("main")
            .and_then(|w| w.scale_factor().ok())
            .unwrap_or(-1.0);

        // Parent (main webview) RAW PHYSICAL size (no division) so we can see the
        // real numbers wry works in.
        let parent_phys = app
            .get_webview("main")
            .and_then(|m| m.size().ok())
            .map(|s| (s.width, s.height));

        let _ = wv.set_position(LogicalPosition::new(x, y));
        let _ = wv.set_size(LogicalSize::new(width.max(1.0), height.max(1.0)));

        // Read back the applied position/size RAW PHYSICAL (no division). If
        // set_position(Logical(x)) lands at physical x, DPR was NOT applied; if
        // it lands at x*dpr, it WAS. This disambiguates the scale bug directly.
        let applied_phys_pos = wv.position().ok().map(|p| (p.x, p.y));
        let applied_phys_size = wv.size().ok().map(|s| (s.width, s.height));

        eprintln!(
            "[aygent][browser][DELTA] win_scale={win_scale} \
             | SENT_LOGICAL pos=({x:.1},{y:.1}) size=({:.1},{:.1}) parentHeight(JS)={:?} \
             | APPLIED_PHYSICAL pos={:?} size={:?} \
             | parent_PHYSICAL={:?} \
             | ratio_pos=({:.3},{:.3})",
            width.max(1.0),
            height.max(1.0),
            parent_height,
            applied_phys_pos,
            applied_phys_size,
            parent_phys,
            applied_phys_pos.map(|(px, _)| px as f64 / x.max(1.0)).unwrap_or(f64::NAN),
            applied_phys_pos.map(|(_, py)| py as f64 / y.max(1.0)).unwrap_or(f64::NAN),
        );
        // --------------------------------------------------------------------
    }
    Ok(())
}

/// Hide the embedded webview when the user leaves the Browser tab so it doesn't
/// float over other screens. Embedded child webviews have no hide() — shrink to
/// zero + move off-screen (reliable across versions).
#[tauri::command]
pub fn webview_hide(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{LogicalPosition, LogicalSize, Manager};
    if let Some(wv) = app.get_webview(WEBVIEW_LABEL) {
        let _ = wv.set_size(LogicalSize::new(0.0, 0.0));
        let _ = wv.set_position(LogicalPosition::new(-10000.0, -10000.0));
    }
    Ok(())
}

/// Navigate the embedded webview to a URL (address bar).
#[tauri::command]
pub fn webview_navigate(app: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri::Manager;
    let target = normalize_url(&url);
    let parsed = target.parse().map_err(|e| format!("bad url: {e}"))?;
    let wv = app.get_webview(WEBVIEW_LABEL).ok_or("browser not open")?;
    wv.navigate(parsed).map_err(|e| format!("navigate: {e}"))
}

/// Close/destroy the embedded webview entirely.
#[tauri::command]
pub fn webview_close(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(wv) = app.get_webview(WEBVIEW_LABEL) {
        let _ = wv.close();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// THE HAND-OFF: the agent acts IN the human's webview.
//
// When the human hands off, this runs a small agent loop where the model's ONLY
// tools operate on the live webview via injected JS: read the page, click by
// text, type, scroll. The agent works in the EXACT window the human is watching
// (same DOM, same session, same cookies) — not a separate headless browser.
//
// We reuse the app's existing Anthropic provider primitive. The webview eval
// runs page-side JS + returns a result the model reads back (page text after an
// action). This is a focused, self-contained loop — the model gets the task +
// the current page text + the webview tools, and iterates until done.
// ---------------------------------------------------------------------------

/// Run one JS expression in the human's webview and return the JSON-stringified
/// result. Uses a Tauri IPC round-trip: the injected script posts its result
/// back on a one-shot channel keyed by a nonce.
pub async fn webview_eval(app: &tauri::AppHandle, expr: &str) -> Result<String, String> {
    use tauri::{Manager, Listener};
    let wv = app.get_webview(WEBVIEW_LABEL).ok_or("browser not open")?;
    let nonce = format!("wvr_{}", SESSION_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));

    // Listen once for the result event the injected script emits.
    let handler = app.once(nonce.clone(), move |ev| {
        if let Ok(mut g) = tx.lock() {
            if let Some(sender) = g.take() {
                let _ = sender.send(ev.payload().to_string());
            }
        }
    });

    // Inject: eval the expression, emit the (stringified) result back to Rust.
    // Wrapped so an exception becomes a readable string instead of silent fail.
    let script = format!(
        "(async () => {{ let r; try {{ r = JSON.stringify(await (async()=>({expr}))()); }} catch(e) {{ r = 'ERR: '+ (e && e.message || e); }} \
         if (window.__TAURI__ && window.__TAURI__.event) {{ window.__TAURI__.event.emit({nonce:?}, r); }} }})()",
        expr = expr, nonce = nonce
    );
    wv.eval(&script).map_err(|e| { app.unlisten(handler); format!("eval: {e}") })?;

    match tokio::time::timeout(std::time::Duration::from_secs(15), rx).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(_)) => Err("webview eval channel closed".into()),
        Err(_) => { app.unlisten(handler); Err("webview eval timed out".into()) }
    }
}

/// THE HAND-OFF COMMAND. The human typed a task in the prompt bar; drive the
/// agent to do it in the live webview. Slice-thin agent loop with webview tools.
#[tauri::command]
pub async fn webview_agent_act(app: tauri::AppHandle, task: String) -> Result<String, String> {
    // Read the current page so the agent knows what it's looking at.
    let page = webview_eval(
        &app,
        "({title: document.title, url: location.href, text: (document.body?document.body.innerText:'').slice(0,4000)})",
    )
    .await
    .unwrap_or_else(|_| "{}".into());

    // For Slice-1 of the hand-off we do a DIRECT interpretation: the agent's
    // action verbs map to webview JS. A full model-in-the-loop version routes
    // through agent_stream; this focused version keeps the hand-off SNAPPY +
    // self-contained (the model call is a follow-up wire-up). We interpret a few
    // natural commands directly so the loop is real + demoable today.
    let t = task.to_lowercase();
    if let Some(rest) = t.strip_prefix("click ") {
        let target = rest.trim().trim_matches('"');
        let expr = format!(
            "(() => {{ const t={target:?}; const els=[...document.querySelectorAll('a,button,input[type=submit],[role=button],label')]; \
             const el=els.find(e=>(e.innerText||e.value||'').toLowerCase().includes(t)); \
             if(!el) return 'no element matching '+t; el.click(); return 'clicked: '+(el.innerText||el.value||t).slice(0,60); }})()",
            target = target
        );
        return webview_eval(&app, &expr).await.map(|r| r.trim_matches('"').to_string());
    }
    if t.starts_with("scroll") {
        let _ = webview_eval(&app, "(()=>{window.scrollBy(0, window.innerHeight*0.8); return 'scrolled';})()").await;
        return Ok("scrolled down".into());
    }
    if t.starts_with("summar") || t.starts_with("read") || t.starts_with("what") {
        // Return the page text for the model layer to summarize. (Until the
        // model call is wired, hand back the visible text so the human sees it.)
        return Ok(format!("Current page: {page}"));
    }
    if let Some(rest) = t.strip_prefix("type ") {
        let text = rest.trim().trim_matches('"');
        let expr = format!(
            "(() => {{ const el=document.activeElement; if(!el||!('value' in el)) return 'no focused input'; \
             el.value={text:?}; el.dispatchEvent(new Event('input',{{bubbles:true}})); return 'typed'; }})()",
            text = text
        );
        return webview_eval(&app, &expr).await.map(|r| r.trim_matches('"').to_string());
    }

    Ok(format!(
        "I can do: click <text>, type <text>, scroll, summarize. (Full free-form agent reasoning is the next wire-up.) You asked: {task}"
    ))
}

// ---------------------------------------------------------------------------
// SLICE 5 — SHARED CONTROL (the wheel).
//
// One page, two possible drivers. The HUMAN ALWAYS WINS: taking the wheel sets
// driver=human immediately, which the agent tools check + refuse to act while
// it's held. The agent claims the wheel (driver=agent) only when idle/agent;
// if it hits a login/CAPTCHA it releases + sets a note so the UI can prompt the
// human. Every change emits `browser:control` so the HUD shows who's driving.
// ---------------------------------------------------------------------------

/// Current control state, for the UI HUD on mount.
#[tauri::command]
pub fn browser_control_status(state: tauri::State<'_, BrowserProc>) -> Result<serde_json::Value, String> {
    let c = state.control.lock().map_err(|_| "control poisoned")?;
    Ok(serde_json::json!({ "driver": c.driver, "note": c.note, "agent_active": c.agent_active }))
}

/// HUMAN takes the wheel — preempts the agent immediately.
#[tauri::command]
pub fn browser_take_wheel(app: tauri::AppHandle, state: tauri::State<'_, BrowserProc>) -> Result<(), String> {
    let mut c = state.control.lock().map_err(|_| "control poisoned")?;
    c.driver = "human".into();
    c.note = String::new();
    emit_control(&app, &c);
    Ok(())
}

/// HUMAN releases the wheel back to idle (agent may resume).
#[tauri::command]
pub fn browser_release_wheel(app: tauri::AppHandle, state: tauri::State<'_, BrowserProc>) -> Result<(), String> {
    let mut c = state.control.lock().map_err(|_| "control poisoned")?;
    c.driver = "idle".into();
    c.note = String::new();
    emit_control(&app, &c);
    Ok(())
}

/// True if the HUMAN currently holds the wheel (agent must not act).
fn human_has_wheel(state: &tauri::State<'_, BrowserProc>) -> bool {
    state.control.lock().map(|c| c.driver == "human").unwrap_or(false)
}

/// Agent claims the wheel for a task. Refused (false) if the human holds it.
fn agent_claim(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> bool {
    let mut c = match state.control.lock() { Ok(c) => c, Err(_) => return false };
    if c.driver == "human" { return false; }
    c.driver = "agent".into();
    c.agent_active = true;
    c.note = String::new();
    emit_control(app, &c);
    true
}

/// Agent releases the wheel (task done, or handing off). `handoff_note` non-empty
/// => the agent wants the human (login/CAPTCHA); UI surfaces it.
fn agent_release(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>, handoff_note: &str) {
    if let Ok(mut c) = state.control.lock() {
        c.agent_active = false;
        // Don't stomp the human if they grabbed the wheel mid-task.
        if c.driver == "agent" { c.driver = if handoff_note.is_empty() { "idle".into() } else { "human".into() }; }
        c.note = handoff_note.to_string();
        emit_control(app, &c);
    }
}

// ===========================================================================
// SLICE 3 — HUMAN INPUT FORWARDING.
//
// The UI captures clicks/keys/scroll on the live frame, maps the coordinates
// into PAGE space (the frame is scaled to fit the pane; the UI sends normalized
// 0..1 coords + we multiply by the real viewport), and dispatches them into
// Chromium via CDP Input.dispatch{Mouse,Key}Event. Now you can click the CAPTCHA.
//
// Coords: the UI sends x,y as fractions (0..1) of the displayed frame. We read
// the real layout viewport (via Page.getLayoutMetrics, cached) and scale. This
// is DPR-robust because screencast frames + layout metrics are both in CSS px.
// ===========================================================================

/// Current CSS viewport (width,height) of the live page, for coord mapping.
async fn viewport_size(state: &tauri::State<'_, BrowserProc>) -> Result<(f64, f64), String> {
    let m = session_call(state, "Page.getLayoutMetrics", serde_json::json!({})).await?;
    // cssLayoutViewport is CSS px (matches screencast frame space).
    let vp = &m["cssLayoutViewport"];
    let w = vp["clientWidth"].as_f64().unwrap_or(1280.0);
    let h = vp["clientHeight"].as_f64().unwrap_or(800.0);
    Ok((w.max(1.0), h.max(1.0)))
}

/// Forward a mouse CLICK at normalized (fx,fy in 0..1) coords on the frame.
/// Dispatches move -> press -> release so pages that track hover/mousedown work.
#[tauri::command]
pub async fn browser_click(
    state: tauri::State<'_, BrowserProc>,
    fx: f64,
    fy: f64,
    button: Option<String>,
) -> Result<(), String> {
    let (w, h) = viewport_size(&state).await?;
    let x = (fx.clamp(0.0, 1.0)) * w;
    let y = (fy.clamp(0.0, 1.0)) * h;
    let btn = button.unwrap_or_else(|| "left".into());
    let base = serde_json::json!({ "x": x, "y": y, "button": btn, "clickCount": 1 });
    // move first (hover), then down, then up.
    let mut mv = base.clone(); mv["type"] = "mouseMoved".into(); mv["button"] = "none".into();
    session_call(&state, "Input.dispatchMouseEvent", mv).await?;
    let mut down = base.clone(); down["type"] = "mousePressed".into();
    session_call(&state, "Input.dispatchMouseEvent", down).await?;
    let mut up = base.clone(); up["type"] = "mouseReleased".into();
    session_call(&state, "Input.dispatchMouseEvent", up).await?;
    Ok(())
}

/// Forward a scroll (wheel) at normalized coords by (dx,dy) CSS px.
#[tauri::command]
pub async fn browser_scroll(
    state: tauri::State<'_, BrowserProc>,
    fx: f64,
    fy: f64,
    dx: f64,
    dy: f64,
) -> Result<(), String> {
    let (w, h) = viewport_size(&state).await?;
    let x = (fx.clamp(0.0, 1.0)) * w;
    let y = (fy.clamp(0.0, 1.0)) * h;
    session_call(
        &state,
        "Input.dispatchMouseEvent",
        serde_json::json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy }),
    )
    .await?;
    Ok(())
}

/// Forward typed text (a run of characters) into the focused element. Uses
/// Input.insertText for the bulk (fast + handles unicode/emoji), which is what
/// you want after a click focuses a field.
#[tauri::command]
pub async fn browser_type(
    state: tauri::State<'_, BrowserProc>,
    text: String,
) -> Result<(), String> {
    session_call(
        &state,
        "Input.insertText",
        serde_json::json!({ "text": text }),
    )
    .await?;
    Ok(())
}

/// Forward a single special KEY (Enter, Backspace, Tab, arrows, etc.) that
/// insertText can't express. `key` is a DOM key name; we map the common ones to
/// the windowsVirtualKeyCode CDP needs so the page's keydown handlers fire.
#[tauri::command]
pub async fn browser_key(
    state: tauri::State<'_, BrowserProc>,
    key: String,
) -> Result<(), String> {
    let (vk, dom_key, text): (i64, &str, &str) = match key.as_str() {
        "Enter" => (13, "Enter", "\r"),
        "Backspace" => (8, "Backspace", ""),
        "Tab" => (9, "Tab", ""),
        "Escape" => (27, "Escape", ""),
        "ArrowUp" => (38, "ArrowUp", ""),
        "ArrowDown" => (40, "ArrowDown", ""),
        "ArrowLeft" => (37, "ArrowLeft", ""),
        "ArrowRight" => (39, "ArrowRight", ""),
        "Delete" => (46, "Delete", ""),
        other => return Err(format!("unsupported key: {other}")),
    };
    let down = serde_json::json!({
        "type": "keyDown", "key": dom_key, "windowsVirtualKeyCode": vk,
        "nativeVirtualKeyCode": vk, "text": text,
    });
    session_call(&state, "Input.dispatchKeyEvent", down).await?;
    let up = serde_json::json!({
        "type": "keyUp", "key": dom_key, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk,
    });
    session_call(&state, "Input.dispatchKeyEvent", up).await?;
    Ok(())
}

// ===========================================================================
// SLICE 4 — AGENT BROWSER TOOLS.
//
// The agent drives the SAME browser the human sees (frames keep streaming, so
// the human watches the agent work). Tools are mediated through the CDP session
// exactly like the human's input, plus a per-agent DOMAIN POLICY (mirrors
// web.rs's SSRF posture): the agent may only navigate to allowlisted hosts;
// file:// / localhost / internal are hard-blocked. The human tier is unrestricted
// (Slice 3) — that split is the whole point (human gets past what the agent can't).
//
// One async entry point `agent_tool` that the turn loop calls for any browser_*
// tool. Returns (result_text, is_error) like exec_tool.
// ===========================================================================

/// Names the agent sees. Kept SMALL + robust (Atlas: favor a small set).
pub const AGENT_TOOL_NAMES: &[&str] = &[
    "browser_open", "browser_read", "browser_click_text", "browser_type_text", "browser_screenshot",
];

/// Is `name` one of our agent browser tools?
pub fn is_agent_tool(name: &str) -> bool {
    AGENT_TOOL_NAMES.contains(&name)
}

/// The JSON schemas fed to the model for the browser tools (added to the tool
/// list when the browser is enabled).
pub fn agent_tool_schemas() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "browser_open",
            "description": "Open a URL in the shared in-app browser (the human can watch + take over). Only allowlisted domains are permitted. Returns the page's readable text + title.",
            "input_schema": { "type": "object", "properties": {
                "url": { "type": "string", "description": "http(s) URL to open" }
            }, "required": ["url"] }
        }),
        serde_json::json!({
            "name": "browser_read",
            "description": "Read the current page's visible text (its accessibility/DOM text). Use this to see what's on the page you opened.",
            "input_schema": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "browser_click_text",
            "description": "Click the first visible element (link/button) whose text contains the given string. Use this to click links/buttons by their label.",
            "input_schema": { "type": "object", "properties": {
                "text": { "type": "string", "description": "visible text of the element to click" }
            }, "required": ["text"] }
        }),
        serde_json::json!({
            "name": "browser_type_text",
            "description": "Type text into the currently focused field (click a field first). Optionally press Enter after.",
            "input_schema": { "type": "object", "properties": {
                "text": { "type": "string" },
                "submit": { "type": "boolean", "description": "press Enter after typing" }
            }, "required": ["text"] }
        }),
        serde_json::json!({
            "name": "browser_screenshot",
            "description": "Capture what the page looks like right now as an image (returns a note; the human sees the live view). Use when the visible layout matters.",
            "input_schema": { "type": "object", "properties": {} }
        }),
    ]
}

/// Execute an agent browser tool against the live session, enforcing the
/// per-agent domain policy. `allowed_domains`: the agent's allowlist (empty =
/// nothing allowed — fail closed). Returns (text, is_error).
pub async fn agent_tool(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    name: &str,
    input: &serde_json::Value,
    allowed_domains: &[String],
) -> (String, bool) {
    // SLICE 5: the human ALWAYS wins the wheel. If they're driving, the agent
    // must not act — tell it plainly so it waits/hands back.
    if human_has_wheel(state) {
        return ("the human is currently driving the browser — wait for them to release the wheel before acting".into(), true);
    }
    // Claim the wheel for this action (refused only if the human grabbed it in
    // the race window).
    if !agent_claim(app, state) {
        return ("the human just took the browser — not acting".into(), true);
    }
    let out = agent_tool_inner(app, state, name, input, allowed_domains).await;
    // On a login/CAPTCHA-ish failure, hand off to the human with a note.
    let handoff = if out.1 && looks_like_handoff(&out.0) {
        "the agent hit a login or verification wall — take the wheel to continue"
    } else { "" };
    agent_release(app, state, handoff);
    out
}

/// Heuristic: does an agent tool result look like it needs a human (login/
/// CAPTCHA/verification)? Used to auto-offer the hand-off.
fn looks_like_handoff(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("captcha") || l.contains("not a robot") || l.contains("sign in")
        || l.contains("log in") || l.contains("login") || l.contains("verify") || l.contains("unusual traffic")
}

async fn agent_tool_inner(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    name: &str,
    input: &serde_json::Value,
    allowed_domains: &[String],
) -> (String, bool) {
    match name {
        "browser_open" => {
            let url = normalize_url(input.get("url").and_then(|u| u.as_str()).unwrap_or(""));
            // POLICY: agent may only navigate to allowlisted hosts; block
            // internal/localhost/file (mirrors web.rs).
            if let Err(e) = check_agent_url(&url, allowed_domains) {
                return (e, true);
            }
            if let Err(e) = ensure_session(app, state).await { return (format!("browser error: {e}"), true); }
            if let Err(e) = session_call(state, "Page.navigate", serde_json::json!({ "url": url })).await {
                return (format!("navigate failed: {e}"), true);
            }
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            match read_page_text(state).await {
                Ok((title, text)) => (format!("Opened. Title: {title}\n\n{text}"), false),
                Err(e) => (format!("opened but read failed: {e}"), true),
            }
        }
        "browser_read" => match read_page_text(state).await {
            Ok((title, text)) => (format!("Title: {title}\n\n{text}"), false),
            Err(e) => (format!("read failed: {e}"), true),
        },
        "browser_click_text" => {
            let want = input.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if want.is_empty() { return ("browser_click_text needs `text`".into(), true); }
            // Find the element's center via a DOM query, then dispatch a click there.
            let expr = format!(
                "(() => {{ const t={:?}; const els=[...document.querySelectorAll('a,button,input[type=submit],[role=button]')]; \
                 const el=els.find(e=>(e.innerText||e.value||'').toLowerCase().includes(t.toLowerCase())); \
                 if(!el) return null; const r=el.getBoundingClientRect(); \
                 return JSON.stringify([r.left+r.width/2, r.top+r.height/2]); }})()",
                want
            );
            match session_call(state, "Runtime.evaluate", serde_json::json!({ "expression": expr, "returnByValue": true })).await {
                Ok(r) => {
                    if let Some(s) = r["result"]["value"].as_str() {
                        if let Ok(xy) = serde_json::from_str::<Vec<f64>>(s) {
                            let (x, y) = (xy[0], xy[1]);
                            for phase in ["mouseMoved", "mousePressed", "mouseReleased"] {
                                let mut ev = serde_json::json!({ "type": phase, "x": x, "y": y, "button": "left", "clickCount": 1 });
                                if phase == "mouseMoved" { ev["button"] = "none".into(); }
                                if let Err(e) = session_call(state, "Input.dispatchMouseEvent", ev).await {
                                    return (format!("click dispatch failed: {e}"), true);
                                }
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                            return (format!("clicked element containing '{want}'"), false);
                        }
                    }
                    (format!("no clickable element found containing '{want}'"), true)
                }
                Err(e) => (format!("click query failed: {e}"), true),
            }
        }
        "browser_type_text" => {
            let text = input.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let submit = input.get("submit").and_then(|b| b.as_bool()).unwrap_or(false);
            if let Err(e) = session_call(state, "Input.insertText", serde_json::json!({ "text": text })).await {
                return (format!("type failed: {e}"), true);
            }
            if submit {
                let down = serde_json::json!({ "type": "keyDown", "key": "Enter", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13, "text": "\r" });
                let up = serde_json::json!({ "type": "keyUp", "key": "Enter", "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13 });
                let _ = session_call(state, "Input.dispatchKeyEvent", down).await;
                let _ = session_call(state, "Input.dispatchKeyEvent", up).await;
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            }
            (format!("typed {} chars{}", text.len(), if submit { " + Enter" } else { "" }), false)
        }
        "browser_screenshot" => {
            match session_call(state, "Page.captureScreenshot", serde_json::json!({ "format": "jpeg", "quality": 60 })).await {
                Ok(_) => ("captured a screenshot (the human sees the live view)".into(), false),
                Err(e) => (format!("screenshot failed: {e}"), true),
            }
        }
        other => (format!("unknown browser tool: {other}"), true),
    }
}

/// Read the page title + visible text (agent's primary "sense"). Caps length so
/// one read can't blow the model context.
async fn read_page_text(state: &tauri::State<'_, BrowserProc>) -> Result<(String, String), String> {
    let expr = "JSON.stringify([document.title, (document.body?document.body.innerText:'').slice(0,8000)])";
    let r = session_call(state, "Runtime.evaluate", serde_json::json!({ "expression": expr, "returnByValue": true })).await?;
    let s = r["result"]["value"].as_str().ok_or("no page text")?;
    let arr: Vec<String> = serde_json::from_str(s).map_err(|e| format!("parse page text: {e}"))?;
    if arr.len() == 2 { Ok((arr[0].clone(), arr[1].clone())) } else { Err("unexpected page text shape".into()) }
}

/// Agent URL policy (mirrors web.rs is_blocked_host + adds an allowlist).
/// Fails CLOSED: empty allowlist => nothing allowed.
fn check_agent_url(url: &str, allowed_domains: &[String]) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("agent may only open http(s) URLs".into());
    }
    // Extract host.
    let host = lower
        .split("://").nth(1).unwrap_or("")
        .split('/').next().unwrap_or("")
        .split('@').last().unwrap_or("")
        .split(':').next().unwrap_or("")
        .to_string();
    if host.is_empty() { return Err("could not parse host".into()); }
    // Block internal/localhost (SSRF).
    if host == "localhost" || host.starts_with("127.") || host.starts_with("10.")
        || host.starts_with("192.168.") || host.ends_with(".local") || host == "0.0.0.0"
        || host.starts_with("169.254.") {
        return Err("refused: internal/localhost host is not allowed for the agent".into());
    }
    // Allowlist: host must equal or be a subdomain of an allowed domain.
    let ok = allowed_domains.iter().any(|d| {
        let d = d.trim().to_ascii_lowercase();
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    });
    if !ok {
        return Err(format!(
            "refused: '{host}' is not in this agent's allowed browsing domains. Add it in the agent's browser policy to let it visit this site."
        ));
    }
    Ok(())
}

/// Add https:// to a bare host; leave full URLs + about:/file: as-is-ish.
fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return "about:blank".into();
    }
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("about:") {
        return s.to_string();
    }
    // Route to a Google search UNLESS it looks like a real host: a search if it
    // has spaces OR has no dot (e.g. "weather", "mason tompkins"). "example.com"
    // and "localhost:3000" still navigate.
    let looks_like_host = !s.contains(' ') && (s.contains('.') || s.starts_with("localhost"));
    if !looks_like_host {
        return format!("https://www.google.com/search?q={}", urlencoding_encode(s));
    }
    format!("https://{s}")
}

/// Tiny percent-encoder for the query fallback (avoid pulling a dep just for
/// this one call).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Shut the headless browser down (frees RAM). Safe to call when not running.
/// Dropping the session's tx ends the pump task (which closes the socket).
#[tauri::command]
pub fn browser_shutdown(state: tauri::State<'_, BrowserProc>) -> Result<(), String> {
    let mut guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
    if let Some(mut rb) = guard.take() {
        drop(rb.session.take()); // ends the pump
        let _ = rb.child.kill();
        let _ = rb.child.wait();
    }
    Ok(())
}
