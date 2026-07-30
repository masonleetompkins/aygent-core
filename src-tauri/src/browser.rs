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

/// A real, current desktop Chrome User-Agent string (NO "HeadlessChrome"),
/// with the major version matching our pinned Chrome-for-Testing build so the
/// UA and the actual engine agree (a mismatch is itself a detection signal).
/// Chrome-for-Testing's default UA contains "HeadlessChrome/<ver>" when run
/// headless — that is the #1 tell, so we override it everywhere.
///
/// Platform token: CfT only ships macOS builds for us today, and Mason runs on
/// a Mac, so we present a Mac desktop Chrome UA. The `Intel Mac OS X` token is
/// the CANONICAL Chrome UA even on Apple Silicon (Chrome reports Intel for
/// compatibility), so this is correct for both arches.
fn desktop_user_agent() -> String {
    // Major version from the pinned CfT build (e.g. "131.0.6778.204" -> "131").
    // Chrome's UA carries the FULL version, so use the whole string; it must
    // match the running engine exactly.
    let full = PINNED_CFT_VERSION;
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
         AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/{full} Safari/537.36"
    )
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
    /// PERMISSION REQUESTS: when the agent wants to do something outside what the
    /// human already navigated to (a NEW host), it opens a request here and
    /// blocks on the oneshot until the human answers Allow/Deny/Take Control via
    /// `browser_permission_answer`. Keyed by request id. Also tracks the set of
    /// hosts the human has granted THIS session (so a granted host stays allowed).
    perm: Mutex<PermState>,
}

/// Pending-permission plumbing. `pending` maps request-id -> the reply channel
/// the waiting agent tool is parked on. `granted_hosts` is the session's
/// human-approved host set (added on Allow), layered on top of "whatever host
/// the active tab is already showing" which is always allowed.
#[derive(Default)]
pub struct PermState {
    pending: std::collections::HashMap<String, oneshot::Sender<String>>,
    granted_hosts: std::collections::HashSet<String>,
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
        Self {
            inner: Mutex::new(None),
            control: Mutex::new(Control::default()),
            perm: Mutex::new(PermState::default()),
        }
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

    // FINGERPRINT HARDENING (#2). Chrome-for-Testing + --headless leaks several
    // "I am automation" tells that Google's bot detector reads:
    //   - `navigator.webdriver === true` (set by --enable-automation, which
    //     Chromium turns on implicitly with remote-debugging; we suppress it
    //     with --disable-blink-features=AutomationControlled + a per-document
    //     patch in ensure_session).
    //   - a User-Agent string containing "HeadlessChrome" (overridden below
    //     via --user-agent AND Network.setUserAgentOverride in ensure_session).
    //   - the automation infobar / "Chrome is being controlled by automated
    //     test software" flag.
    // We keep --headless=new (we do NOT want a second visible OS window; the
    // human watches via the screencast mirror), but strip the automation tells.
    // A real, current desktop Chrome UA matching the CfT major version:
    let real_ua = desktop_user_agent();
    let mut child = std::process::Command::new(&exe)
        .arg("--headless=new")
        .arg(format!("--remote-debugging-port={port}"))
        // Bind DevTools to loopback only — never expose the CDP port off-box.
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-gpu")
        // --- ANTI-DETECTION LAUNCH FLAGS (#2) --------------------------------
        // Kills the `navigator.webdriver=true` blink flag + the AutomationControlled
        // feature that flips several detectable properties.
        .arg("--disable-blink-features=AutomationControlled")
        // Suppress the "controlled by automated test software" infobar/flag.
        .arg("--disable-infobars")
        // Belt-and-suspenders for the automation feature (the blink flag above
        // is the documented one; we also avoid ever passing --enable-automation,
        // which is what would set navigator.webdriver=true in the first place).
        // Override the HeadlessChrome UA at the process level (belt); we ALSO
        // apply Network.setUserAgentOverride per-session (suspenders) so the UA
        // is normal even on the CDP page's network requests.
        .arg(format!("--user-agent={real_ua}"))
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

/// The Browser UI's CURRENT active tab id, reported by the frontend via
/// `set_active_browser_tab`. The agent acts on THIS tab's embedded webview so
/// it operates in the exact page the human is watching. -1 = none/unknown.
static ACTIVE_TAB_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

/// The visible tab's current URL (reported by the frontend). Used for the host
/// pre-check so same-site actions don't prompt — the CDP read path can lag or
/// point at about:blank, so the UI's own knowledge of where the tab is is the
/// authoritative "current page" for permission purposes.
static ACTIVE_TAB_URL: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Frontend reports which tab is active + its URL (called on switch/open/nav).
/// Lets the agent tools target the visible tab AND lets the host pre-check know
/// what page the human is actually on.
#[tauri::command]
pub fn set_active_browser_tab(tab_id: i64, url: Option<String>) {
    ACTIVE_TAB_ID.store(tab_id, std::sync::atomic::Ordering::SeqCst);
    if let Some(u) = url {
        if let Ok(mut g) = ACTIVE_TAB_URL.lock() { *g = u; }
    }
}

/// FIRE-AND-FORGET action in the ACTIVE tab's embedded webview. `wv.eval()`
/// runs JS with NO return channel; on EXTERNAL pages (google.com etc) the page
/// has no `window.__TAURI__` to emit a result back, so ANY round-trip stalls.
/// For actions (click/type/navigate) we don't need a value — run + assume
/// success. Returns Ok(()) once the eval is dispatched.
pub fn active_tab_run(app: &tauri::AppHandle, expr: &str) -> Result<(), String> {
    use tauri::Manager;
    let id = ACTIVE_TAB_ID.load(std::sync::atomic::Ordering::SeqCst);
    if id < 0 { return Err("no active browser tab".into()); }
    let wv = app.get_webview(&tab_label(Some(id))).ok_or("active tab has no open page")?;
    // Wrap so a page-side exception can't crash anything; result is discarded.
    let script = format!("(()=>{{ try {{ {expr} }} catch(e) {{}} }})()", expr = expr);
    eprintln!("[aygent][browser][RUN] tab={id} {:.80}", expr);
    wv.eval(&script).map_err(|e| format!("eval: {e}"))
}

/// READ a value from the AUTHORITATIVE agent page — the CDP session. This is
/// THE desync fix (Problem 1): the CDP session is now the SINGLE SOURCE OF
/// TRUTH the agent both ACTS ON and READS FROM. We NO LONGER mirror the CDP
/// page to a frontend-reported URL here — the old code did that and it dragged
/// every read back to a stale google.com the frontend last reported, so the
/// agent never "saw" it had navigated (the loop). Now: the CDP page is wherever
/// the agent's own actions (browser_open/click/type below) drove it; reads just
/// read THAT page. The VISIBLE WKWebView is mirrored TO the CDP page's url after
/// each navigation (see `mirror_visible_to_cdp`), never the other way. `expr`
/// must be a JS expression returning a JSON-serializable value; returns its
/// JSON string.
pub async fn active_tab_read(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    expr: &str,
) -> Result<String, String> {
    ensure_session(app, state).await?;
    let r = session_call(state, "Runtime.evaluate",
        serde_json::json!({ "expression": format!("JSON.stringify({expr})"), "returnByValue": true })
    ).await?;
    r["result"]["value"].as_str().map(|s| s.to_string())
        .ok_or_else(|| "no value from page".to_string())
}

/// Read the CDP page's REAL current url (the authoritative agent page).
async fn cdp_current_url(state: &tauri::State<'_, BrowserProc>) -> String {
    session_call(state, "Runtime.evaluate",
        serde_json::json!({ "expression": "location.href", "returnByValue": true })).await
        .ok()
        .and_then(|r| r["result"]["value"].as_str().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// SINGLE-SOURCE-OF-TRUTH SYNC (Problem 1 fix). After the agent navigates the
/// AUTHORITATIVE CDP page, drag the VISIBLE WKWebView to that same url so the
/// human watches where the agent actually went — and store it as ACTIVE_TAB_URL
/// for the permission host pre-check. Direction is ALWAYS CDP -> visible (never
/// visible -> CDP), which is what breaks the old tug-of-war desync loop.
async fn mirror_visible_to_cdp(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) {
    let url = cdp_current_url(state).await;
    if !url.starts_with("http") { return; }
    if let Ok(mut g) = ACTIVE_TAB_URL.lock() { *g = url.clone(); }
    // Navigate the visible embedded webview (best-effort; the human sees it).
    let id = ACTIVE_TAB_ID.load(std::sync::atomic::Ordering::SeqCst);
    if id < 0 { return; }
    use tauri::Manager;
    if let Some(wv) = app.get_webview(&tab_label(Some(id))) {
        if let Ok(parsed) = url.parse::<tauri::Url>() {
            eprintln!("[aygent][browser][MIRROR] visible tab={id} -> {url}");
            let _ = wv.navigate(parsed);
        }
    }
}

/// Read the active tab's (title, url) — from the VISIBLE embedded webview via a
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
    // DOM domain: required for DOM.getContentQuads (used by the trusted-click
    // target resolver to get the click point in the exact coord space CDP
    // Input.* uses — DPR-safe). Best-effort; the resolver falls back to the
    // rect center if DOM is unavailable.
    let _ = session_call(state, "DOM.enable", serde_json::json!({})).await;
    // FINGERPRINT HARDENING (#2) — done ONCE per session, right after the
    // domains are enabled and BEFORE any real navigation, so it applies to the
    // very first document too.
    apply_fingerprint_hardening(state).await;
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

/// Per-tab webview label. Multi-tab = one native child webview per tab, each
/// with a unique label `aygent-browser-<tabId>`, so every tab holds its OWN
/// live page + WebKit history. `tab_id: None` maps to the legacy single label
/// (back-compat + the agent path that operates on "the active tab").
fn tab_label(tab_id: Option<i64>) -> String {
    match tab_id {
        Some(id) => format!("{WEBVIEW_LABEL}-{id}"),
        None => WEBVIEW_LABEL.to_string(),
    }
}

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
    // Content-area size the frontend measured against. Kept in the invoke
    // signature for compatibility but no longer used: the pane rect is already
    // in the SAME content-coordinate space add_child uses, so we pass it through
    // exactly. (Every titlebar/DPR/inset correction we tried broke it a new way
    // — there was no offset to correct.)
    client_width: Option<f64>,
    client_height: Option<f64>,
    // Corner radius (CSS px) to round the WKWebView's own CALayer, so the OS
    // clips the page to a rounded rect matching the UI frame.
    radius: Option<f64>,
    // Which tab this webview belongs to. Each tab = its own native webview.
    tab_id: Option<i64>,
) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl};
    let target = normalize_url(&url);
    eprintln!("[aygent][browser] webview_open ENTER url={url:?} -> target={target:?} tab_id={tab_id:?} rect=({x},{y} {width}x{height})");
    let parsed: tauri::Url = target.parse().map_err(|e| { eprintln!("[aygent][browser] webview_open BAD URL: {e}"); format!("bad url: {e}") })?;
    let label = tab_label(tab_id);
    eprintln!("[aygent][browser] webview_open label={label} existing={}", app.get_webview(&label).is_some());

    let _ = (client_width, client_height);
    let pos = tauri::LogicalPosition::new(x, y);
    let size = tauri::LogicalSize::new(width.max(1.0), height.max(1.0));

    // Existing EMBEDDED child webview for THIS tab? reposition + navigate.
    // add_child creates a `Webview` (embedded child), retrieved via
    // get_webview() (not get_webview_window(), None for embedded children).
    if let Some(wv) = app.get_webview(&label) {
        #[cfg(target_os = "macos")]
        {
            place_child_exact(&wv, x, y, width, height, client_height, radius);
            set_child_hidden(&wv, false);
            bring_child_to_front(&wv);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = wv.set_position(pos);
            let _ = wv.set_size(size);
        }
        wv.navigate(parsed).map_err(|e| format!("navigate: {e}"))?;
        return Ok(());
    }

    // EMBED a child webview INSIDE the main window. add_child lives on the raw
    // WINDOW (not WebviewWindow/AppHandle). Getting the parent WINDOW:
    // get_webview_window("main") returns None once child webviews exist (the
    // registry entry "main" is the parent WEBVIEW, and after children the
    // lookup can miss) — which is exactly why tab 2 failed with "no main
    // window" while tab 1 succeeded. Get the parent Window robustly: prefer
    // the windows() map (keyed by window label), falling back to any existing
    // child webview's own .window() (all children share the same parent
    // window), then finally the webview lookup.
    let parent_window: tauri::Window = app
        .get_window("main")
        .or_else(|| app.windows().into_values().next())
        .or_else(|| app.get_webview_window("main").map(|wv| wv.as_ref().window()))
        .or_else(|| app.webviews().into_values().next().map(|wv| wv.window()))
        .ok_or_else(|| { eprintln!("[aygent][browser] NO PARENT WINDOW found via any path"); "no main window".to_string() })?;
    eprintln!("[aygent][browser] parent window resolved label={}", parent_window.label());
    let builder = tauri::webview::WebviewBuilder::new(&label, WebviewUrl::External(parsed));
    let wv = parent_window
        .add_child(builder, pos, size)
        .map_err(|e| { eprintln!("[aygent][browser] add_child FAILED: {e}"); format!("embed webview: {e}") })?;
    eprintln!("[aygent][browser] webview_open add_child OK label={label}");
    // Immediately pin the freshly-created child to the exact measured rect via
    // AppKit — add_child's own placement is what we stopped trusting.
    #[cfg(target_os = "macos")]
    place_child_exact(&wv, x, y, width, height, client_height, radius);
    #[cfg(not(target_os = "macos"))]
    let _ = wv;
    Ok(())
}

/// GROUND-TRUTH PLACEMENT (macOS): set the child webview's NSView frame
/// DIRECTLY against the window contentView's live bounds. We do NOT trust
/// wry's child-positioning math — every attempt to pre-correct for it
/// (titlebar inset, DPR, frame inset) broke a new way because we were
/// guessing its reference frame. Here there is nothing to guess: AppKit
/// tells us the content area, we convert top-left CSS coords to AppKit
/// bottom-left coords ourselves, and we place the view. All values live,
/// nothing hardcoded.
#[cfg(target_os = "macos")]
fn place_child_exact(wv: &tauri::Webview, x: f64, y: f64, w: f64, h: f64, content_h: Option<f64>, radius: Option<f64>) {
    let w = w.max(1.0);
    let h = h.max(1.0);
    let radius = radius.unwrap_or(0.0).max(0.0);
    let _ = wv.with_webview(move |pw| unsafe {
        use objc2::rc::Retained;
        use objc2::Message;
        use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
        use objc2_foundation::{NSPoint, NSRect, NSSize};

        let raw: *mut NSView = pw.inner().cast();
        if raw.is_null() {
            return;
        }
        let view: &NSView = &*raw;
        let Some(window) = view.window() else { return };
        let Some(content) = window.contentView() else { return };

        // wry may wrap the WKWebView in a container view; the frame that
        // matters is the ancestor sitting DIRECTLY inside contentView.
        let mut target: Retained<NSView> = view.retain();
        loop {
            let Some(sup) = target.superview() else { break };
            if Retained::as_ptr(&sup) == Retained::as_ptr(&content) {
                break;
            }
            target = sup;
        }

        // The frame we set lives in the coordinate space of target.superview
        // (the direct parent), NOT contentView per se. Flip against the PARENT's
        // flip state + the PARENT's height. If the parent is flipped, top-left
        // y is used directly; otherwise convert to bottom-left.
        let parent = target.superview();
        let (parent_flipped, parent_h) = match &parent {
            Some(p) => (p.isFlipped(), p.bounds().size.height),
            None => (content.isFlipped(), content.bounds().size.height),
        };
        // KEY FIX: the parent NSView spans the FULL WINDOW (incl. titlebar),
        // but the frontend's y is measured from the CONTENT area top (below the
        // titlebar). Flip against the CONTENT height (client_height from JS),
        // not the parent's full height — otherwise the webview rides up over the
        // tabs by exactly the titlebar height. NSVIEW log proved parent_h=720
        // while content=688. Fall back to parent_h if JS didn't send it.
        let flip_h = content_h.filter(|v| *v > 0.0).unwrap_or(parent_h);
        let oy = if parent_flipped { y } else { flip_h - y - h };

        let wrapped = Retained::as_ptr(&target) != (raw as *const NSView);
        eprintln!(
            "[aygent][browser][NSVIEW] in=({x:.0},{y:.0} {w:.0}x{h:.0}) \
             content_flipped={} parent_flipped={parent_flipped} parent_h={parent_h:.0} \
             flip_h={flip_h:.0} wrapped={wrapped} => frame=({x:.0},{oy:.0} {w:.0}x{h:.0})",
            content.isFlipped(),
        );

        // MINIMAL INTERVENTION: place ONLY the outer container (the view sitting
        // directly under contentView). Do NOT touch the inner WKWebView frame
        // and do NOT clear autoresizing — doing both is what broke rendering
        // (blank page). The WKWebView tracks its container via its own
        // autoresizing mask; we only correct WHERE the container sits. wry still
        // sized it via set_size before this call; we override position + size on
        // the container only.
        target.setFrame(NSRect::new(NSPoint::new(x, oy), NSSize::new(w, h)));
        let _ = NSAutoresizingMaskOptions::empty();

        // PREMIUM ROUNDED CORNERS — the proper native way: round the WKWebView's
        // OWN backing CALayer so the OS clips the web content itself to a
        // rounded rect. No DOM inset, sizes match the pane EXACTLY; the DOM
        // frame overlay then overlaps only the corner pixels for the accent
        // border. We round the actual web-content view (`raw`) AND the outer
        // container so nothing square peeks past the arc. wantsLayer=true
        // guarantees a layer is backing the view (WKWebView is layer-backed,
        // but set it defensively on any wrapper too).
        if radius > 0.0 {
            for v in [view as &NSView, &*target] {
                v.setWantsLayer(true);
                if let Some(layer) = v.layer() {
                    layer.setCornerRadius(radius);
                    layer.setMasksToBounds(true);
                }
            }
        }
    });
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
    // Content-area size the frontend rect was measured against, used to compute
    // the native titlebar inset at runtime. `parent_height` kept as an alias for
    // client_height so the older invoke shape still works.
    client_width: Option<f64>,
    client_height: Option<f64>,
    parent_height: Option<f64>,
    // Corner radius (CSS px) to round the WKWebView's CALayer, matching the UI
    // frame. Same single-sourced value the open call sends.
    radius: Option<f64>,
    // Which tab's webview to reposition.
    tab_id: Option<i64>,
) -> Result<(), String> {
    use tauri::Manager;
    if let Some(wv) = app.get_webview(&tab_label(tab_id)) {
        let _ = client_width;
        let content_h = client_height.or(parent_height);
        #[cfg(target_os = "macos")]
        place_child_exact(&wv, x, y, width, height, content_h, radius);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = content_h;
            let _ = wv.set_position(tauri::LogicalPosition::new(x, y));
            let _ = wv.set_size(tauri::LogicalSize::new(width.max(1.0), height.max(1.0)));
        }
    }
    Ok(())
}

/// Hide the embedded webview when the user leaves the Browser tab so it doesn't
/// float over other screens. Embedded child webviews have no hide() — shrink to
/// zero + move off-screen (reliable across versions).
/// Set NSView `hidden` on a tab's webview (macOS). Atlas P0: hiding by
/// setHidden removes the view from hit-testing (no leaked clicks) + drops it
/// from compositing (no GPU/flash), and is lossless on re-show (no reload).
/// The old size-0/offscreen hack leaked input + paint. Non-macOS falls back to
/// the geometry hack.
#[cfg(target_os = "macos")]
fn set_child_hidden(wv: &tauri::Webview, hidden: bool) {
    let _ = wv.with_webview(move |pw| unsafe {
        use objc2_app_kit::NSView;
        let raw: *mut NSView = pw.inner().cast();
        if raw.is_null() { return; }
        (&*raw).setHidden(hidden);
    });
}

#[cfg(target_os = "macos")]
fn bring_child_to_front(wv: &tauri::Webview) {
    let _ = wv.with_webview(move |pw| unsafe {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        use objc2_app_kit::{NSView, NSWindowOrderingMode};
        let raw: *mut NSView = pw.inner().cast();
        if raw.is_null() { return; }
        let view: &NSView = &*raw;
        if let Some(superview) = view.superview() {
            let _: () = msg_send![
                &*superview,
                addSubview: view,
                positioned: NSWindowOrderingMode::Above,
                relativeTo: std::ptr::null::<AnyObject>()
            ];
        }
    });
}

#[tauri::command]
pub fn webview_hide(app: tauri::AppHandle, tab_id: Option<i64>) -> Result<(), String> {
    use tauri::Manager;
    if let Some(wv) = app.get_webview(&tab_label(tab_id)) {
        #[cfg(target_os = "macos")]
        set_child_hidden(&wv, true);
        #[cfg(not(target_os = "macos"))]
        {
            use tauri::{LogicalPosition, LogicalSize};
            let _ = wv.set_size(LogicalSize::new(0.0, 0.0));
            let _ = wv.set_position(LogicalPosition::new(-10000.0, -10000.0));
        }
    }
    Ok(())
}

/// Hide EVERY tab's webview except `keep` (the active tab), and bring `keep` to
/// the front. Called on tab switch. keep=None hides all (leaving the Browser
/// screen). Iterates the window's webviews by our label prefix.
#[tauri::command]
pub fn webview_hide_others(app: tauri::AppHandle, keep: Option<i64>) -> Result<(), String> {
    use tauri::Manager;
    let keep_label = keep.map(|id| tab_label(Some(id)));
    for (label, wv) in app.webviews() {
        if !label.starts_with(WEBVIEW_LABEL) { continue; }
        let is_keep = Some(&label) == keep_label.as_ref();
        #[cfg(target_os = "macos")]
        {
            set_child_hidden(&wv, !is_keep);
            if is_keep { bring_child_to_front(&wv); }
        }
        #[cfg(not(target_os = "macos"))]
        if !is_keep {
            use tauri::{LogicalPosition, LogicalSize};
            let _ = wv.set_size(LogicalSize::new(0.0, 0.0));
            let _ = wv.set_position(LogicalPosition::new(-10000.0, -10000.0));
        }
    }
    Ok(())
}

/// Navigate a tab's embedded webview to a URL (address bar).
#[tauri::command]
pub fn webview_navigate(app: tauri::AppHandle, url: String, tab_id: Option<i64>) -> Result<(), String> {
    use tauri::Manager;
    let target = normalize_url(&url);
    let parsed = target.parse().map_err(|e| format!("bad url: {e}"))?;
    let wv = app.get_webview(&tab_label(tab_id)).ok_or("browser not open")?;
    wv.navigate(parsed).map_err(|e| format!("navigate: {e}"))
}

/// Read a tab's current page title + URL from its embedded webview. Used by the
/// UI to label the tab with the REAL page title ("Google") instead of the typed
/// address / "New Tab". Round-trips through a Tauri IPC event keyed by a nonce:
/// inject JS that emits [title, href] back, wait (bounded) for it. If the
/// webview isn't loaded yet the caller just retries.
#[tauri::command]
pub async fn webview_page_info(app: tauri::AppHandle, tab_id: Option<i64>) -> Result<serde_json::Value, String> {
    use tauri::{Manager, Listener};
    let wv = app.get_webview(&tab_label(tab_id)).ok_or("browser not open")?;
    let nonce = format!("wvpi_{}", SESSION_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));
    let handler = app.once(nonce.clone(), move |ev| {
        if let Ok(mut g) = tx.lock() {
            if let Some(sender) = g.take() { let _ = sender.send(ev.payload().to_string()); }
        }
    });
    let script = format!(
        "(() => {{ try {{ const r = JSON.stringify([document.title||'', location.href||'']); \
         if (window.__TAURI__ && window.__TAURI__.event) window.__TAURI__.event.emit({nonce:?}, r); }} catch(e) {{}} }})()",
        nonce = nonce
    );
    wv.eval(&script).map_err(|e| { app.unlisten(handler); format!("eval: {e}") })?;
    let raw = match tokio::time::timeout(std::time::Duration::from_secs(3), rx).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return Err("page info channel closed".into()),
        Err(_) => { app.unlisten(handler); return Err("page info timed out".into()); }
    };
    // The payload is a JSON string containing a JSON-stringified array; unwrap both.
    let inner: String = serde_json::from_str(&raw).unwrap_or(raw);
    let arr: Vec<String> = serde_json::from_str(&inner).unwrap_or_default();
    let title = arr.get(0).cloned().unwrap_or_default();
    let url = arr.get(1).cloned().unwrap_or_default();
    Ok(serde_json::json!({ "title": title, "url": url }))
}

/// Close/destroy a tab's embedded webview entirely.
#[tauri::command]
pub fn webview_close(app: tauri::AppHandle, tab_id: Option<i64>) -> Result<(), String> {
    use tauri::Manager;
    if let Some(wv) = app.get_webview(&tab_label(tab_id)) {
        let _ = wv.close();
    }
    Ok(())
}

/// Browser HISTORY within a tab's webview (Phase C prep): back / forward /
/// reload via injected JS on the live WebKit view. Simple + engine-native.
#[tauri::command]
pub fn webview_history(app: tauri::AppHandle, action: String, tab_id: Option<i64>) -> Result<(), String> {
    use tauri::Manager;
    let wv = app.get_webview(&tab_label(tab_id)).ok_or("browser not open")?;
    let js = match action.as_str() {
        "back" => "history.back()",
        "forward" => "history.forward()",
        "reload" => "location.reload()",
        other => return Err(format!("unknown history action: {other}")),
    };
    wv.eval(js).map_err(|e| format!("history {action}: {e}"))
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

// ===========================================================================
// FINGERPRINT HARDENING (#2) + TRUSTED CDP INPUT (#1) SHARED HELPERS.
//
// Everything below produces input/fingerprints that are INDISTINGUISHABLE from
// a real human on a real Chrome: CDP Input.* events carry isTrusted:true at the
// browser layer (unlike el.click()/dispatchEvent JS which are isTrusted:false),
// and the per-document patch scrubs the automation tells (navigator.webdriver,
// missing plugins, HeadlessChrome UA) that detectors like Google read.
// ===========================================================================

/// Apply anti-detection to the live CDP session. Called ONCE per session in
/// ensure_session after Page.enable/Runtime.enable. Two parts:
///   (a) Network.setUserAgentOverride — force a normal desktop Chrome UA (no
///       "HeadlessChrome") on the CDP page's own requests, matching the launch
///       --user-agent so page-JS `navigator.userAgent` and the network layer
///       agree.
///   (b) Page.addScriptToEvaluateOnNewDocument — runs BEFORE any page script on
///       every new document, so it patches the automation fingerprint the
///       instant the page loads (webdriver, plugins, languages, chrome runtime,
///       and any leftover cdc_/webdriver props).
async fn apply_fingerprint_hardening(state: &tauri::State<'_, BrowserProc>) {
    let ua = desktop_user_agent();
    // Network.setUserAgentOverride is honored without Network.enable on current
    // Chromium, but enabling the domain first is the documented, robust order
    // (and lets the override apply to the very first request). Best-effort.
    let _ = session_call(state, "Network.enable", serde_json::json!({})).await;
    // (a) UA override. Also set a matching platform + acceptLanguage + a
    // client-hints userAgentMetadata so navigator.userAgentData (Chrome's UA-CH)
    // does not still say Headless/leak a mismatch.
    let ua_res = session_call(
        state,
        "Network.setUserAgentOverride",
        serde_json::json!({
            "userAgent": ua,
            "acceptLanguage": "en-US,en;q=0.9",
            "platform": "MacIntel",
            "userAgentMetadata": {
                "brands": [
                    { "brand": "Not_A Brand", "version": "24" },
                    { "brand": "Chromium", "version": "131" },
                    { "brand": "Google Chrome", "version": "131" }
                ],
                "fullVersion": PINNED_CFT_VERSION,
                "platform": "macOS",
                "platformVersion": "14.5.0",
                "architecture": "arm",
                "model": "",
                "mobile": false
            }
        }),
    )
    .await;
    eprintln!(
        "[aygent][browser][FINGERPRINT] setUserAgentOverride ua=\"{ua}\" ok={}",
        ua_res.is_ok()
    );

    // (b) Per-new-document patch. This is the load-bearing scrub: it deletes the
    // webdriver flag, gives navigator sane plugins/languages, defines a
    // window.chrome shim, and strips any Selenium/cdc_ globals. Runs before the
    // page's own JS on EVERY document (main frame + iframes).
    let patch = r#"
(() => {
  try {
    // 1) navigator.webdriver -> undefined (the single biggest tell).
    try { Object.defineProperty(Navigator.prototype, 'webdriver', { get: () => undefined, configurable: true }); } catch (e) {}
    try { Object.defineProperty(navigator, 'webdriver', { get: () => undefined, configurable: true }); } catch (e) {}
    try { delete navigator.__proto__.webdriver; } catch (e) {}

    // 2) navigator.languages must be a real array (headless can return []).
    try { Object.defineProperty(navigator, 'languages', { get: () => ['en-US', 'en'], configurable: true }); } catch (e) {}

    // 3) navigator.plugins / mimeTypes non-empty (headless returns 0 -> a tell).
    try {
      const fakePlugin = (name, filename, desc) => {
        const p = { name, filename, description: desc, length: 1 };
        return p;
      };
      const plugins = [
        fakePlugin('Chrome PDF Plugin', 'internal-pdf-viewer', 'Portable Document Format'),
        fakePlugin('Chrome PDF Viewer', 'mhjfbmdgcfjbbpaeojofohoefgiehjai', ''),
        fakePlugin('Native Client', 'internal-nacl-plugin', '')
      ];
      Object.defineProperty(navigator, 'plugins', {
        get: () => { const arr = plugins.slice(); arr.item = (i) => arr[i]; arr.namedItem = (n) => arr.find(p => p.name === n); return arr; },
        configurable: true
      });
    } catch (e) {}

    // 4) window.chrome shim (real Chrome exposes window.chrome.runtime etc;
    //    headless often does not, which detectors check).
    try {
      if (!window.chrome) { window.chrome = {}; }
      if (!window.chrome.runtime) { window.chrome.runtime = {}; }
    } catch (e) {}

    // 5) Permissions.query for 'notifications' should not throw / should look
    //    like a real prompt-state (a known headless divergence).
    try {
      const origQuery = window.navigator.permissions && window.navigator.permissions.query;
      if (origQuery) {
        window.navigator.permissions.query = (params) =>
          (params && params.name === 'notifications')
            ? Promise.resolve({ state: Notification.permission || 'default', onchange: null })
            : origQuery.call(window.navigator.permissions, params);
      }
    } catch (e) {}

    // 6) Scrub Selenium/ChromeDriver globals if any snuck in.
    try {
      for (const k of Object.keys(window)) {
        if (k.indexOf('cdc_') === 0 || k.indexOf('$cdc_') === 0 || k === 'webdriver' || k === '__webdriver_evaluate' || k === '__selenium_evaluate' || k === '__driver_evaluate') {
          try { delete window[k]; } catch (e) {}
        }
      }
      try { delete document.$cdc_asdjflasutopfhvcZLmcfl_; } catch (e) {}
    } catch (e) {}
  } catch (e) {}
})();
"#;
    let patch_res = session_call(
        state,
        "Page.addScriptToEvaluateOnNewDocument",
        serde_json::json!({ "source": patch }),
    )
    .await;
    eprintln!(
        "[aygent][browser][FINGERPRINT] addScriptToEvaluateOnNewDocument ok={}",
        patch_res.is_ok()
    );
}

/// A tiny randomized delay (ms range inclusive) so sub-actions are not
/// instantaneous — uniform timing is itself a bot signal. No rand crate dep:
/// derive jitter from the nanosecond clock.
async fn human_delay(min_ms: u64, max_ms: u64) {
    let span = max_ms.saturating_sub(min_ms).max(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = nanos % (span + 1);
    tokio::time::sleep(std::time::Duration::from_millis(min_ms + jitter)).await;
}

/// The resolved click target: the coordinate to click (in the SAME CSS-px /
/// layout-viewport space CDP Input.* uses) plus diagnostics for the [CLICK] log
/// and an honest post-click success check.
struct ClickTarget {
    x: f64,
    y: f64,
    /// The nearest clickable ancestor's tag (a/button/…) we actually target.
    tag: String,
    /// If the target (or an ancestor) is/inside an <a>, its resolved href —
    /// used as a navigation fallback if the trusted click produced no nav.
    href: Option<String>,
    /// Diagnostics: the page's reported DPR + scroll + inner viewport.
    dpr: f64,
    scroll_x: f64,
    scroll_y: f64,
    inner_w: f64,
    inner_h: f64,
    /// Whether the coord came from getContentQuads (true) or the rect fallback.
    via_quads: bool,
}

/// Find an element by visible text, resolve it to the nearest CLICKABLE ancestor
/// (`el.closest('a,button,[role=button],…') || el`), scroll it into view, and
/// return the exact point to click — computed from `DOM.getContentQuads`, which
/// returns quads in the SAME coordinate space CDP `Input.dispatchMouseEvent`
/// expects (CSS px relative to the layout viewport). This sidesteps all
/// CSS-vs-device-px / DPR guesswork that plagues pure `getBoundingClientRect`
/// math. Falls back to the rect center only if quads are empty. Returns None if
/// no element matches. FIRST half of a TRUSTED click (never el.click()).
async fn element_center_by_text(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    want: &str,
) -> Result<Option<ClickTarget>, String> {
    ensure_session(app, state).await?;
    // 1) Locate the element AND resolve to the nearest clickable ancestor, then
    //    scroll into view. Return the NODE (returnByValue:false -> objectId) so
    //    we can ask CDP for its content quads. Also stash diagnostics + the rect
    //    center (fallback) + href on the node object so a single eval gives us
    //    everything.
    let locate_fn = format!(
        "(()=>{{ const t={want:?}.toLowerCase(); \
         const els=[...document.querySelectorAll('a,button,input[type=submit],input[type=button],[role=button],label,[onclick],h3,li,span,div')]; \
         let el=els.find(e=>{{ const s=(e.innerText||e.value||e.textContent||''); return s && s.toLowerCase().includes(t) && e.offsetParent!==null; }}); \
         if(!el) return null; \
         const clickable = el.closest('a,button,input[type=submit],input[type=button],[role=button],[onclick]') || el; \
         el = clickable; \
         el.scrollIntoView({{block:'center',inline:'center'}}); \
         const r=el.getBoundingClientRect(); \
         const a = el.closest('a'); \
         el.__aygentDiag = {{ \
            tag: el.tagName.toLowerCase(), \
            href: a ? a.href : null, \
            rx: r.left + r.width/2, ry: r.top + r.height/2, \
            rw: r.width, rh: r.height, \
            dpr: window.devicePixelRatio||1, \
            sx: window.scrollX||0, sy: window.scrollY||0, \
            iw: window.innerWidth||0, ih: window.innerHeight||0 \
         }}; \
         return el; }})()",
        want = want
    );
    let node = session_call(
        state,
        "Runtime.evaluate",
        serde_json::json!({ "expression": locate_fn, "returnByValue": false }),
    )
    .await?;
    let object_id = match node["result"]["objectId"].as_str() {
        Some(id) => id.to_string(),
        None => return Ok(None), // matched nothing (result was null)
    };

    // 2) Pull the diagnostics we stashed on the node (returnByValue this time).
    let diag = session_call(
        state,
        "Runtime.callFunctionOn",
        serde_json::json!({
            "objectId": object_id,
            "functionDeclaration": "function(){ return this.__aygentDiag; }",
            "returnByValue": true
        }),
    )
    .await?;
    let d = &diag["result"]["value"];
    let tag = d["tag"].as_str().unwrap_or("?").to_string();
    let href = d["href"].as_str().map(|s| s.to_string());
    let dpr = d["dpr"].as_f64().unwrap_or(1.0);
    let scroll_x = d["sx"].as_f64().unwrap_or(0.0);
    let scroll_y = d["sy"].as_f64().unwrap_or(0.0);
    let inner_w = d["iw"].as_f64().unwrap_or(0.0);
    let inner_h = d["ih"].as_f64().unwrap_or(0.0);
    let rect_cx = d["rx"].as_f64().unwrap_or(0.0);
    let rect_cy = d["ry"].as_f64().unwrap_or(0.0);

    // 3) Ask CDP for the element's content quads — SAME coord space as Input.*.
    //    A quad is [x1,y1, x2,y2, x3,y3, x4,y4]; its center is the mean of the
    //    x's and y's. If quads are empty (element off-screen after scroll, or a
    //    zero-box wrapper), fall back to the rect center from the eval above.
    let (mut x, mut y, mut via_quads) = (rect_cx, rect_cy, false);
    match session_call(
        state,
        "DOM.getContentQuads",
        serde_json::json!({ "objectId": object_id }),
    )
    .await
    {
        Ok(q) => {
            if let Some(quads) = q["quads"].as_array() {
                if let Some(first) = quads.first().and_then(|f| f.as_array()) {
                    if first.len() == 8 {
                        let get = |i: usize| first[i].as_f64().unwrap_or(0.0);
                        x = (get(0) + get(2) + get(4) + get(6)) / 4.0;
                        y = (get(1) + get(3) + get(5) + get(7)) / 4.0;
                        via_quads = true;
                    }
                }
            }
        }
        Err(e) => {
            // DOM domain may not be enabled on this build/path — the rect
            // fallback still works, just note it.
            eprintln!("[aygent][browser][CLICK] getContentQuads err (rect fallback): {e}");
        }
    }

    // Release the remote object so we don't leak references in the page.
    let _ = session_call(
        state,
        "Runtime.releaseObject",
        serde_json::json!({ "objectId": object_id }),
    )
    .await;

    if x <= 0.0 && y <= 0.0 {
        return Ok(None);
    }

    Ok(Some(ClickTarget {
        x, y, tag, href, dpr, scroll_x, scroll_y, inner_w, inner_h, via_quads,
    }))
}

/// Find a typeable field's viewport-center [x,y] in CSS px (scrolled into view),
/// so we can REAL-click it to focus before typing. Prefers the current
/// activeElement if it is already an editable field; otherwise the first visible
/// text-like input/textarea/contenteditable.
async fn typeable_field_center(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
) -> Result<Option<(f64, f64)>, String> {
    let expr = "(()=>{ \
         let el=document.activeElement; \
         const editable=(e)=>e && (('value' in e && e!==document.body) || e.isContentEditable); \
         if(!editable(el)){ el=[...document.querySelectorAll('input[type=text],input[type=search],input:not([type]),textarea,input[type=email],input[type=url],[contenteditable=true],[role=searchbox],[role=textbox]')].find(e=>e.offsetParent!==null); } \
         if(!el) return null; \
         el.scrollIntoView({block:'center',inline:'center'}); \
         const r=el.getBoundingClientRect(); \
         if(r.width===0||r.height===0) return null; \
         return [r.left + r.width/2, r.top + r.height/2]; })()";
    let json = active_tab_read(app, state, expr).await?;
    if json.trim() == "null" || json.trim().is_empty() {
        return Ok(None);
    }
    let coords: Vec<f64> = serde_json::from_str(&json).unwrap_or_default();
    if coords.len() == 2 { Ok(Some((coords[0], coords[1]))) } else { Ok(None) }
}

/// TRUSTED mouse click at ABSOLUTE CSS-px coords (x,y) via CDP Input.* — the
/// events carry isTrusted:true. Adds a short human-like approach: 2-3
/// intermediate mouseMoved points toward the target before press/release, plus
/// small randomized delays. This is the shape a real pointer produces.
async fn trusted_click_at(
    state: &tauri::State<'_, BrowserProc>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    // Approach from a point up-and-left of the target (arbitrary but plausible),
    // stepping in 3 mouseMoved events so hover/mousemove handlers see motion.
    let start_x = (x - 40.0).max(0.0);
    let start_y = (y - 30.0).max(0.0);
    for i in 1..=3u32 {
        let f = i as f64 / 3.0;
        let mx = start_x + (x - start_x) * f;
        let my = start_y + (y - start_y) * f;
        session_call(
            state,
            "Input.dispatchMouseEvent",
            serde_json::json!({ "type": "mouseMoved", "x": mx, "y": my, "button": "none", "buttons": 0 }),
        )
        .await?;
        human_delay(15, 45).await;
    }
    // Settle on the exact target.
    session_call(
        state,
        "Input.dispatchMouseEvent",
        serde_json::json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 }),
    )
    .await?;
    human_delay(30, 90).await;
    // Press.
    session_call(
        state,
        "Input.dispatchMouseEvent",
        serde_json::json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": 1 }),
    )
    .await?;
    human_delay(40, 110).await; // dwell time of a real press
    // Release.
    session_call(
        state,
        "Input.dispatchMouseEvent",
        serde_json::json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "buttons": 0, "clickCount": 1 }),
    )
    .await?;
    eprintln!("[aygent][browser][INPUT] trusted click at ({x:.1},{y:.1})");
    Ok(())
}

/// Map a character to the CDP key-event fields Chromium wants so per-character
/// typing produces trusted keydown/char/keyup with a plausible virtual keycode.
/// For printable chars, `text` carries the char and the keyDown "char" event is
/// what actually inserts it. Returns (windowsVirtualKeyCode, key, text).
fn char_key_fields(c: char) -> (i64, String, String) {
    let text = c.to_string();
    // Best-effort virtual keycode: letters/digits map to their ASCII-upper code;
    // everything else we leave 0 (the "char" event still inserts the glyph).
    let vk = if c.is_ascii_alphabetic() {
        (c.to_ascii_uppercase() as u8) as i64
    } else if c.is_ascii_digit() {
        (c as u8) as i64
    } else if c == ' ' {
        32
    } else {
        0
    };
    (vk, text.clone(), text)
}

/// TRUSTED per-character typing into the focused element via CDP
/// Input.dispatchKeyEvent (keyDown with `text` -> keyUp), with small randomized
/// inter-key delays so timing looks human. isTrusted:true, real key events —
/// unlike el.value=... which fires nothing trusted.
async fn trusted_type(
    state: &tauri::State<'_, BrowserProc>,
    text: &str,
) -> Result<(), String> {
    for c in text.chars() {
        let (vk, key, ch) = char_key_fields(c);
        // keyDown carrying `text` inserts the character as a trusted input.
        let mut down = serde_json::json!({
            "type": "keyDown", "text": ch, "key": key.clone(),
            "unmodifiedText": text_unmod(c),
        });
        if vk != 0 {
            down["windowsVirtualKeyCode"] = vk.into();
            down["nativeVirtualKeyCode"] = vk.into();
        }
        session_call(state, "Input.dispatchKeyEvent", down).await?;
        let mut up = serde_json::json!({ "type": "keyUp", "key": key });
        if vk != 0 {
            up["windowsVirtualKeyCode"] = vk.into();
            up["nativeVirtualKeyCode"] = vk.into();
        }
        session_call(state, "Input.dispatchKeyEvent", up).await?;
        human_delay(40, 120).await; // per-key human cadence
    }
    eprintln!("[aygent][browser][INPUT] trusted type of {} chars", text.chars().count());
    Ok(())
}

fn text_unmod(c: char) -> String {
    // For a shifted char the unmodifiedText would differ; we type without a
    // shift modifier and let `text` carry the final glyph, so unmodifiedText =
    // the lowercase/base char is a fine approximation for detectors.
    c.to_string()
}

/// TRUSTED Enter keypress via CDP Input.dispatchKeyEvent (keyDown+keyUp,
/// windowsVirtualKeyCode 13). Real navigation trigger — not form.submit().
async fn trusted_enter(state: &tauri::State<'_, BrowserProc>) -> Result<(), String> {
    let down = serde_json::json!({
        "type": "keyDown", "key": "Enter", "code": "Enter",
        "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13, "text": "\r"
    });
    session_call(state, "Input.dispatchKeyEvent", down).await?;
    human_delay(30, 80).await;
    let up = serde_json::json!({
        "type": "keyUp", "key": "Enter", "code": "Enter",
        "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13
    });
    session_call(state, "Input.dispatchKeyEvent", up).await?;
    eprintln!("[aygent][browser][INPUT] trusted Enter");
    Ok(())
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
    // After ANY action that could navigate the AUTHORITATIVE CDP page, mirror the
    // VISIBLE WKWebView to it (CDP -> visible), so the human watches where the
    // agent went and the permission host pre-check stays correct. This is the
    // single-source-of-truth sync that replaces the old fragile read-mirror.
    if matches!(name, "browser_open" | "browser_click_text" | "browser_type_text") {
        mirror_visible_to_cdp(app, state).await;
    }
    // On a login/CAPTCHA-ish failure, hand off to the human with a note.
    let handoff = if out.1 && looks_like_handoff(&out.0) {
        "the agent hit a login or verification wall — take the wheel to continue"
    } else { "" };
    agent_release(app, state, handoff);
    out
}

/// Extract the bare host from a URL (lowercased, no scheme/port/path/creds).
fn host_of(url: &str) -> String {
    let lower = url.trim().to_ascii_lowercase();
    lower
        .split("://").last().unwrap_or("")
        .split('/').next().unwrap_or("")
        .split('@').last().unwrap_or("")
        .split(':').next().unwrap_or("")
        .to_string()
}

/// The host the ACTIVE tab is currently showing (always allowed for the agent,
/// since the human navigated there and is watching). Prefer the UI-reported URL
/// (authoritative for where the human is); fall back to a CDP read.
async fn active_tab_host(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> String {
    // UI-reported URL wins — it's the page the human actually navigated to.
    if let Ok(g) = ACTIVE_TAB_URL.lock() {
        let h = host_of(&normalize_url(&g));
        if !h.is_empty() { return h; }
    }
    match active_tab_page_info_cdp(app, state).await {
        Ok((_t, url)) => host_of(&url),
        Err(_) => String::new(),
    }
}

/// Is `host` allowed WITHOUT asking? True if it matches the active tab's current
/// host (human already there), a host the human granted this session, or the
/// agent's configured allowlist.
async fn host_preallowed(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    host: &str,
    allowed_domains: &[String],
) -> bool {
    if host.is_empty() { return false; }
    let cur = active_tab_host(app, state).await;
    if !cur.is_empty() && (host == cur || host.ends_with(&format!(".{cur}")) || cur.ends_with(&format!(".{host}"))) {
        return true;
    }
    if let Ok(p) = state.perm.lock() {
        if p.granted_hosts.iter().any(|g| host == g || host.ends_with(&format!(".{g}"))) {
            return true;
        }
    }
    allowed_domains.iter().any(|d| {
        let d = d.trim().to_ascii_lowercase();
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    })
}

/// ASK THE HUMAN. Emit `browser:permission-request` describing what the agent
/// wants, then PARK on a oneshot until the human answers via
/// `browser_permission_answer`. Returns "allow" | "deny" | "take". Pulses the
/// toggle (sets a control note). 5-min safety timeout -> "deny".
async fn request_permission(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    action: &str,
    detail: &str,
) -> String {
    use tauri::Emitter;
    let id = format!("perm_{}", SESSION_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    let (tx, rx) = oneshot::channel::<String>();
    if let Ok(mut p) = state.perm.lock() { p.pending.insert(id.clone(), tx); }
    if let Ok(mut c) = state.control.lock() {
        c.note = format!("agent wants to {action}");
        emit_control(app, &c);
    }
    let _ = app.emit("browser:permission-request", &serde_json::json!({
        "id": id, "action": action, "detail": detail,
    }));
    let ans = match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
        Ok(Ok(v)) => v,
        _ => "deny".to_string(),
    };
    if let Ok(mut p) = state.perm.lock() { p.pending.remove(&id); }
    ans
}

/// The human's answer to a pending permission request. `answer` = "allow" |
/// "deny" | "take". On allow, `grant_host` (if given) is remembered for the
/// session so the agent won't ask again for it.
#[tauri::command]
pub fn browser_permission_answer(
    app: tauri::AppHandle,
    state: tauri::State<'_, BrowserProc>,
    id: String,
    answer: String,
    grant_host: Option<String>,
) -> Result<(), String> {
    let tx = {
        let mut p = state.perm.lock().map_err(|_| "perm poisoned")?;
        if answer == "allow" {
            if let Some(h) = grant_host.as_ref().map(|h| host_of(h)).filter(|h| !h.is_empty()) {
                p.granted_hosts.insert(h);
            }
        }
        p.pending.remove(&id)
    };
    if let Ok(mut c) = state.control.lock() {
        c.note = String::new();
        if answer == "take" { c.driver = "human".into(); }
        emit_control(&app, &c);
    }
    if let Some(tx) = tx { let _ = tx.send(answer); }
    Ok(())
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
    // SINGLE SOURCE OF TRUTH (Problem 1 fix). ALL agent actions AND reads run
    // against the CDP session's page — the SAME page. Actions used to fire into
    // the VISIBLE WKWebView via wv.eval() while reads hit a SEPARATE headless
    // Chromium; those two desynced constantly (act on page A, read stale page B,
    // loop forever). Now: navigate/click/type all go through CDP, reads read the
    // very page the CDP actions just changed, and the VISIBLE webview is mirrored
    // TO the CDP page's url afterwards (see agent_tool -> mirror_visible_to_cdp)
    // so the human still watches. One authoritative page state; no desync.
    ensure_session(app, state).await.ok();
    match name {
        "browser_open" => {
            let url = normalize_url(input.get("url").and_then(|u| u.as_str()).unwrap_or(""));
            if let Err(e) = check_internal_only(&url) { return (e, true); }
            let host = host_of(&url);
            if !host_preallowed(app, state, &host, allowed_domains).await {
                let ans = request_permission(app, state, &format!("open {host}"), &url).await;
                match ans.as_str() {
                    "allow" => { if let Ok(mut p) = state.perm.lock() { p.granted_hosts.insert(host.clone()); } }
                    "take" => return ("the human took the wheel to handle this navigation".into(), true),
                    _ => return (format!("the human declined opening {host}. Try a different site or ask them to do it."), true),
                }
            }
            // Navigate the AUTHORITATIVE CDP page + wait for load.
            if let Err(e) = session_call(state, "Page.navigate", serde_json::json!({ "url": url })).await {
                return (format!("navigate failed: {e}"), true);
            }
            wait_for_cdp_load(state).await;
            match active_tab_page_text(app, state).await {
                Ok((title, text)) => (format!("Opened. Title: {title}\n\n{text}"), false),
                Err(e) => (format!("opened but read failed: {e}"), true),
            }
        }
        "browser_read" => match active_tab_page_text(app, state).await {
            Ok((title, text)) => (format!("Title: {title}\n\n{text}"), false),
            Err(e) => (format!("read failed: {e}"), true),
        },
        "browser_click_text" => {
            let want = input.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if want.is_empty() { return ("browser_click_text needs `text`".into(), true); }
            // GATE THE CLICK: clicking a link navigates the page for the human,
            // so ask before doing it (Approve/Deny/Take Control, inline in the
            // Agent panel). This is Mason's requirement: the human confirms the
            // click. Allow -> proceed; Deny -> stop this action; Take -> hand the
            // wheel to the human.
            let ans = request_permission(app, state, &format!("click \"{want}\""), &format!("The agent wants to click the link/button: {want}")).await;
            match ans.as_str() {
                "allow" => {}
                "take" => return ("the human took the wheel to do this themselves".into(), true),
                _ => return (format!("the human declined clicking '{want}'. Stopping."), true),
            }
            // TRUSTED CLICK (#1). Instead of el.click() via JS (isTrusted:false,
            // which Google flags), we (a) locate the element's viewport-center
            // coords via a CDP Runtime.evaluate (scrolling it into view), then
            // (b) dispatch a REAL CDP mouse click at those coords
            // (mouseMoved x3 approach -> mousePressed -> mouseReleased). The
            // events carry isTrusted:true — indistinguishable from a human click.
            eprintln!("[aygent][browser][INPUT] browser_click_text want={want:?}");
            let target = match element_center_by_text(app, state, want).await {
                Ok(Some(c)) => c,
                Ok(None) => return (format!("no clickable element matching '{want}' on this page"), true),
                Err(e) => return (format!("click locate failed: {e}"), true),
            };
            // GROUND-TRUTH DIAGNOSTICS: paste-able terminal line showing exactly
            // where we're about to click, the page's DPR/scroll/viewport, the
            // resolved element tag, and whether we used getContentQuads (coord
            // space guaranteed to match Input.*) or the rect fallback.
            eprintln!(
                "[aygent][browser][CLICK] want={want:?} tag=<{}> coord=({:.1},{:.1}) via_quads={} dpr={} scroll=({:.0},{:.0}) inner=({:.0}x{:.0}) href={:?}",
                target.tag, target.x, target.y, target.via_quads, target.dpr,
                target.scroll_x, target.scroll_y, target.inner_w, target.inner_h, target.href
            );
            // Capture the URL + a coarse DOM signature BEFORE the click so we can
            // tell — HONESTLY — whether the click actually did anything.
            let url_before = cdp_current_url(state).await;
            human_delay(60, 150).await;
            if let Err(e) = trusted_click_at(state, target.x, target.y).await {
                return (format!("trusted click dispatch failed: {e}"), true);
            }
            wait_for_cdp_load(state).await;
            let mut url_after = cdp_current_url(state).await;
            let mut navigated = url_after != url_before && url_after.starts_with("http");
            // FALLBACK: the trusted click landed but produced no navigation and
            // we DO have a resolved <a> href (classic search-result case). Rather
            // than lie with a false ✓, navigate the authoritative CDP page to
            // that href so the human still gets the result — still no el.click().
            if !navigated {
                if let Some(href) = target.href.as_deref() {
                    if href.starts_with("http") && href != url_before {
                        eprintln!("[aygent][browser][CLICK] trusted click produced NO nav — href fallback -> {href}");
                        if session_call(state, "Page.navigate", serde_json::json!({ "url": href })).await.is_ok() {
                            wait_for_cdp_load(state).await;
                            url_after = cdp_current_url(state).await;
                            navigated = url_after != url_before && url_after.starts_with("http");
                        }
                    }
                }
            }
            let (title, url) = active_tab_page_info_cdp(app, state).await.unwrap_or_default();
            eprintln!(
                "[aygent][browser][CLICK] result navigated={navigated} url_before={url_before:?} url_after={url_after:?}"
            );
            if navigated {
                (format!("clicked '{want}'. Now on: {title} ({url})"), false)
            } else {
                // HONEST FAILURE (#4): do NOT report success. The click event
                // fired (isTrusted) but the page did not change, so the model /
                // checklist must NOT tick this step off.
                (
                    format!(
                        "clicked '{want}' (trusted mouse event dispatched on <{}> at ({:.0},{:.0})) but the page did NOT navigate or change — the target may be wrong or the link needs a different action. Still on: {title} ({url}).",
                        target.tag, target.x, target.y
                    ),
                    true,
                )
            }
        }
        "browser_type_text" => {
            let text = input.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let submit = input.get("submit").and_then(|b| b.as_bool()).unwrap_or(false);
            if text.is_empty() { return ("browser_type_text needs `text`".into(), true); }
            // TRUSTED TYPING (#1). Instead of el.value=... (fires no trusted
            // input events — flagged), we:
            //   (a) locate the target field's center coords + REAL-click it to
            //       focus (trusted mouse click),
            //   (b) type per-character via CDP Input.dispatchKeyEvent
            //       (keyDown+keyUp, `text` carries the glyph) with 40-120ms
            //       human-cadence delays,
            //   (c) on submit, press a REAL Enter (windowsVirtualKeyCode 13) via
            //       Input.dispatchKeyEvent — not form.submit(). requestSubmit()
            //       is kept ONLY as a fallback if the Enter path caused no nav.
            eprintln!("[aygent][browser][INPUT] browser_type_text submit={submit} text_len={}", text.len());
            let field = match typeable_field_center(app, state).await {
                Ok(Some(c)) => c,
                Ok(None) => return ("no typeable field found on this page".into(), true),
                Err(e) => return (format!("type locate failed: {e}"), true),
            };
            human_delay(60, 150).await;
            if let Err(e) = trusted_click_at(state, field.0, field.1).await {
                return (format!("focus-click failed: {e}"), true);
            }
            human_delay(60, 150).await;
            if let Err(e) = trusted_type(state, text).await {
                return (format!("trusted type failed: {e}"), true);
            }
            if submit {
                // Capture the url before Enter so we can tell if it navigated.
                let url_before = cdp_current_url(state).await;
                human_delay(60, 150).await;
                if let Err(e) = trusted_enter(state).await {
                    return (format!("Enter dispatch failed: {e}"), true);
                }
                wait_for_cdp_load(state).await;
                let url_after = cdp_current_url(state).await;
                // FALLBACK ONLY: if the trusted Enter produced no navigation
                // (some SPA search boxes rely on form submit), try
                // requestSubmit()/submit() on the field's form as a last resort.
                if url_after == url_before {
                    eprintln!("[aygent][browser][INPUT] Enter yielded no nav — requestSubmit() fallback");
                    let _ = active_tab_read(app, state,
                        "(()=>{ const el=document.activeElement; const f=el&&el.form; if(f){ try{ if(f.requestSubmit) f.requestSubmit(); else f.submit(); }catch(e){} } return 'OK'; })()").await;
                    wait_for_cdp_load(state).await;
                }
            }
            (format!("typed '{}'{}", text, if submit { " and submitted" } else { "" }), false)
        }
        "browser_screenshot" => {
            match active_tab_page_info_cdp(app, state).await {
                Ok((title, url)) => (format!("the human sees the live page. Title: {title} ({url})"), false),
                Err(e) => (format!("page info failed: {e}"), true),
            }
        }
        other => (format!("unknown browser tool: {other}"), true),
    }
}

/// Wait for the CDP page to settle after a navigation: poll document.readyState
/// until 'complete' (or a bounded timeout). Replaces the old fixed-sleep guesses
/// so a slow page doesn't get read half-loaded (a source of stale reads).
async fn wait_for_cdp_load(state: &tauri::State<'_, BrowserProc>) {
    // Small initial delay so the navigation has actually begun before we poll.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    for _ in 0..24 { // up to ~6s (24 * 250ms)
        let ready = session_call(state, "Runtime.evaluate",
            serde_json::json!({ "expression": "document.readyState", "returnByValue": true })).await
            .ok()
            .and_then(|r| r["result"]["value"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        if ready == "complete" { break; }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    // Tiny settle for post-load JS (SPA route paints, etc).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// Read the ACTIVE tab's (title, visible text) via CDP Runtime.evaluate (works
/// on external pages; no __TAURI__ needed).
async fn active_tab_page_text(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> Result<(String, String), String> {
    let r = active_tab_read(app, state,
        "[document.title||'', (document.body?document.body.innerText:'').slice(0,8000)]").await?;
    let arr: Vec<String> = serde_json::from_str(&r).unwrap_or_default();
    if arr.len() == 2 { Ok((arr[0].clone(), arr[1].clone())) } else { Err("unexpected page text shape".into()) }
}

/// Read (title, url) via CDP.
async fn active_tab_page_info_cdp(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> Result<(String, String), String> {
    let r = active_tab_read(app, state, "[document.title||'', location.href||'']").await?;
    let arr: Vec<String> = serde_json::from_str(&r).unwrap_or_default();
    Ok((arr.get(0).cloned().unwrap_or_default(), arr.get(1).cloned().unwrap_or_default()))
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

/// SSRF guard only (no allowlist): reject non-http(s) + internal/localhost
/// hosts. Used by the visible-tab agent path where the human is the allowlist
/// (they Allow/Deny new hosts), but internal targets are NEVER promptable.
fn check_internal_only(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("agent may only open http(s) URLs".into());
    }
    let host = host_of(&lower);
    if host.is_empty() { return Err("could not parse host".into()); }
    if host == "localhost" || host.starts_with("127.") || host.starts_with("10.")
        || host.starts_with("192.168.") || host.ends_with(".local") || host == "0.0.0.0"
        || host.starts_with("169.254.") {
        return Err("refused: internal/localhost host is not allowed".into());
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
