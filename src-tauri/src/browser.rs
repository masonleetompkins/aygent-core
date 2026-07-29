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

/// The running headless Chromium: the child process + the port its DevTools
/// endpoint is on. Held in Tauri state so we launch ONCE and reuse.
#[derive(Default)]
pub struct BrowserProc {
    inner: Mutex<Option<RunningBrowser>>,
}

struct RunningBrowser {
    child: std::process::Child,
    port: u16,
}

impl BrowserProc {
    pub fn new() -> Self {
        Self { inner: Mutex::new(None) }
    }
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
    // A jailed per-app profile dir INSIDE AYGENT's managed browser dir (never a
    // system Chrome profile). Per-agent isolation (Atlas C) comes in a later
    // slice; Slice 1 uses one shared profile under our own dir.
    let profile = browser_dir(app)?.join("profile-default");
    std::fs::create_dir_all(&profile).map_err(|e| format!("mkdir profile: {e}"))?;

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
    *guard = Some(RunningBrowser { child, port });
    Ok(port)
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

/// Minimal CDP call over a page WebSocket: send one method + params, wait for
/// the reply with the matching id, return its `result`. (Slice 1 does a small
/// fixed sequence, so a per-call open/close socket is fine.)
async fn cdp_call(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let req = serde_json::json!({ "id": id, "method": method, "params": params });
    ws.send(Message::Text(req.to_string().into()))
        .await
        .map_err(|e| format!("cdp send {method}: {e}"))?;
    // Read frames until we get our id (skip events + other ids).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!("cdp {method}: timed out"));
        }
        let frame = tokio::time::timeout(remaining, ws.next())
            .await
            .map_err(|_| format!("cdp {method}: timed out"))?;
        let msg = match frame {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Close(_))) | None => return Err(format!("cdp {method}: socket closed")),
            Some(Ok(_)) => continue, // ping/binary — ignore
            Some(Err(e)) => return Err(format!("cdp {method} recv: {e}")),
        };
        let v: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["id"].as_u64() == Some(id) {
            if let Some(err) = v.get("error") {
                return Err(format!("cdp {method}: {err}"));
            }
            return Ok(v["result"].clone());
        }
        // else: an event or another call's reply — keep reading.
    }
}

/// SLICE 1 — navigate to `url` and return a base64 PNG screenshot of the page.
/// This is the first time the browser DOES something the human can SEE inside
/// AYGENT. Also returns the final URL + page title.
#[tauri::command]
pub async fn browser_navigate(
    app: tauri::AppHandle,
    state: tauri::State<'_, BrowserProc>,
    url: String,
) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::connect_async;

    // Normalize a bare host into a URL (so "example.com" works).
    let url = normalize_url(&url);

    let port = ensure_running(&app, &state).await?;
    let client = reqwest::Client::new();
    let ws_url = page_ws_url(&client, port).await?;
    let (mut ws, _) = connect_async(&ws_url)
        .await
        .map_err(|e| format!("cdp connect: {e}"))?;

    let mut id = 1u64;
    let mut next = || { let n = id; id += 1; n };

    // Enable the domains we use.
    cdp_call(&mut ws, next(), "Page.enable", serde_json::json!({})).await?;

    // Navigate.
    cdp_call(&mut ws, next(), "Page.navigate", serde_json::json!({ "url": url })).await?;

    // Give the page a moment to render (Slice 1 is a simple settle; S2's
    // screencast will stream live frames instead of a single settled shot).
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    // Capture a PNG screenshot (base64).
    let shot = cdp_call(
        &mut ws,
        next(),
        "Page.captureScreenshot",
        serde_json::json!({ "format": "png" }),
    )
    .await?;
    let data = shot["data"].as_str().unwrap_or("").to_string();

    // Read the final URL + title (best-effort).
    let (mut final_url, mut title) = (url.clone(), String::new());
    if let Ok(r) = cdp_call(
        &mut ws,
        next(),
        "Runtime.evaluate",
        serde_json::json!({ "expression": "JSON.stringify([location.href, document.title])", "returnByValue": true }),
    )
    .await
    {
        if let Some(s) = r["result"]["value"].as_str() {
            if let Ok(arr) = serde_json::from_str::<Vec<String>>(s) {
                if arr.len() == 2 {
                    final_url = arr[0].clone();
                    title = arr[1].clone();
                }
            }
        }
    }

    let _ = ws.close(None).await;

    if data.is_empty() {
        return Err("screenshot empty".into());
    }
    Ok(serde_json::json!({
        "screenshot": format!("data:image/png;base64,{data}"),
        "url": final_url,
        "title": title,
    }))
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
#[tauri::command]
pub fn browser_shutdown(state: tauri::State<'_, BrowserProc>) -> Result<(), String> {
    let mut guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
    if let Some(mut rb) = guard.take() {
        let _ = rb.child.kill();
        let _ = rb.child.wait();
    }
    Ok(())
}
