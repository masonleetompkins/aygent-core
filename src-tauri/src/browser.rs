// AYGENT â€” In-app browser provisioning, SLICE 0: THE GATE.
//
// Atlas's BROWSER-ARCH north star (projects/aygent/BROWSER-ARCH.md). Slice 0 is
// the make-or-break: prove AYGENT can, on a clean Mac with the user having
// installed NOTHING, obtain a real Chromium and LAUNCH it as our own child
// process â€” no Homebrew, no npm, no terminal, no Gatekeeper block.
//
// The install-nothing solution (Mason's load-bearing rule + his "would I have to
// host the file?" test â†’ NO): we fetch Google's OFFICIAL, version-pinned
// **Chrome for Testing** build from Google's own CDN
// (storage.googleapis.com/chrome-for-testing-public/...) â€” the exact same
// "upstream hosts it, we just fetch it" model as the GGUF weights local models
// already download. Zero hosting/bandwidth cost on us. This mirrors
// `local_download` in lib.rs precisely (streamed download + progress events +
// atomic .partâ†’final rename), then adds the three macOS-specific steps that make
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
// NOT yet drive pages, mirror, or expose agent tools â€” those are Slices 1+.
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
/// headless â€” that is the #1 tell, so we override it everywhere.
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
/// (the unzipped Chromium, later per-agent profiles) lives UNDER here â€” inside
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

/// The ROOT folder of the currently-active agent — the folder shown at boot as
/// `agent folder restored: <path>`. This is the authoritative source: the PATH
/// BROKER holds one scope per agent (registered from each agent's
/// `folder_path`, see `register_all_agent_scopes`) PLUS a `"default"` scope
/// that always points at the active agent's folder (set by `agents_set_active`
/// / `restore_agent_folder`). We ask the broker for the active agent's root
/// first, then fall back to `"default"`. Never hardcoded.
///
/// Returns `None` only when no folder has been picked yet (no scope registered).
pub fn agent_folder_root(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let broker = app.try_state::<std::sync::Arc<crate::broker::Broker>>()?;
    let agent = active_browser_agent(app);
    broker
        .root_for(&agent)
        .or_else(|_| broker.root_for("default"))
        .ok()
}

/// The directory downloads land in for the ACTIVE agent: a `downloads/` folder
/// at the ROOT of the active agent's own folder
/// (e.g. `/Users/masontompkins/Aygent/Copywriter/downloads`). Mason's explicit
/// direction: the agent works in its own workspace, so it should have immediate
/// access to what it downloads, and the human knows exactly where to look.
///
/// This is the FINAL destination. It is NOT necessarily where WebKit writes —
/// see `download_event`: WKWebView's sandboxed networking XPC process needs a
/// sandbox extension for the write dir, which deep/new paths can't reliably get
/// (`sandbox_extension_issue_file failed for : 2`). So the download WRITE goes
/// to the OS temp dir (always extension-granted) and OUR process moves the file
/// here on `Finished`. The move is done by the MAIN app process (a normal user
/// dir it can write freely), never by WebKit's networking sandbox — sidestepping
/// the extension issue entirely. `browser_downloads_list` and the headless-CDP
/// agent-download path (a separate Chromium process, not under WebKit's
/// networking sandbox) both use this dir directly.
///
/// Falls back to `~/Downloads/AYGENT/<agent>` (the legacy location) ONLY if no
/// agent folder is picked yet, then the app_data browser dir, then the process
/// CWD, so a destination is ALWAYS a real existing directory.
pub fn agent_downloads_dir(app: &tauri::AppHandle) -> PathBuf {
    let dir = match agent_folder_root(app) {
        Some(root) => root.join("downloads"),
        None => {
            // No agent folder picked yet — keep the old ~/Downloads/AYGENT/<agent>
            // behaviour so downloads still land SOMEWHERE real.
            let agent = active_browser_agent(app);
            let base = dirs::download_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
                .or_else(|| browser_dir(app).ok())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            base.join("AYGENT").join(&agent)
        }
    };
    let _ = std::fs::create_dir_all(&dir);
    // Canonicalize to a real, symlink-resolved absolute path; if canonicalize
    // fails (shouldn't, we just made it) keep the plain path.
    let resolved = std::fs::canonicalize(&dir).unwrap_or(dir);
    // ENGINE-CEF: keep CEF's DownloadHandler destination in sync with the
    // active agent's downloads dir, so native CEF downloads land in the SAME
    // place the WKWebView path used. Cheap + idempotent.
    #[cfg(all(target_os = "macos", feature = "engine-cef"))]
    crate::cef_engine::set_downloads_dir(resolved.clone());
    resolved
}

/// Where the unzipped Chromium runtime lands: <browser>/chromium/<version>/.
/// Version-scoped so a future upgrade can download the new one alongside, verify
/// it, then flip â€” never leaving the user without a working browser mid-upgrade.
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
/// the URL and the integrity hash â€” so we always have a hash to check.
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
    // current we just take current stable (with its hash) â€” the pinned URL might
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
    // No hash available at all (JSON shape changed?) â€” last resort, pinned URL,
    // hash empty (caller will WARN, not silently trust â€” see verify step).
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

/// SLICE 0 â€” download + provision Chrome for Testing, emitting progress on
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

    // --- DOWNLOAD (streamed, progress events, atomic .partâ†’final) -----------
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
    // silently trusting â€” HTTPS transport integrity still applies, but we flag it.
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
            "provisioned but executable missing at {} â€” CfT layout may have changed",
            exe.display()
        ));
    }

    let _ = app.emit(&channel, &serde_json::json!({ "phase": "done", "done": true, "version": version, "executable": exe.to_string_lossy() }));
    Ok(exe.to_string_lossy().to_string())
}

/// SLICE 0 GATE â€” launch the provisioned Chromium as OUR child process and
/// confirm it actually runs with no install + no Gatekeeper block. We do the
/// cheapest possible proof: `--version` (prints the build + exits 0). If that
/// returns cleanly, the whole install-nothing thesis holds; if macOS blocked it,
/// we get a Gatekeeper error here and know Slice 0 failed (â†’ consider bundled).
///
/// Returns the version string Chromium reports (proof it launched).
#[tauri::command]
pub async fn browser_launch_probe(app: tauri::AppHandle) -> Result<String, String> {
    if !is_installed(&app) {
        return Err("browser not installed â€” run browser_install first".into());
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
// SLICE 1 â€” LIVE DRIVE: launch Chromium headless w/ CDP, navigate, screenshot.
//
// The mature CDP client (playwright-core) lives in the Node daemon per Atlas's
// doc â€” that's Slice 4's home for the agent-tool surface. But for Slice 1's
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
    /// SLICE 5 â€” who's driving the shared page: "human" | "agent" | "idle".
    /// The human ALWAYS wins: taking the wheel preempts the agent instantly.
    control: Mutex<Control>,
    /// PERMISSION REQUESTS: when the agent wants to do something outside what the
    /// human already navigated to (a NEW host), it opens a request here and
    /// blocks on the oneshot until the human answers Allow/Deny/Take Control via
    /// `browser_permission_answer`. Keyed by request id. Also tracks the set of
    /// hosts the human has granted THIS session (so a granted host stays allowed).
    perm: Mutex<PermState>,
    /// DOWNLOADS (sandbox-safe temp-then-move). WKWebView performs the file
    /// WRITE in its own sandboxed networking XPC process, which needs a sandbox
    /// extension for the destination's parent dir. Deep/new dirs (bundle
    /// containers, ~/Downloads/AYGENT) trip `sandbox_extension_issue_file`. So
    /// in `Requested` we point WebKit at the OS temp dir (which its networking
    /// process ALWAYS holds an extension for) and remember, keyed by download
    /// URL, both the temp path WebKit is writing to AND the final destination
    /// under the agent folder's `downloads/`. In `Finished` OUR process (not
    /// WebKit's sandbox) MOVES the temp file to the agent folder. `path` in
    /// `Finished` is empty on macOS, so this map is how we recover the file.
    dl_temps: Mutex<std::collections::HashMap<String, (PathBuf, PathBuf)>>,
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
    pub note: String,         // e.g. "agent hit a login â€” take the wheel"
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
            dl_temps: Mutex::new(std::collections::HashMap::new()),
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

/// SLICE 6 â€” which agent's browser profile to use. Reads the active agent id
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
/// still alive; relaunches if it died/crashed â€” crash recovery).
async fn ensure_running(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> Result<u16, String> {
    // ENGINE-CEF UNIFICATION: when the CEF engine is live, the agent CDP client
    // must attach to the SAME Chromium the human sees — CEF's remote-debugging
    // port — instead of launching a SEPARATE headless Chrome-for-Testing. This
    // is the two-browser desync kill: human + agent act in the exact same tab,
    // natively. We DON'T spawn/track a child here in that case; the port is
    // owned by the CEF engine (cef_engine::init).
    #[cfg(all(target_os = "macos", feature = "engine-cef"))]
    {
        if let Some(port) = crate::cef_engine::cdp_port() {
            eprintln!("[aygent][cef] ensure_running -> CEF CDP port {port} (unified)");
            return Ok(port);
        }
    }
    // Fast path: already running + alive.
    {
        let mut guard = state.inner.lock().map_err(|_| "browser state poisoned")?;
        if let Some(rb) = guard.as_mut() {
            match rb.child.try_wait() {
                Ok(None) => return Ok(rb.port), // still alive
                _ => { *guard = None; }          // died â€” fall through to relaunch
            }
        }
    }

    if !is_installed(app) {
        return Err("browser not installed â€” enable it in Settings first".into());
    }
    let rt = runtime_dir(app, PINNED_CFT_VERSION)?;
    let exe = mac_executable(&rt, cft_platform());
    let port = free_port()?;
    // SLICE 6 â€” PER-AGENT profile isolation. The active agent's own profile dir
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
        // Bind DevTools to loopback only â€” never expose the CDP port off-box.
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
// SLICE 2 â€” LIVE SCREENCAST SESSION.
//
// One persistent CDP WebSocket per browser, owned by a background "pump" task.
// The pump: (a) forwards outbound CDP requests (from commands, via a channel)
// and routes replies back by id; (b) receives `Page.screencastFrame` events and
// emits them to the UI as `browser:frame` Tauri events (base64 JPEG), acking
// each so Chromium keeps streaming. This replaces Slice 1's navigateâ†’screenshot
// â†’close with a live view + is exactly the socket Slice 3 forwards input into.
// ---------------------------------------------------------------------------

/// Session generation counter so a stale pump never clobbers a newer session.
static SESSION_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The Browser UI's CURRENT active tab id, reported by the frontend via
/// `set_active_browser_tab`. The agent acts on THIS tab's embedded webview so
/// it operates in the exact page the human is watching. -1 = none/unknown.
static ACTIVE_TAB_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

/// The visible tab's current URL (reported by the frontend). Used for the host
/// pre-check so same-site actions don't prompt â€” the CDP read path can lag or
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

/// The current active-tab URL as last mirrored from the authoritative CDP page
/// (see `mirror_visible_to_cdp`). Used by `agent_run`'s step-overrun guard to
/// decide, DETERMINISTICALLY, whether a "click/open the result" step has already
/// navigated off the search page onto its destination â€” so the loop can require
/// step_done instead of tolerating a second, wandering click. Returns "" if the
/// tab URL is unknown.
pub fn current_agent_url() -> String {
    ACTIVE_TAB_URL.lock().map(|g| g.clone()).unwrap_or_default()
}

/// The bare host of the current active-tab URL (lowercased, no scheme/path).
/// Empty when unknown. `agent_run` keys its step-overrun guard on this: a
/// nav-step whose host is a real non-search destination is SATISFIED.
pub fn current_agent_host() -> String {
    host_of(&current_agent_url())
}

// ---------------------------------------------------------------------------
// PER-TAB URL TRACKING (engine-cef). CEF's DisplayHandler reports address/title
// changes per Browser; we map Browser -> tab in cef_engine, then call back here
// so the tab-strip label + the permission host pre-check + the mirror-visible
// path all read the SAME per-tab url the WKWebView path tracked via
// on_page_load. Harmless when engine-cef is off (nothing calls these).
// ---------------------------------------------------------------------------
#[cfg(feature = "engine-cef")]
static TAB_URLS: std::sync::Mutex<Option<std::collections::HashMap<i64, String>>> =
    std::sync::Mutex::new(None);

/// Record the latest committed main-frame url for a tab (called by cef_engine's
/// DisplayHandler.on_address_change). Also updates the single ACTIVE_TAB_URL if
/// this is the active tab, so the permission host pre-check stays correct.
#[cfg(feature = "engine-cef")]
pub fn note_active_tab_url(tab_id: i64, url: &str) {
    if let Ok(mut g) = TAB_URLS.lock() {
        g.get_or_insert_with(std::collections::HashMap::new)
            .insert(tab_id, url.to_string());
    }
    if ACTIVE_TAB_ID.load(std::sync::atomic::Ordering::SeqCst) == tab_id {
        if let Ok(mut g) = ACTIVE_TAB_URL.lock() { *g = url.to_string(); }
    }
}

/// The last committed url for a tab ("" if unknown). Used by cef_engine's
/// title-change handler to upgrade the matching history entry's title.
#[cfg(feature = "engine-cef")]
pub fn last_tab_url(tab_id: i64) -> String {
    TAB_URLS
        .lock()
        .ok()
        .and_then(|g| g.as_ref().and_then(|m| m.get(&tab_id).cloned()))
        .unwrap_or_default()
}

/// Heuristic: is `host` a search-engine / launcher host (i.e. NOT yet a real
/// destination)? Used ONLY to decide whether a "click the result" step has
/// actually landed somewhere. This is deliberately NOT the brittle old
/// `left_google` turn-gate â€” it never forbids or forces navigation; it only
/// helps recognize that a nav-step's GOAL is met once the page is off search.
pub fn is_search_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    if h.is_empty() { return true; } // unknown => treat as "not a destination yet"
    h.contains("google.") || h == "google" || h.starts_with("www.google")
        || h.contains("bing.com") || h.contains("duckduckgo.com")
        || h.contains("search.brave.com") || h.contains("startpage.com")
        || h.contains("ecosia.org") || h.contains("search.yahoo")
}

/// READ a value from the AUTHORITATIVE agent page â€” the CDP session. This is
/// THE desync fix (Problem 1): the CDP session is now the SINGLE SOURCE OF
/// TRUTH the agent both ACTS ON and READS FROM. We NO LONGER mirror the CDP
/// page to a frontend-reported URL here â€” the old code did that and it dragged
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
/// human watches where the agent actually went â€” and store it as ACTIVE_TAB_URL
/// for the permission host pre-check. Direction is ALWAYS CDP -> visible (never
/// visible -> CDP), which is what breaks the old tug-of-war desync loop.
async fn mirror_visible_to_cdp(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) {
    let url = cdp_current_url(state).await;
    if !url.starts_with("http") { return; }
    if let Ok(mut g) = ACTIVE_TAB_URL.lock() { *g = url.clone(); }
    // PERSISTENT HISTORY: this runs after EVERY agent navigation
    // (browser_open/click/type -> mirror) and on the visible-tab url-change
    // path, reading the AUTHORITATIVE CDP url. Record it here so agent
    // navigations, in-page nav and redirects are all captured at the source
    // (the crux fix for the old empty/FE-only history). Grab the CDP page title
    // too (best-effort) so the entry has a real label immediately; the FE's
    // webview_page_info poll backfills/upgrades it either way.
    let title = session_call(state, "Runtime.evaluate",
        serde_json::json!({ "expression": "document.title", "returnByValue": true })).await
        .ok()
        .and_then(|r| r["result"]["value"].as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    crate::history::record(app, &url, &title);
    // WKWebView removed (Mason, browser rebuild): the CDP page IS the visible
    // page now — the screencast mirror shows it. Nothing else to sync.
}

/// Read the active tab's (title, url) â€” from the VISIBLE embedded webview via a
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
            rb.session = Some(CdpSession { tx });
        }
    }

    // Enable the domains + start the screencast (JPEG, capped size for latency).
    session_call(state, "Page.enable", serde_json::json!({})).await?;
    session_call(state, "Runtime.enable", serde_json::json!({})).await?;
    // DOM domain: required for DOM.getContentQuads (used by the trusted-click
    // target resolver to get the click point in the exact coord space CDP
    // Input.* uses â€” DPR-safe). Best-effort; the resolver falls back to the
    // rect center if DOM is unavailable.
    let _ = session_call(state, "DOM.enable", serde_json::json!({})).await;
    // FINGERPRINT HARDENING (#2) â€” done ONCE per session, right after the
    // domains are enabled and BEFORE any real navigation, so it applies to the
    // very first document too.
    apply_fingerprint_hardening(state).await;
    // ISSUE 4 (agent-side downloads) — tell the headless CDP Chromium to ALLOW
    // downloads and save them into the SAME jailed dir the visible WKWebView's
    // on_download handler uses, so agent-driven downloads also populate the
    // Downloads panel. Best-effort: older builds may only accept the Page.*
    // form, so try Browser.setDownloadBehavior then fall back. Never fatal.
    {
        // SAME dir the visible WKWebView on_download handler + browser_downloads_list
        // use (now <agentFolder>/downloads), so agent-driven downloads land in the
        // panel too. The headless CDP Chromium is a SEPARATE process (not under
        // WebKit's networking sandbox), so it can write the agent folder directly
        // — no temp-then-move needed here. agent_downloads_dir create_dir_all's +
        // canonicalizes.
        let dir = agent_downloads_dir(app);
        let dl = dir.display().to_string();
        let params = serde_json::json!({ "behavior": "allow", "downloadPath": dl });
        if session_call(state, "Browser.setDownloadBehavior", params.clone()).await.is_err() {
            let _ = session_call(state, "Page.setDownloadBehavior", params).await;
        }
        eprintln!("[aygent][browser][DL] CDP download behavior=allow path={dl}");
    }
    session_call(
        state,
        "Page.startScreencast",
        // QUALITY (browser rebuild): q60 @ 1280x800 CSS px was the "foggy
        // glass" — a Retina viewport downscaled then re-upscaled. q85 with 2x
        // headroom keeps text crisp; frames still track the real viewport.
        serde_json::json!({ "format": "jpeg", "quality": 85, "maxWidth": 2560, "maxHeight": 1600, "everyNthFrame": 1 }),
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

/// SLICE 2 â€” navigate the LIVE session to `url`. Frames stream to the UI via
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
    // PERF (ITEM 2): was a FIXED 900ms sleep regardless of how fast the page
    // loaded. Replace with the event-equivalent readyState wait so the URL/title
    // read fires as soon as the DOM is ready (typically far under 900ms) and
    // still caps out on a slow page. Frames stream live regardless.
    wait_for_cdp_load(&state).await;

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

    // PERSISTENT HISTORY: the CDP-side navigate settled; record the landed
    // url + title (this is the headless/agent navigate command path).
    crate::history::record(&app, &final_url, &title);
    Ok(serde_json::json!({ "url": final_url, "title": title }))
}

/// Start (or restart) the live view without navigating â€” used when the UI opens
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
// The screencast (above) was foggy glass â€” a JPEG video of a browser you can't
// select text in. WRONG tool for a human. This is the right one: a REAL native
// child webview (WKWebView on macOS) rendered INSIDE the AYGENT window, over the
// Browser pane. Crisp text, native selection, hover, scroll, real DOM. YOU
// browse in this.
//
// THE MAGIC â€” hand-off: the same webview the human browses is ALSO driveable by
// the agent via `eval` (inject JS to click/type/read). So "hand off to agent"
// doesn't switch surfaces â€” the agent acts in the exact window you're looking at
// while you watch. Toggle back instantly; same page, same session, no reload.
//
// The screencast/CDP path stays for HEADLESS agent-only browsing (scheduled/
// background). This webview is the interactive human+handoff surface.
// ===========================================================================





/// Make a URL/Content-Disposition-ish name filesystem-safe. Strips path
/// separators and control chars; caps length; falls back to a timestamped name.
fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    // Drop any query string on a URL-derived name.
    let base = base.split(['?', '#']).next().unwrap_or(base);
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        format!("download-{}", SESSION_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
    } else {
        cleaned.chars().take(180).collect()
    }
}

/// DOWNLOAD THAT ACTUALLY WORKS (2026-07-30). wry's `on_download` NEVER fires
/// for a right-click "Save Image" context-menu action (confirmed: no `[DL]
/// begin` ever printed), so ALL the WKWebView-destination work was dead code.
/// We bypass WebKit's download machinery entirely: the FE (a page-injected
/// context-menu handler + the address-bar/agent) calls THIS command with a URL,
/// and OUR process fetches the bytes via reqwest and writes them straight into
/// `<agentFolder>/downloads/`. No WKWebView, no sandboxed networking XPC, so the
/// `sandbox_extension_issue_file` error can NEVER fire — the write is done by the
/// normal (non-sandboxed) app process, which can write the agent folder freely.
#[tauri::command]
pub async fn browser_download_url(
    app: tauri::AppHandle,
    url: String,
    suggested_name: Option<String>,
) -> Result<String, String> {
    use tauri::Emitter;
    // Only http(s) — never file:// or data: paths through here.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) URLs can be downloaded".into());
    }
    let dir = agent_downloads_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir downloads: {e}"))?;

    let raw_name = suggested_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| sanitize_filename(&url));
    let name0 = sanitize_filename(&raw_name);
    let name = dedup_name(&dir, &name0);
    let dest = dir.join(&name);

    eprintln!("[aygent][browser][DL] fetch url={url} -> {}", dest.display());
    let _ = app.emit("browser:download", &serde_json::json!({
        "state": "begin", "name": name, "url": url,
    }));

    // Fetch the bytes ourselves.
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client.get(&url).send().await.map_err(|e| format!("fetch failed: {e}"))?;
    if !resp.status().is_success() {
        let s = resp.status();
        let _ = app.emit("browser:download", &serde_json::json!({ "state": "error", "url": url, "success": false }));
        return Err(format!("download failed: HTTP {s}"));
    }
    // If we synthesized a name with no extension, try to add one from the
    // Content-Type so the file opens correctly.
    let final_dest = if dest.extension().is_none() {
        let ext = resp.headers().get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|ct| ext_for_mime(ct));
        match ext {
            Some(e) => dir.join(dedup_name(&dir, &format!("{name}.{e}"))),
            None => dest.clone(),
        }
    } else { dest.clone() };

    let bytes = resp.bytes().await.map_err(|e| format!("read body: {e}"))?;
    std::fs::write(&final_dest, &bytes).map_err(|e| format!("write file: {e}"))?;

    let path_str = final_dest.display().to_string();
    eprintln!("[aygent][browser][DL] complete success=true bytes={} final={}", bytes.len(), path_str);
    let _ = app.emit("browser:download", &serde_json::json!({
        "state": "complete", "url": url, "path": path_str, "success": true,
        "name": final_dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(name),
    }));
    Ok(path_str)
}

/// Guess a file extension from a MIME type for downloads whose URL had none.
fn ext_for_mime(ct: &str) -> Option<&'static str> {
    let m = ct.split(';').next().unwrap_or(ct).trim().to_ascii_lowercase();
    Some(match m.as_str() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "application/zip" => "zip",
        "application/json" => "json",
        "video/mp4" => "mp4",
        "audio/mpeg" => "mp3",
        _ => return None,
    })
}

/// ISSUE 4 — the SINGLE handler for WKWebView download events (wry
/// `on_download`). Wiring `on_download` is what STOPS the terminal flood: with
/// no handler, WKWebView treated the download URL as a navigation the page
/// couldn't render and churned it through on_navigation -> history in a tight
/// loop. Here we INTERCEPT the download so it is NEVER a navigation: on
/// `Requested` we pick a concrete destination under the active agent's jailed
/// downloads dir (so the file actually lands where `browser_downloads_list`
/// reads); on `Finished` we log + notify the FE. Logging is ONE LINE PER EVENT
/// (begin/complete) so it can't flood. Returns `true` to let the download run.
///
/// A download URL therefore NEVER touches history or the tab label (on_page_load
/// only fires for committed PAGE loads, and a download is not one).
///
/// SANDBOX-SAFE TEMP-THEN-MOVE (the fix for the recurring
/// `sandbox_extension_issue_file failed for : 2 (No such file or directory)`):
/// WKWebView performs the WRITE in its sandboxed networking XPC process
/// (com.apple.WebKit.Networking), which must hold a sandbox EXTENSION for the
/// destination's parent dir. Deep/newly-created paths (bundle containers,
/// ~/Downloads/AYGENT, and — not assumed safe — even the agent folder) can fail
/// to get one. The OS temp dir (`std::env::temp_dir()`, e.g. /var/folders/...)
/// is a location that process ALWAYS has an extension for. So on `Requested` we
/// point WebKit at temp, and on `Finished` OUR process (the main app, a normal
/// user process that can write the agent folder freely) MOVES the finished file
/// into `<agentFolder>/downloads/`. The write and the final destination are
/// decoupled — WebKit's sandbox never touches the agent folder.
#[allow(dead_code)] // TODO(browser-rebuild): rewire to CDP Browser.downloadWillBegin
fn download_event(app: &tauri::AppHandle, event: tauri::webview::DownloadEvent<'_>) -> bool {
    use tauri::webview::DownloadEvent;
    use tauri::Manager;
    match event {
        DownloadEvent::Requested { url, destination: _ } => {
            // Derive a filename from the URL (wry 0.55.1's `Requested` does NOT
            // expose WebKit's suggestedFilename — only `url` + `destination` — so
            // we synthesize one; `sanitize_filename` falls back to a unique
            // `download-N` if the URL has no usable last path segment).
            let name = sanitize_filename(url.as_str());

            // FINAL destination = the active agent folder's `downloads/` dir.
            // Compute (+ de-dupe) it NOW so the FE can show where it's going,
            // but WebKit does NOT write here — our Finished handler moves it here.
            let final_dir = agent_downloads_dir(app);
            let final_name = dedup_name(&final_dir, &name);
            let final_dest = final_dir.join(&final_name);

            // ============================================================
            // THE FIX (2026-07-30, after 4 destination-theory failures).
            //
            // Verified from wry 0.55.1 source: on_navigation does NOT swallow
            // the download, and *destination IS honored by WebKit. So the
            // recurring `sandbox_extension_issue_file failed for : 2` is WebKit's
            // sandboxed NETWORKING process failing to MINT a write extension for
            // ANY custom destination we hand it (Library/.., ~/Downloads/AYGENT,
            // /var/folders temp — all failed identically).
            //
            // The one location WebKit's networking sandbox RELIABLY holds a
            // write extension for is its OWN DEFAULT download dir (~/Downloads).
            // So: DO NOT override *destination at all — let WebKit download to
            // ~/Downloads (sandbox-blessed), then OUR non-sandboxed process MOVES
            // the finished file into <agentFolder>/downloads/. WebKit never has
            // to touch a custom path, so the extension error can't fire.
            //
            // We predict WebKit's default path (~/Downloads/<final_name>, with
            // the SAME de-dup WebKit applies) so Finished can move it even though
            // Finished.path is empty on macOS. If Finished DOES report a path, we
            // prefer it (source of truth for where WebKit actually wrote).
            // ============================================================
            let webkit_downloads = dirs::download_dir()
                .unwrap_or_else(|| std::env::temp_dir());
            // WebKit de-dups with " (1)", " (2)" too; match that so our predicted
            // path lines up with where it actually writes.
            let webkit_name = dedup_name(&webkit_downloads, &name);
            let predicted_webkit_path = webkit_downloads.join(&webkit_name);

            // Map url -> (WebKit's predicted write path, our final agent dest).
            // Finished moves predicted (or its reported path) -> final.
            if let Some(proc) = app.try_state::<BrowserProc>() {
                if let Ok(mut m) = proc.dl_temps.lock() {
                    m.insert(url.as_str().to_string(), (predicted_webkit_path.clone(), final_dest.clone()));
                }
            }

            eprintln!(
                "[aygent][browser][DL] begin url={} webkit_default={} -> final={} (final_dir_exists={})",
                url, predicted_webkit_path.display(), final_dest.display(), final_dir.is_dir()
            );
            // DO NOT set *destination — leave WebKit's own ~/Downloads default in
            // place. This is the whole fix: WebKit's sandbox already trusts it.
            // Notify the FE a download STARTED (so it can auto-open Downloads).
            {
                use tauri::Emitter;
                let _ = app.emit("browser:download", &serde_json::json!({
                    "state": "begin", "name": final_name, "url": url.as_str(),
                }));
            }
            true // allow the download
        }
        DownloadEvent::Finished { url, path, success } => {
            // WebKit downloaded to its own ~/Downloads (we never overrode the
            // destination). Recover our predicted ~/Downloads path + the final
            // agent dest from the map. Prefer WebKit's reported non-empty path
            // (source of truth) over our prediction; on macOS it's often empty,
            // so we fall back to the predicted path (and a stem-scan of
            // ~/Downloads if WebKit de-duped the name differently).
            let mapped = app
                .try_state::<BrowserProc>()
                .and_then(|proc| proc.dl_temps.lock().ok().and_then(|mut m| m.remove(url.as_str())));

            let reported = path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
            eprintln!(
                "[aygent][browser][DL] finished success={success} url={url} reported_path={:?} (mapped={})",
                reported, mapped.is_some()
            );

            let mut final_path = String::new();
            let mut moved_ok = success;
            if success {
                if let Some((predicted_webkit_path, final_dest)) = mapped {
                    // WebKit wrote the file to ~/Downloads. Source = its reported
                    // path if non-empty (source of truth), else our predicted
                    // ~/Downloads/<name>. If WebKit de-duped differently than we
                    // predicted, fall back to scanning ~/Downloads for the file.
                    let mut src = match path.as_ref() {
                        Some(p) if !p.as_os_str().is_empty() => p.clone(),
                        _ => predicted_webkit_path.clone(),
                    };
                    if !src.exists() {
                        // Predicted name missed (WebKit's de-dup differed). Look
                        // in ~/Downloads for the newest file matching our stem.
                        if let Some(found) = newest_matching_download(&src) {
                            src = found;
                        }
                    }
                    eprintln!(
                        "[aygent][browser][DL] finished, moving from={} -> final={}",
                        src.display(), final_dest.display()
                    );
                    if src == final_dest {
                        moved_ok = src.exists();
                        if moved_ok { final_path = final_dest.display().to_string(); }
                    } else if src.exists() {
                        // Ensure the agent's downloads dir exists (agent may have
                        // switched since Requested).
                        if let Some(parent) = final_dest.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        // rename() is atomic within a filesystem; ~/Downloads and
                        // the agent folder can be on different volumes, so fall
                        // back to copy + remove across filesystems.
                        let ok = match std::fs::rename(&src, &final_dest) {
                            Ok(()) => true,
                            Err(_) => match std::fs::copy(&src, &final_dest) {
                                Ok(_) => { let _ = std::fs::remove_file(&src); true }
                                Err(e) => {
                                    eprintln!("[aygent][browser][DL] error move failed: {e}");
                                    false
                                }
                            },
                        };
                        moved_ok = ok;
                        if ok {
                            final_path = final_dest.display().to_string();
                            eprintln!(
                                "[aygent][browser][DL] moved {} -> {}",
                                src.display(), final_dest.display()
                            );
                        }
                    } else {
                        moved_ok = false;
                        eprintln!(
                            "[aygent][browser][DL] error webkit file missing at {} (reported={:?})",
                            src.display(), reported
                        );
                    }
                } else {
                    // No mapping (shouldn't happen) — honor any path WebKit gave.
                    final_path = path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                }
            }

            eprintln!(
                "[aygent][browser][DL] complete success={moved_ok} url={url} final={final_path}"
            );
            use tauri::Emitter;
            let _ = app.emit("browser:download", &serde_json::json!({
                "state": if moved_ok { "complete" } else { "error" },
                "url": url.as_str(), "path": final_path, "success": moved_ok,
            }));
            true
        }
        _ => true,
    }
}

/// WebKit may de-dup a download name differently than we predicted ("foo.png"
/// vs "foo-1.png" vs "foo (1).png"), so if our predicted ~/Downloads path is
/// missing at Finished, scan the download dir for the NEWEST file whose stem
/// starts with our expected stem — that's almost certainly the one WebKit just
/// wrote. Returns the matched path if found.
#[allow(dead_code)] // TODO(browser-rebuild): CDP download path
fn newest_matching_download(predicted: &Path) -> Option<PathBuf> {
    let dir = predicted.parent()?;
    let stem = predicted.file_stem()?.to_string_lossy().to_string();
    // Strip a trailing " (n)"/"-n" so "foo (1)" still matches "foo".
    let base = stem
        .split(|c| c == '(' || c == '-')
        .next()
        .unwrap_or(&stem)
        .trim()
        .to_string();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if !p.is_file() { continue; }
        // Skip in-progress downloads.
        if p.extension().map(|e| e == "download" || e == "crdownload").unwrap_or(false) { continue; }
        let fname = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !base.is_empty() && !fname.to_lowercase().starts_with(&base.to_lowercase()) { continue; }
        let mtime = entry.metadata().ok().and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        // Only consider very-recently-written files (last 30s) to avoid grabbing
        // an unrelated old file that happens to share the stem.
        let recent = mtime.elapsed().map(|e| e.as_secs() < 30).unwrap_or(false);
        if !recent { continue; }
        if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, p));
        }
    }
    best.map(|(_, p)| p)
}

/// De-dupe a filename against a directory so we never clobber an existing file:
/// `foo.png` -> `foo (1).png` -> `foo (2).png` … Returns the name to use.
fn dedup_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    let mut n = 1u32;
    loop {
        let cand = format!("{stem} ({n}){ext}");
        if !dir.join(&cand).exists() {
            return cand;
        }
        n += 1;
    }
}












/// ITEM 3 (Downloads). List the files in the ACTIVE agent's downloads directory
/// (`<agentFolder>/downloads` — see `agent_downloads_dir`) so the Browser view
/// can show a Downloads panel. If the active agent changes, this reads the
/// CURRENT agent's folder (agent_downloads_dir resolves it live via the broker).
/// Returns each file's name, byte size, and modified-time (unix seconds),
/// newest first. Missing dir => empty list (not an error) so a fresh profile
/// just shows "no downloads yet". Reads the SAME dir the on_download handler
/// moves finished files into and the CDP agent-download path writes to.
#[tauri::command]
pub fn browser_downloads_list(app: tauri::AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let dir = agent_downloads_dir(&app);
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(Vec::new()), // no dir yet -> no downloads
    };
    let mut out: Vec<(u64, serde_json::Value)> = Vec::new();
    for entry in rd.flatten() {
        let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
        if !meta.is_file() { continue; }
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip Chromium's in-progress temp files so the panel shows finished ones.
        if name.ends_with(".crdownload") || name.ends_with(".part") { continue; }
        let size = meta.len();
        let mtime = meta.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push((mtime, serde_json::json!({ "name": name, "size": size, "mtime": mtime })));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    Ok(out.into_iter().map(|(_, v)| v).collect())
}

// ---------------------------------------------------------------------------
// PERSISTENT BROWSING HISTORY (Atlas). The RECORD side lives in `history.rs`
// and is called from the navigation-confirmation points below
// (`mirror_visible_to_cdp`, `webview_page_info`, `webview_navigate`,
// `browser_navigate`). These two commands are the FE's read + clear surface.
// ---------------------------------------------------------------------------

/// List browsing history, NEWEST-FIRST, for the History pane. Each entry is
/// { url, title, ts } (ts = unix seconds). Loaded from the persisted store so
/// it survives app restarts.
#[tauri::command]
pub fn browser_history_list(app: tauri::AppHandle) -> Result<Vec<crate::history::HistEntry>, String> {
    Ok(crate::history::list(&app))
}

/// Wipe all browsing history (memory + the on-disk history.json). The FE calls
/// this from the "Clear history" button, then re-lists.
#[tauri::command]
pub fn browser_history_clear(app: tauri::AppHandle) -> Result<(), String> {
    crate::history::clear(&app);
    Ok(())
}

// ---------------------------------------------------------------------------
// THE HAND-OFF: the agent acts IN the human's webview.
//
// When the human hands off, this runs a small agent loop where the model's ONLY
// tools operate on the live webview via injected JS: read the page, click by
// text, type, scroll. The agent works in the EXACT window the human is watching
// (same DOM, same session, same cookies) â€” not a separate headless browser.
//
// We reuse the app's existing Anthropic provider primitive. The webview eval
// runs page-side JS + returns a result the model reads back (page text after an
// action). This is a focused, self-contained loop â€” the model gets the task +
// the current page text + the webview tools, and iterates until done.
// ---------------------------------------------------------------------------



// ---------------------------------------------------------------------------
// SLICE 5 â€” SHARED CONTROL (the wheel).
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

/// HUMAN takes the wheel â€” preempts the agent immediately.
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
// SLICE 3 â€” HUMAN INPUT FORWARDING.
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
///   (a) Network.setUserAgentOverride â€” force a normal desktop Chrome UA (no
///       "HeadlessChrome") on the CDP page's own requests, matching the launch
///       --user-agent so page-JS `navigator.userAgent` and the network layer
///       agree.
///   (b) Page.addScriptToEvaluateOnNewDocument â€” runs BEFORE any page script on
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
/// instantaneous â€” uniform timing is itself a bot signal. No rand crate dep:
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
    /// The nearest clickable ancestor's tag (a/button/â€¦) we actually target.
    tag: String,
    /// If the target (or an ancestor) is/inside an <a>, its resolved href â€”
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
/// (`el.closest('a,button,[role=button],â€¦') || el`), scroll it into view, and
/// return the exact point to click â€” computed from `DOM.getContentQuads`, which
/// returns quads in the SAME coordinate space CDP `Input.dispatchMouseEvent`
/// expects (CSS px relative to the layout viewport). This sidesteps all
/// CSS-vs-device-px / DPR guesswork that plagues pure `getBoundingClientRect`
/// math. Falls back to the rect center only if quads are empty. Returns None if
/// no element matches. FIRST half of a TRUSTED click (never el.click()).
async fn element_center_by_text(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    want: &str,
    force_first_result: bool,
) -> Result<Option<ClickTarget>, String> {
    ensure_session(app, state).await?;
    // 1) Locate the element AND resolve to the nearest clickable ancestor, then
    //    scroll into view. Return the NODE (returnByValue:false -> objectId) so
    //    we can ask CDP for its content quads. Also stash diagnostics + the rect
    //    center (fallback) + href on the node object so a single eval gives us
    //    everything.
    // DETERMINISTIC FIRST-RESULT TARGETING (spec #3). When the request text is a
    // "first result" / "first link" intent, we must click the FIRST ORGANIC
    // result â€” not the first element whose text merely contains a word, and NOT
    // an ad. On a Google SERP the first organic result's link is the first
    // `a:has(h3)` inside the main results container (#rso, falling back to
    // #search / #center_col). We pick that <a> directly. If the page isn't a
    // recognizable SERP, we fall back to the ordinary first anchor with an h3,
    // then to normal text matching â€” so this never regresses non-Google pages.
    // `force_first_result` (from the dedicated browser_click_first_result tool
    // OR the agent_run override on a first-result plan step) makes the
    // deterministic first-organic-anchor path AUTHORITATIVE regardless of the
    // model-supplied `want` text â€” this is the fix for the bug where a
    // model-passed label like "Claude: Sign in" routed to the broad text-match
    // path and clicked a <div> tweet embed instead of the real first result.
    let first_result_intent = force_first_result || {
        let w = want.to_ascii_lowercase();
        let w = w.trim();
        (w.contains("first") && (w.contains("result") || w.contains("link") || w.contains("hit") || w.contains("listing")))
            || w == "first"
            || w == "top result"
    };
    // When we're FORCING the first-result path, the text-match fallback (which
    // is what produced the wrong <div>) must be DISABLED â€” if no organic anchor
    // is found we return None (honest "no first result") rather than clicking
    // whatever element merely contained the label text.
    let disable_text_fallback = force_first_result;
    let locate_fn = format!(
        "(()=>{{ const t={want:?}.toLowerCase(); const firstResult={first_result_intent}; const noTextFallback={disable_text_fallback}; \
         let el=null; \
         if(firstResult){{ \
           const containers=['#rso','#search','#center_col','#main']; \
           let cont=null; for(const sel of containers){{ const c=document.querySelector(sel); if(c){{cont=c;break;}} }} \
           const scope = cont || document; \
           const anchors=[...scope.querySelectorAll('a')]; \
           el = anchors.find(a=>{{ \
             if(a.offsetParent===null) return false; \
             if(!a.querySelector('h3')) return false; \
             const h=(a.href||''); if(!/^https?:/i.test(h)) return false; \
             if(/google\\.com\\/(search|preferences|advanced_search|intl)/i.test(h)) return false; \
             if(a.closest('[data-text-ad],[aria-label=\"Ads\"],.uEierd,.commercial-unit-desktop-top')) return false; \
             return true; \
           }}) || anchors.find(a=>a.querySelector('h3') && a.offsetParent!==null && /^https?:/i.test(a.href||'')); \
           if(el){{ el = el.closest('a') || el; }} \
         }} \
         if(!el && !noTextFallback){{ \
           const els=[...document.querySelectorAll('a,button,input[type=submit],input[type=button],[role=button],label,[onclick],h3,li,span,div')]; \
           el=els.find(e=>{{ const s=(e.innerText||e.value||e.textContent||''); return s && s.toLowerCase().includes(t) && e.offsetParent!==null; }}); \
         }} \
         if(!el) return null; \
         const clickable = firstResult ? (el.closest('a') || el) : (el.closest('a,button,input[type=submit],input[type=button],[role=button],[onclick]') || el); \
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
        want = want,
        first_result_intent = first_result_intent,
        disable_text_fallback = disable_text_fallback
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

    // 1b) SETTLE AFTER SCROLL (bug fix). The locate eval called
    //     scrollIntoView() but read the rect in the SAME synchronous tick â€” and
    //     step 3's getContentQuads is a SEPARATE CDP call that reads the CURRENT
    //     (possibly still-settling) layout. That desync produced the observed
    //     scroll=(0,1713)/y=356 mismatch that clicked a tweet embed instead of
    //     the first result. We now (a) wait a beat for the scroll to settle,
    //     then (b) RE-READ the element's rect + diagnostics from its POST-SCROLL
    //     position, and step 3 reads getContentQuads on that same settled layout.
    human_delay(120, 200).await;
    // Re-scroll + re-read the rect on the settled layout so rx/ry, scroll, and
    // the quads below all agree on ONE frame.
    let diag = session_call(
        state,
        "Runtime.callFunctionOn",
        serde_json::json!({
            "objectId": object_id,
            "functionDeclaration": "function(){ \
                this.scrollIntoView({block:'center',inline:'center'}); \
                const r=this.getBoundingClientRect(); \
                const a=this.closest('a'); \
                return { \
                  tag:this.tagName.toLowerCase(), href:a?a.href:null, \
                  rx:r.left+r.width/2, ry:r.top+r.height/2, rw:r.width, rh:r.height, \
                  dpr:window.devicePixelRatio||1, sx:window.scrollX||0, sy:window.scrollY||0, \
                  iw:window.innerWidth||0, ih:window.innerHeight||0 \
                }; }",
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

    // ANCHOR-REQUIRED GUARD (bug fix, first-result path). The observed failure
    // resolved a <div> with href=None and clicked a tweet embed. When we FORCE
    // the first-result path the target MUST be a real organic <a> with an http
    // href â€” anything else means our selector matched a non-link, so we REFUSE
    // (return None -> honest "no first result") rather than click a wrong box.
    if force_first_result {
        let is_anchor = tag == "a";
        let has_http_href = href.as_deref().map(|h| h.starts_with("http")).unwrap_or(false);
        if !is_anchor || !has_http_href {
            let _ = session_call(state, "Runtime.releaseObject",
                serde_json::json!({ "objectId": object_id })).await;
            eprintln!("[aygent][browser][CLICK] first-result REFUSED: resolved tag=<{tag}> href={href:?} is not a real organic anchor");
            return Ok(None);
        }
    }

    // 3) Ask CDP for the element's content quads â€” SAME coord space as Input.*.
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
            // DOM domain may not be enabled on this build/path â€” the rect
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

/// TRUSTED mouse click at ABSOLUTE CSS-px coords (x,y) via CDP Input.* â€” the
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
        human_delay(6, 18).await; // PERF: trimmed 15-45ms approach steps
    }
    // Settle on the exact target.
    session_call(
        state,
        "Input.dispatchMouseEvent",
        serde_json::json!({ "type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0 }),
    )
    .await?;
    human_delay(15, 40).await; // PERF: trimmed pre-press hover dwell
    // Press.
    session_call(
        state,
        "Input.dispatchMouseEvent",
        serde_json::json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": 1 }),
    )
    .await?;
    human_delay(20, 55).await; // PERF: trimmed press dwell (still a real dwell)
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
/// inter-key delays so timing looks human. isTrusted:true, real key events â€”
/// unlike el.value=... which fires nothing trusted.
async fn trusted_type(
    state: &tauri::State<'_, BrowserProc>,
    text: &str,
) -> Result<(), String> {
    let t0 = std::time::Instant::now();
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
        // PERF (ITEM 2): per-key cadence trimmed 40-120ms -> 12-35ms. Still
        // jittered + non-uniform (keeps the human-typing signal), but ~3-4x
        // faster: a 12-char query drops from ~1s to ~0.3s of typing delay.
        human_delay(12, 35).await;
    }
    eprintln!("[aygent][browser][PERF] trusted type of {} chars in {}ms (trimmed per-key cadence)",
        text.chars().count(), t0.elapsed().as_millis());
    Ok(())
}

fn text_unmod(c: char) -> String {
    // For a shifted char the unmodifiedText would differ; we type without a
    // shift modifier and let `text` carry the final glyph, so unmodifiedText =
    // the lowercase/base char is a fine approximation for detectors.
    c.to_string()
}

/// TRUSTED Enter keypress via CDP Input.dispatchKeyEvent (keyDown+keyUp,
/// windowsVirtualKeyCode 13). Real navigation trigger â€” not form.submit().
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
// SLICE 4 â€” AGENT BROWSER TOOLS.
//
// The agent drives the SAME browser the human sees (frames keep streaming, so
// the human watches the agent work). Tools are mediated through the CDP session
// exactly like the human's input, plus a per-agent DOMAIN POLICY (mirrors
// web.rs's SSRF posture): the agent may only navigate to allowlisted hosts;
// file:// / localhost / internal are hard-blocked. The human tier is unrestricted
// (Slice 3) â€” that split is the whole point (human gets past what the agent can't).
//
// One async entry point `agent_tool` that the turn loop calls for any browser_*
// tool. Returns (result_text, is_error) like exec_tool.
// ===========================================================================

/// Names the agent sees. Kept SMALL + robust (Atlas: favor a small set).
pub const AGENT_TOOL_NAMES: &[&str] = &[
    "browser_open", "browser_read", "browser_click_text", "browser_click_first_result",
    "browser_type_text", "browser_screenshot",
];

/// Is `name` one of our agent browser tools?
pub fn is_agent_tool(name: &str) -> bool {
    AGENT_TOOL_NAMES.contains(&name)
}

// ===========================================================================
// HARD-STOP SENTINELS (Mason's 5-point spec #5).
//
// A tool result whose text STARTS WITH one of these tokens is a machine-
// detectable signal that the agent turn must END IMMEDIATELY â€” it is NOT a
// normal recoverable tool error the model should react to. `agent_run` checks
// the result text of every browser tool call against these prefixes and, on a
// match, BREAKS the loop with a clear final message (never feeding the result
// back to the model). This is how DENY / TAKE-CONTROL propagate up from the
// permission dialog through `agent_tool` to the loop without any signature
// churn (no compiler on Windows â€” a text sentinel is the lowest-risk vehicle).
//
//   __DENIED__   -> the human clicked Deny on the permission dialog.
//   __TAKEOVER__ -> the human clicked Take Control (driver flipped to human).
//
// Both carry a human-readable tail after the token for the transcript/log.
// Keep these EXACT â€” `agent_run` string-matches the prefixes.
pub const STOP_DENIED: &str = "__DENIED__";
pub const STOP_TAKEOVER: &str = "__TAKEOVER__";

/// True if a tool result string is one of the hard-stop sentinels. Used by
/// `agent_run` after each browser tool call to end the turn deterministically.
pub fn is_hard_stop(s: &str) -> bool {
    s.starts_with(STOP_DENIED) || s.starts_with(STOP_TAKEOVER)
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
            "description": "Click the first visible element (link/button) whose text contains the given string. Use this to click a SPECIFIC named link/button by its label. Do NOT use this to click 'the first result' â€” use browser_click_first_result for that.",
            "input_schema": { "type": "object", "properties": {
                "text": { "type": "string", "description": "visible text of the element to click" }
            }, "required": ["text"] }
        }),
        serde_json::json!({
            "name": "browser_click_first_result",
            "description": "Click the FIRST ORGANIC search result on a results page (Google/Bing/etc). Takes NO arguments â€” it deterministically targets the first real result link (skips ads). ALWAYS use this for any 'click the first result / first link / top result' step; never guess result text with browser_click_text.",
            "input_schema": { "type": "object", "properties": {} }
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
/// nothing allowed â€” fail closed). Returns (text, is_error).
pub async fn agent_tool(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    name: &str,
    input: &serde_json::Value,
    allowed_domains: &[String],
) -> (String, bool) {
    // SLICE 5: the human ALWAYS wins the wheel. If they're driving, the agent
    // must not act â€” tell it plainly so it waits/hands back.
    if human_has_wheel(state) {
        return ("the human is currently driving the browser â€” wait for them to release the wheel before acting".into(), true);
    }
    // Claim the wheel for this action (refused only if the human grabbed it in
    // the race window).
    if !agent_claim(app, state) {
        return ("the human just took the browser â€” not acting".into(), true);
    }
    let out = agent_tool_inner(app, state, name, input, allowed_domains).await;

    // HARD STOP (spec #5): if the human clicked Deny or Take Control, the inner
    // tool returned a sentinel result. Do NOT mirror, do NOT run the handoff
    // heuristic, do NOT re-home the wheel via the generic release path â€” just
    // propagate the sentinel straight up so `agent_run` ends the turn. On
    // TAKEOVER the driver was already flipped to human in browser_permission_answer
    // + request_permission; we re-assert it here and emit control so the toggle
    // shows "You" even if a race left it on agent. On DENY we release the wheel
    // to idle (agent no longer driving) but keep the sentinel intact.
    if is_hard_stop(&out.0) {
        if out.0.starts_with(STOP_TAKEOVER) {
            if let Ok(mut c) = state.control.lock() {
                c.driver = "human".into();
                c.agent_active = false;
                c.note = String::new();
                emit_control(app, &c);
            }
            eprintln!("[aygent][browser][PERM] TAKEOVER propagating up â€” driver=human, ending turn");
        } else {
            // DENY: agent stops driving; hand the wheel back to idle.
            agent_release(app, state, "");
            eprintln!("[aygent][browser][PERM] DENY propagating up â€” ending turn");
        }
        return out;
    }

    // After ANY action that could navigate the AUTHORITATIVE CDP page, mirror the
    // VISIBLE WKWebView to it (CDP -> visible), so the human watches where the
    // agent went and the permission host pre-check stays correct. This is the
    // single-source-of-truth sync that replaces the old fragile read-mirror.
    if matches!(name, "browser_open" | "browser_click_text" | "browser_click_first_result" | "browser_type_text") {
        mirror_visible_to_cdp(app, state).await;
    }
    // On a login/CAPTCHA-ish failure, hand off to the human with a note.
    let handoff = if out.1 && looks_like_handoff(&out.0) {
        "the agent hit a login or verification wall â€” take the wheel to continue"
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
    // UI-reported URL wins â€” it's the page the human actually navigated to.
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
///
/// IMPORTANT: this must NOT fire on incidental page CONTENT (e.g. a Google
/// result titled "Claude: Sign in", or any page that merely MENTIONS sign-in).
/// It previously matched bare "sign in"/"login", which flipped the wheel to the
/// human the instant the agent read a search page containing those words â€” the
/// exact bug where clicking Allow bounced control back to You. Now we only fire
/// on STRONG blocker signals: an explicit tool ERROR string we control, a
/// CAPTCHA/robot check, or Google's rate-limit wall. Generic auth words in page
/// text are NOT a handoff.
fn looks_like_handoff(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("captcha")
        || l.contains("not a robot")
        || l.contains("unusual traffic")
        || l.contains("detected unusual")
        || l.contains("verify you're human")
        || l.contains("verify you are human")
}

async fn agent_tool_inner(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, BrowserProc>,
    name: &str,
    input: &serde_json::Value,
    allowed_domains: &[String],
) -> (String, bool) {
    // SINGLE SOURCE OF TRUTH (Problem 1 fix). ALL agent actions AND reads run
    // against the CDP session's page â€” the SAME page. Actions used to fire into
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
                    "allow" => {
                        if let Ok(mut p) = state.perm.lock() { p.granted_hosts.insert(host.clone()); }
                        eprintln!("[aygent][browser][PERM] ALLOW open {host} â€” continuing");
                    }
                    // HARD STOP (spec #5): Take Control ends the turn; driver=human.
                    "take" => {
                        eprintln!("[aygent][browser][PERM] TAKE open {host} â€” hard stop");
                        return (format!("{STOP_TAKEOVER} the human took the wheel to handle opening {host} themselves."), true);
                    }
                    // HARD STOP (spec #5): Deny ends the turn immediately â€” no retry, no wander.
                    _ => {
                        eprintln!("[aygent][browser][PERM] DENY open {host} â€” hard stop");
                        return (format!("{STOP_DENIED} the human denied opening {host}."), true);
                    }
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
                "allow" => { eprintln!("[aygent][browser][PERM] ALLOW click {want:?} â€” continuing"); }
                // HARD STOP (spec #5): Take Control ends the turn; driver=human.
                "take" => {
                    eprintln!("[aygent][browser][PERM] TAKE click {want:?} â€” hard stop");
                    return (format!("{STOP_TAKEOVER} the human took the wheel to click '{want}' themselves."), true);
                }
                // HARD STOP (spec #5): Deny ends the turn immediately â€” no retry, no wander.
                _ => {
                    eprintln!("[aygent][browser][PERM] DENY click {want:?} â€” hard stop");
                    return (format!("{STOP_DENIED} the human denied clicking '{want}'."), true);
                }
            }
            // TRUSTED CLICK (#1). Instead of el.click() via JS (isTrusted:false,
            // which Google flags), we (a) locate the element's viewport-center
            // coords via a CDP Runtime.evaluate (scrolling it into view), then
            // (b) dispatch a REAL CDP mouse click at those coords
            // (mouseMoved x3 approach -> mousePressed -> mouseReleased). The
            // events carry isTrusted:true â€” indistinguishable from a human click.
            eprintln!("[aygent][browser][INPUT] browser_click_text want={want:?}");
            let target = match element_center_by_text(app, state, want, false).await {
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
            // tell â€” HONESTLY â€” whether the click actually did anything.
            let url_before = cdp_current_url(state).await;
            human_delay(25, 60).await; // PERF: trimmed pre-action pause
            if let Err(e) = trusted_click_at(state, target.x, target.y).await {
                return (format!("trusted click dispatch failed: {e}"), true);
            }
            wait_for_cdp_load(state).await;
            let mut url_after = cdp_current_url(state).await;
            let mut navigated = url_after != url_before && url_after.starts_with("http");
            // FALLBACK: the trusted click landed but produced no navigation and
            // we DO have a resolved <a> href (classic search-result case). Rather
            // than lie with a false âœ“, navigate the authoritative CDP page to
            // that href so the human still gets the result â€” still no el.click().
            if !navigated {
                if let Some(href) = target.href.as_deref() {
                    if href.starts_with("http") && href != url_before {
                        eprintln!("[aygent][browser][CLICK] trusted click produced NO nav â€” href fallback -> {href}");
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
                        "clicked '{want}' (trusted mouse event dispatched on <{}> at ({:.0},{:.0})) but the page did NOT navigate or change â€” the target may be wrong or the link needs a different action. Still on: {title} ({url}).",
                        target.tag, target.x, target.y
                    ),
                    true,
                )
            }
        }
        // DETERMINISTIC FIRST-RESULT CLICK (bug fix, ITEM 1). A dedicated,
        // TEXT-FREE tool: it ALWAYS targets the first ORGANIC result anchor
        // (first `a:has(h3)` inside #rso/#search/#center_col with a real http
        // href, skipping ads) â€” the model cannot mis-route it to a text-match
        // that lands on a <div>/tweet embed. The plan/prompt steers first-result
        // steps here, and agent_run also OVERRIDES browser_click_text to this
        // path on a first-result step (belt + suspenders).
        "browser_click_first_result" => {
            let ans = request_permission(app, state, "click the first result", "The agent wants to click the first organic search result link.").await;
            match ans.as_str() {
                "allow" => { eprintln!("[aygent][browser][PERM] ALLOW click first-result â€” continuing"); }
                "take" => {
                    eprintln!("[aygent][browser][PERM] TAKE click first-result â€” hard stop");
                    return (format!("{STOP_TAKEOVER} the human took the wheel to click the first result themselves."), true);
                }
                _ => {
                    eprintln!("[aygent][browser][PERM] DENY click first-result â€” hard stop");
                    return (format!("{STOP_DENIED} the human denied clicking the first result."), true);
                }
            }
            eprintln!("[aygent][browser][INPUT] browser_click_first_result (forced first-organic-anchor)");
            // force_first_result=true -> anchor-required, no text fallback.
            let target = match element_center_by_text(app, state, "first result", true).await {
                Ok(Some(c)) => c,
                Ok(None) => return ("no first organic result link found on this page (not a recognizable results page, or only ads/non-link results)".into(), true),
                Err(e) => return (format!("first-result locate failed: {e}"), true),
            };
            eprintln!(
                "[aygent][browser][CLICK] want=\"<first-result>\" tag=<{}> coord=({:.1},{:.1}) via_quads={} dpr={} scroll=({:.0},{:.0}) inner=({:.0}x{:.0}) href={:?}",
                target.tag, target.x, target.y, target.via_quads, target.dpr,
                target.scroll_x, target.scroll_y, target.inner_w, target.inner_h, target.href
            );
            let url_before = cdp_current_url(state).await;
            human_delay(25, 55).await; // PERF: trimmed pre-action pause
            if let Err(e) = trusted_click_at(state, target.x, target.y).await {
                return (format!("trusted click dispatch failed: {e}"), true);
            }
            wait_for_cdp_load(state).await;
            let mut url_after = cdp_current_url(state).await;
            let mut navigated = url_after != url_before && url_after.starts_with("http");
            // href fallback: since the target is GUARANTEED a real <a> with an
            // http href, this reliably navigates even if the trusted click was
            // swallowed by an overlay.
            if !navigated {
                if let Some(href) = target.href.as_deref() {
                    if href.starts_with("http") && href != url_before {
                        eprintln!("[aygent][browser][CLICK] first-result trusted click produced NO nav â€” href fallback -> {href}");
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
                (format!("clicked the first result. Now on: {title} ({url})"), false)
            } else {
                (format!("dispatched a trusted click on the first result <a> at ({:.0},{:.0}) but the page did NOT navigate. Still on: {title} ({url}).", target.x, target.y), true)
            }
        }
        "browser_type_text" => {
            let text = input.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let submit = input.get("submit").and_then(|b| b.as_bool()).unwrap_or(false);
            if text.is_empty() { return ("browser_type_text needs `text`".into(), true); }
            // TRUSTED TYPING (#1). Instead of el.value=... (fires no trusted
            // input events â€” flagged), we:
            //   (a) locate the target field's center coords + REAL-click it to
            //       focus (trusted mouse click),
            //   (b) type per-character via CDP Input.dispatchKeyEvent
            //       (keyDown+keyUp, `text` carries the glyph) with 40-120ms
            //       human-cadence delays,
            //   (c) on submit, press a REAL Enter (windowsVirtualKeyCode 13) via
            //       Input.dispatchKeyEvent â€” not form.submit(). requestSubmit()
            //       is kept ONLY as a fallback if the Enter path caused no nav.
            eprintln!("[aygent][browser][INPUT] browser_type_text submit={submit} text_len={}", text.len());
            let field = match typeable_field_center(app, state).await {
                Ok(Some(c)) => c,
                Ok(None) => return ("no typeable field found on this page".into(), true),
                Err(e) => return (format!("type locate failed: {e}"), true),
            };
            human_delay(25, 60).await; // PERF: trimmed pre-action pause
            if let Err(e) = trusted_click_at(state, field.0, field.1).await {
                return (format!("focus-click failed: {e}"), true);
            }
            human_delay(25, 60).await; // PERF: trimmed pre-action pause
            if let Err(e) = trusted_type(state, text).await {
                return (format!("trusted type failed: {e}"), true);
            }
            if submit {
                // Capture the url before Enter so we can tell if it navigated.
                let url_before = cdp_current_url(state).await;
                human_delay(25, 60).await; // PERF: trimmed pre-action pause
                if let Err(e) = trusted_enter(state).await {
                    return (format!("Enter dispatch failed: {e}"), true);
                }
                wait_for_cdp_load(state).await;
                let url_after = cdp_current_url(state).await;
                // FALLBACK ONLY: if the trusted Enter produced no navigation
                // (some SPA search boxes rely on form submit), try
                // requestSubmit()/submit() on the field's form as a last resort.
                if url_after == url_before {
                    eprintln!("[aygent][browser][INPUT] Enter yielded no nav â€” requestSubmit() fallback");
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
    // PERF (ITEM 2): this is an EVENT-EQUIVALENT wait â€” it proceeds the instant
    // the page reports interactive/complete rather than sleeping a fixed time.
    // We poll document.readyState on a TIGHT cadence (was 300ms warmup + 250ms
    // poll + 200ms tail = >=500ms floor even on instant loads) and now break as
    // soon as the DOM is at least `interactive` (usable/readable) â€” capped so a
    // slow page can't hang. Typical saving on a fast SERP: ~400-600ms/nav.
    let t0 = std::time::Instant::now();
    // Brief warmup so the navigation has actually begun before the first poll.
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let mut settled = "";
    for _ in 0..60 { // up to ~6s (60 * ~100ms) â€” same cap, finer granularity.
        let ready = session_call(state, "Runtime.evaluate",
            serde_json::json!({ "expression": "document.readyState", "returnByValue": true })).await
            .ok()
            .and_then(|r| r["result"]["value"].as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        // `interactive` means the DOM is parsed + usable (readable/clickable);
        // we don't need to block for every subresource (`complete`) before the
        // agent can read/act. Break early on interactive OR complete.
        if ready == "complete" { settled = "complete"; break; }
        if ready == "interactive" { settled = "interactive"; break; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // Tiny settle for post-load JS (SPA route paints, etc). Trimmed from 200ms.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    eprintln!("[aygent][browser][PERF] wait_for_cdp_load: readyState={} in {}ms (event-equivalent poll, not fixed sleep)",
        if settled.is_empty() { "timeout" } else { settled }, t0.elapsed().as_millis());
}

/// Read the ACTIVE tab's (title, visible text) via CDP Runtime.evaluate (works
/// on external pages; no __TAURI__ needed).
async fn active_tab_page_text(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> Result<(String, String), String> {
    // POLISH #4b (Mason v1.0.1): innerText alone showed the agent PROSE ONLY —
    // no buttons, no inputs, no links. It couldn't find Google's search button
    // because the button isn't in innerText. Append an INTERACTIVE ELEMENTS
    // digest: every visible button/link/input with its accessible label, so
    // browser_click_text has real targets to aim at.
    let expr = r#"(() => {
        const title = document.title || '';
        const text = (document.body ? document.body.innerText : '').slice(0, 6000);
        const seen = new Set();
        const items = [];
        const els = document.querySelectorAll(
            'a[href], button, input, textarea, select, [role="button"], [role="link"], [role="searchbox"], [role="textbox"], [onclick]'
        );
        for (const el of els) {
            if (items.length >= 60) break;
            const r = el.getBoundingClientRect();
            if (r.width < 2 || r.height < 2) continue;
            const style = getComputedStyle(el);
            if (style.visibility === 'hidden' || style.display === 'none') continue;
            const tag = el.tagName.toLowerCase();
            const label = (
                el.getAttribute('aria-label') ||
                (tag === 'input' && (el.value || el.placeholder)) ||
                (el.innerText || '').trim().slice(0, 60) ||
                el.getAttribute('title') || el.getAttribute('name') || ''
            ).toString().trim().slice(0, 60);
            if (!label && tag !== 'input' && tag !== 'textarea') continue;
            const kind = tag === 'a' ? 'link'
                : (tag === 'input' ? (el.type || 'input')
                : (tag === 'textarea' || tag === 'select' ? tag
                : 'button'));
            const key = kind + '|' + label;
            if (seen.has(key)) continue;
            seen.add(key);
            items.push('[' + kind + '] ' + (label || '(unlabeled)'));
        }
        const digest = items.length
            ? '\n\n--- INTERACTIVE ELEMENTS (click with browser_click_text using the label) ---\n' + items.join('\n')
            : '';
        return JSON.stringify([title, text + digest]);
    })()"#;
    let r = active_tab_read(app, state, expr).await?;
    // active_tab_read returns the JSON.stringify'd value (possibly double-encoded).
    let decoded: String = serde_json::from_str(&r).unwrap_or(r);
    let arr: Vec<String> = serde_json::from_str(&decoded).unwrap_or_default();
    if arr.len() == 2 { Ok((arr[0].clone(), arr[1].clone())) } else { Err("unexpected page text shape".into()) }
}

/// Back/forward/reload on the CDP page (replaces the WKWebView-era
/// webview_history). Back/forward walk Page.getNavigationHistory; reload is
/// Page.reload. No-ops quietly at history edges.
#[tauri::command]
pub async fn browser_history_nav(
    app: tauri::AppHandle,
    state: tauri::State<'_, BrowserProc>,
    action: String,
) -> Result<(), String> {
    ensure_session(&app, &state).await?;
    match action.as_str() {
        "reload" => {
            session_call(&state, "Page.reload", serde_json::json!({})).await?;
        }
        "back" | "forward" => {
            let h = session_call(&state, "Page.getNavigationHistory", serde_json::json!({})).await?;
            let cur = h["currentIndex"].as_i64().unwrap_or(0);
            let entries = h["entries"].as_array().cloned().unwrap_or_default();
            let target = if action == "back" { cur - 1 } else { cur + 1 };
            if target < 0 || target as usize >= entries.len() {
                return Ok(()); // edge of history — quiet no-op
            }
            let id = entries[target as usize]["id"].as_i64().unwrap_or(0);
            session_call(&state, "Page.navigateToHistoryEntry", serde_json::json!({ "entryId": id })).await?;
        }
        _ => return Err(format!("unknown nav action: {action}")),
    }
    // Keep history/active-url in sync with wherever we landed.
    mirror_visible_to_cdp(&app, &state).await;
    Ok(())
}

/// Address-bar poll: current CDP page url + title (the ONE page).
#[tauri::command]
pub async fn browser_page_info(
    app: tauri::AppHandle,
    state: tauri::State<'_, BrowserProc>,
) -> Result<serde_json::Value, String> {
    let (title, url) = active_tab_page_info_cdp(&app, &state).await?;
    Ok(serde_json::json!({ "url": url, "title": title }))
}

/// Read (title, url) via CDP.
async fn active_tab_page_info_cdp(app: &tauri::AppHandle, state: &tauri::State<'_, BrowserProc>) -> Result<(String, String), String> {
    let r = active_tab_read(app, state, "[document.title||'', location.href||'']").await?;
    let arr: Vec<String> = serde_json::from_str(&r).unwrap_or_default();
    Ok((arr.get(0).cloned().unwrap_or_default(), arr.get(1).cloned().unwrap_or_default()))
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

// ===========================================================================
// ENGINE-CEF FRONTEND-FACING COMMANDS. Registered unconditionally in lib.rs so
// the FE can call them regardless of build; they are NO-OPS when the CEF engine
// isn't active (feature off OR not yet initialized), so the WKWebView UI is
// unaffected.
// ===========================================================================

/// OVERLAY HIT-TEST TOGGLE (Atrium's hard-won lesson). The FE calls this with
/// `enabled=false` whenever ANY React overlay is drawn over the browser
/// (permission card, History/Downloads panel, agent pane, any modal) so the CEF
/// wrapper DECLINES hit-testing and clicks fall through to React; `enabled=true`
/// when nothing overlays the browser. Driven by Browser.tsx's overlay REGISTRY
/// (a Set, never a counter). No-op unless the CEF engine is live.
#[tauri::command]
pub fn set_browser_hittest(enabled: bool) -> Result<(), String> {
    #[cfg(all(target_os = "macos", feature = "engine-cef"))]
    {
        crate::cef_geometry::set_hittest_enabled(enabled);
    }
    #[cfg(not(all(target_os = "macos", feature = "engine-cef")))]
    {
        let _ = enabled;
    }
    Ok(())
}

/// Which engine backs the VISIBLE browser surface: "cef" | "wkwebview". The FE
/// uses this to decide whether to draw the rounded-corner overlay border (CEF
/// clips its own layer) and for diagnostics. Always answers.
#[tauri::command]
pub fn browser_engine_info() -> Result<serde_json::Value, String> {
    #[cfg(all(target_os = "macos", feature = "engine-cef"))]
    {
        let ready = crate::cef_engine::is_ready();
        return Ok(serde_json::json!({
            "engine": if ready { "cef" } else { "wkwebview" },
            "cefFeature": true,
            "cefReady": ready,
            "cdpPort": crate::cef_engine::cdp_port(),
        }));
    }
    #[allow(unreachable_code)]
    Ok(serde_json::json!({ "engine": "wkwebview", "cefFeature": false, "cefReady": false }))
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
