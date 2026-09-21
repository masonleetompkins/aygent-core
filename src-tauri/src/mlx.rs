//! MLX local runner (Apple Silicon only).
//!
//! llama.cpp covers GGUF weights in-process. This module covers the other half
//! of the local ecosystem — `mlx-community/*` safetensors weights — through a
//! uv-managed Python venv running `mlx_lm.server` (OpenAI-compatible `/v1` on
//! localhost). Everything lives under app-data `runtime/mlx` (venv + HF weight
//! cache via `HF_HOME`), so AYGENT still installs nothing outside the app.
//!
//! Scope: one model served at a time (switching kills + restarts the server),
//! chat completions with SSE streaming, pull-to-local-library with progress.
//! Turns run chat-only (same honest fallback as GGUF chat-only models).
//! No manual setup: the engine auto-installs on first pull/chat.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::provision::{ensure_uv, runtime_dir};
use crate::provider::StreamEvent;

/// Localhost port for the sidecar. Fixed in v1 (one model at a time).
pub const MLX_PORT: u16 = 18789;
struct ServerState {
    repo: String,
    child: std::process::Child,
}

static SERVER: Mutex<Option<ServerState>> = Mutex::new(None);

fn emit(app: &AppHandle, channel: &str, text: &str) {
    let _ = app.emit(channel, StreamEvent::Info { text: text.to_string() });
}

// ---- paths ---------------------------------------------------------------

/// `runtime/mlx` under app-data (venv + logs + models/ library + HF cache
/// fallback for repos served straight from HF).
pub fn mlx_home(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = runtime_dir(app)?.join("mlx");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir mlx: {e}"))?;
    Ok(dir)
}

fn venv_python(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(mlx_home(app)?.join("venv/bin/python"))
}

fn hf_cache(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = mlx_home(app)?.join("hf-cache");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir hf-cache: {e}"))?;
    Ok(dir)
}

fn server_log(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(mlx_home(app)?.join("server.log"))
}

/// True once the venv has a working `mlx_lm` import.
pub fn mlx_installed(app: &AppHandle) -> bool {
    let py = match venv_python(app) {
        Ok(p) => p,
        Err(_) => return false,
    };
    if !py.is_file() {
        return false;
    }
    std::process::Command::new(&py)
        .args(["-c", "import mlx_lm"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---- install ---------------------------------------------------------------

/// One-time setup: uv venv (Python 3.11) + `pip install mlx-lm`. Apple
/// Silicon only — MLX has no Intel/CUDA build, so anything else is a clean
/// error instead of a confusing pip failure.
pub async fn ensure_mlx(app: &AppHandle, channel: &str) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") || !cfg!(target_arch = "aarch64") {
        return Err("MLX models need Apple Silicon (this machine is not arm64 macOS)".into());
    }
    let home = mlx_home(app)?;
    let py = venv_python(app)?;
    if mlx_installed(app) {
        return Ok(py);
    }
    let uv = ensure_uv(app, channel).await?;
    let venv_dir = home.join("venv");
    if !py.is_file() {
        emit(app, channel, "mlx: creating Python 3.11 venv…");
        let out = std::process::Command::new(&uv)
            .args(["venv", "--python", "3.11"])
            .arg(&venv_dir)
            .output()
            .map_err(|e| format!("uv venv: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "uv venv failed: {}",
                String::from_utf8_lossy(&out.stderr).chars().take(500).collect::<String>()
            ));
        }
        let _ = std::process::Command::new("xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(&venv_dir)
            .output();
    }
    emit(app, channel, "mlx: installing mlx-lm (one-time, a few minutes)…");
    let out = std::process::Command::new(&uv)
        .args(["pip", "install", "--python"])
        .arg(&py)
        .arg("mlx-lm")
        .output()
        .map_err(|e| format!("uv pip: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pip install mlx-lm failed: {}",
            String::from_utf8_lossy(&out.stderr).chars().take(800).collect::<String>()
        ));
    }
    if !mlx_installed(app) {
        return Err("mlx-lm installed but `import mlx_lm` still fails — reinstall from Tools".into());
    }
    emit(app, channel, "mlx: ready");
    Ok(py)
}

// ---- server lifecycle --------------------------------------------------------

fn base_url() -> String {
    format!("http://127.0.0.1:{MLX_PORT}")
}

async fn healthy() -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client.get(format!("{}/v1/models", base_url())).send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

fn stop_locked(mut slot: std::sync::MutexGuard<'_, Option<ServerState>>) {
    if let Some(mut st) = slot.take() {
        let _ = st.child.kill();
        let _ = st.child.wait();
    }
}

/// Stop the sidecar (e.g. neuronal teardown, switching engines).
pub fn mlx_stop() {
    if let Ok(slot) = SERVER.lock() {
        stop_locked(slot);
    }
}

/// Ensure a server for `repo` is up; reuse the warm one when it matches.
/// First boot downloads weights (GBs) — readiness polls up to ~20 min with
/// progress notes on `channel`.
pub async fn ensure_server(
    app: &AppHandle,
    channel: &str,
    repo: &str,
) -> Result<String, String> {
    let repo = repo.trim().to_string();
    if repo.is_empty() || !repo.contains('/') {
        return Err("MLX model should be a Hugging Face repo id like mlx-community/Qwen3-4B-4bit".into());
    }
    // Warm reuse (lock is scoped: the guard never crosses an await, keeping
    // the caller future Send for Tauri).
    let warm: Option<String> = SERVER
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|st| st.repo.clone()));
    if warm.as_deref() == Some(&repo) && healthy().await {
        return Ok(base_url());
    }
    mlx_stop();
    let py = ensure_mlx(app, channel).await?;
    let cache = hf_cache(app)?;
    // Prefer a pulled copy (instant, offline); else the server auto-downloads
    // the repo into the HF cache on first boot.
    let target = serve_target(app, &repo);
    let log_path = server_log(app)?;
    let log = std::fs::File::create(&log_path).map_err(|e| format!("server log: {e}"))?;
    let err_log = log.try_clone().map_err(|e| format!("server log: {e}"))?;
    emit(app, channel, &format!("mlx: starting {repo} (first boot downloads weights)…"));
    let mut child = std::process::Command::new(&py)
        .args(["-m", "mlx_lm.server", "--model", &target, "--port", &MLX_PORT.to_string()])
        .env("HF_HOME", &cache)
        .env("HF_HUB_CACHE", &cache)
        .env("HF_HUB_OFFLINE", "0")
        .stdout(log)
        .stderr(err_log)
        .spawn()
        .map_err(|e| format!("launch mlx_lm.server: {e}"))?;
    // Early exit = bad repo / bind failure. Surface the log tail, not silence.
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Ok(None) = child.try_wait() {
        // still starting — hand over to the readiness poll below.
    } else {
        let tail = std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .chars()
            .rev()
            .take(600)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        let _ = child.wait();
        return Err(format!("mlx server exited at startup (bad repo id?): {tail}"));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20 * 60);
    let mut n = 0u32;
    while tokio::time::Instant::now() < deadline {
        if healthy().await {
            if let Ok(mut slot) = SERVER.lock() {
                *slot = Some(ServerState { repo: repo.clone(), child });
            } else {
                let _ = child.kill();
            }
            emit(app, channel, &format!("mlx: {repo} serving"));
            return Ok(base_url());
        }
        n += 1;
        if n % 6 == 0 {
            emit(app, channel, &format!("mlx: still loading {repo} ({:.0} min)…", n as f64 / 12.0));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let _ = child.kill();
    Err(format!("mlx server for {repo} never became ready (see {})", log_path.display()))
}

// ---- chat --------------------------------------------------------------------

/// Flatten AYGENT's Anthropic-block history into OpenAI `{role, content}`
/// messages. Tool-result blocks inline as user text (v1 chat path).
fn flatten_to_openai(history: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let arr = match history.as_array() {
        Some(a) => a,
        None => return out,
    };
    for m in arr {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let role = if role == "assistant" { "assistant" } else { "user" };
        let mut text = String::new();
        match m.get("content") {
            Some(serde_json::Value::String(s)) => text.push_str(s),
            Some(serde_json::Value::Array(blocks)) => {
                for b in blocks {
                    let t = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match t {
                        "text" => {
                            if let Some(s) = b.get("text").and_then(|s| s.as_str()) {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(s);
                            }
                        }
                        "tool_use" => {
                            text.push_str(&format!(
                                "\n[tool request: {} {}]",
                                b.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                                b.get("input").map(|v| v.to_string()).unwrap_or_default()
                            ));
                        }
                        "tool_result" => {
                            text.push_str(&format!(
                                "\n[tool result] {}",
                                b.get("content").map(|v| match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    v => v.to_string(),
                                }).unwrap_or_default()
                            ));
                        }
                        _ => {}
                    }
                }
            }
            _ => continue,
        }
        if text.trim().is_empty() {
            continue;
        }
        out.push(serde_json::json!({ "role": role, "content": text }));
    }
    out
}

/// One streamed chat turn against the sidecar. Returns Anthropic-shaped
/// content blocks (`[{type:text,text}]`) like `local_stream_turn`, so the
/// agent loop can share its history handling.
pub async fn mlx_stream_turn<F: FnMut(StreamEvent)>(
    app: &AppHandle,
    channel: &str,
    repo: &str,
    history: &serde_json::Value,
    max_tokens: u32,
    mut emit_ev: F,
) -> Result<serde_json::Value, String> {
    let base = ensure_server(app, channel, repo).await?;
    let messages = flatten_to_openai(history);
    if messages.is_empty() {
        return Err("mlx: nothing to send (empty history)".into());
    }
    let body = serde_json::json!({
        "model": serve_target(app, repo),
        "messages": messages,
        "stream": true,
        "max_tokens": max_tokens,
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|e| format!("mlx http: {e}"))?;
    let resp = client
        .post(format!("{base}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("mlx request: {e}"))?;
    if !resp.status().is_success() {
        let code = resp.status();
        let detail = resp.text().await.unwrap_or_default().chars().take(500).collect::<String>();
        return Err(format!("mlx server HTTP {code}: {detail}"));
    }
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut full = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("mlx stream: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..=pos).collect();
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(delta) = v
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|c| c.first())
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
            {
                if !delta.is_empty() {
                    full.push_str(delta);
                    emit_ev(StreamEvent::TextDelta { text: delta.to_string() });
                }
            }
        }
    }
    if full.trim().is_empty() {
        return Err("mlx returned no text (model may need a different prompt format)".into());
    }
    Ok(serde_json::json!([{ "type": "text", "text": full }]))
}

// ---- weights -------------------------------------------------------------------

/// In-app model library: `runtime/mlx/models/<author>/<name>/`.
fn mlx_models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = mlx_home(app)?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir mlx models: {e}"))?;
    Ok(dir)
}

/// Jailed path for a repo id. Rejects anything that isn't exactly
/// `author/name` (no traversal, no nesting) so pull/delete/serve can only
/// ever touch inside the library.
fn local_repo_dir(app: &AppHandle, repo: &str) -> Result<PathBuf, String> {
    let repo = repo.trim();
    let mut parts = repo.split('/');
    let ok_shape = matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(n), None) if !a.is_empty() && !n.is_empty());
    let ok_chars = repo.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/');
    if !ok_shape || !ok_chars || repo.contains("..") {
        return Err("repo id should be 'author/name' (e.g. mlx-community/Qwen3-4B-4bit)".into());
    }
    let mut it = repo.split('/');
    let (author, name) = (it.next().unwrap(), it.next().unwrap());
    Ok(mlx_models_dir(app)?.join(author).join(name))
}

/// True when a directory holds a servable model: config up top + at least one
/// weights file anywhere under it.
fn dir_has_weights(dir: &std::path::Path) -> bool {
    if !dir.join("config.json").is_file() { return false; }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p); continue; }
            if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("safetensors")).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p); continue; }
            total += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

/// What to hand `mlx_lm.server --model`: the pulled directory when it's
/// complete, else the repo id (the server auto-downloads into the HF cache).
fn serve_target(app: &AppHandle, repo: &str) -> String {
    local_repo_dir(app, repo)
        .ok()
        .filter(|d| dir_has_weights(d))
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_else(|| repo.trim().to_string())
}

/// Every complete pull in the library: `{ repo, path, size_gb }`.
fn pulled_models(app: &AppHandle) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(models) = mlx_models_dir(app) else { return out };
    let Ok(authors) = std::fs::read_dir(&models) else { return out };
    for a in authors.flatten() {
        if !a.path().is_dir() { continue; }
        let author = a.file_name().to_string_lossy().to_string();
        let Ok(repos) = std::fs::read_dir(a.path()) else { continue };
        for r in repos.flatten() {
            if !r.path().is_dir() { continue; }
            let name = r.file_name().to_string_lossy().to_string();
            if !dir_has_weights(&r.path()) { continue; }
            out.push(serde_json::json!({
                "repo": format!("{author}/{name}"),
                "path": r.path().to_string_lossy(),
                "size_gb": dir_size(&r.path()) as f64 / 1_073_741_824.0,
            }));
        }
    }
    out.sort_by(|a, b| a.get("repo").and_then(|r| r.as_str()).cmp(&b.get("repo").and_then(|r| r.as_str())));
    out
}

/// Pull a repo's full file set into the library, emitting per-file progress
/// on `channel` (`{file, index, files, got, total}`, then `{done, repo}`).
/// Completed files are skipped, so an interrupted pull resumes. The engine
/// auto-installs first — no manual step.
pub async fn mlx_pull(app: &AppHandle, channel: &str, repo: &str) -> Result<String, String> {
    use futures_util::StreamExt;
    use std::io::Write as _;
    let repo = repo.trim().to_string();
    let dest_top = local_repo_dir(app, &repo)?;
    ensure_mlx(app, channel).await?;
    let api = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .build()
        .map_err(|e| format!("mlx http: {e}"))?;
    let meta: serde_json::Value = api
        .get(format!("https://huggingface.co/api/models/{repo}"))
        .send()
        .await
        .map_err(|e| format!("mlx repo lookup: {e}"))?
        .error_for_status()
        .map_err(|e| format!("mlx repo lookup: {e}"))?
        .json()
        .await
        .map_err(|e| format!("mlx repo lookup: {e}"))?;
    let files: Vec<String> = meta
        .get("siblings")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.get("rfilename").and_then(|f| f.as_str()).map(str::to_string)).collect())
        .unwrap_or_default();
    if files.is_empty() {
        return Err(format!("no files listed for {repo}"));
    }
    std::fs::create_dir_all(&dest_top).map_err(|e| format!("mkdir: {e}"))?;
    let dl = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("mlx http: {e}"))?;
    let n = files.len();
    for (i, f) in files.iter().enumerate() {
        let dest = dest_top.join(f);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        if dest.is_file() && std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0) > 0 {
            continue; // resume: keep completed files
        }
        let tmp = dest.with_extension("part");
        let url = format!("https://huggingface.co/{repo}/resolve/main/{f}");
        let resp = dl
            .get(&url)
            .header("Accept", "*/*")
            .send()
            .await
            .map_err(|e| format!("download {f}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("download {f}: HTTP {}", resp.status()));
        }
        let total = resp.content_length().unwrap_or(0);
        let _ = app.emit(channel, &serde_json::json!({ "file": f, "index": i, "files": n, "got": 0u64, "total": total }));
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("create file: {e}"))?;
        let mut got: u64 = 0;
        let mut last: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| format!("download {f}: {e}"))?;
            file.write_all(&bytes).map_err(|e| format!("write: {e}"))?;
            got += bytes.len() as u64;
            if got - last > 2_000_000 {
                last = got;
                let _ = app.emit(channel, &serde_json::json!({ "file": f, "index": i, "files": n, "got": got, "total": total }));
            }
        }
        drop(file);
        std::fs::rename(&tmp, &dest).map_err(|e| format!("finalize {f}: {e}"))?;
        let _ = app.emit(channel, &serde_json::json!({ "file": f, "index": i, "files": n, "got": got, "total": total }));
    }
    if !dir_has_weights(&dest_top) {
        return Err(format!("{repo} pulled but no weights found — the repo may not be an MLX model"));
    }
    let _ = app.emit(channel, &serde_json::json!({ "done": true, "repo": repo }));
    Ok(repo)
}

// ---- tauri commands --------------------------------------------------------------

/// `{ installed, running, repo }` for Settings/Tools status surfaces.
#[tauri::command]
pub fn mlx_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let (running, repo) = match SERVER.lock() {
        Ok(slot) => match slot.as_ref() {
            Some(st) => (true, Some(st.repo.clone())),
            None => (false, None),
        },
        Err(_) => (false, None),
    };
    Ok(serde_json::json!({
        "installed": mlx_installed(&app),
        "running": running,
        "repo": repo,
        "apple_silicon": cfg!(target_os = "macos") && cfg!(target_arch = "aarch64"),
    }))
}

/// One-time (or repair) install of the venv + mlx-lm. Progress streams on
/// `channel` (mirrors hyperframes_install's contract).
#[tauri::command]
pub async fn mlx_install_cmd(app: AppHandle, channel: String) -> Result<String, String> {
    let py = ensure_mlx(&app, &channel).await?;
    Ok(py.to_string_lossy().into_owned())
}

/// Prefetch a repo's weights into the in-app cache. Progress on `channel`.
#[tauri::command]
pub async fn mlx_pull_cmd(app: AppHandle, channel: String, repo: String) -> Result<String, String> {
    mlx_pull(&app, &channel, &repo).await
}

/// Stop the sidecar (frees RAM). Serving restarts on the next MLX turn.
#[tauri::command]
pub fn mlx_stop_cmd() -> Result<bool, String> {
    mlx_stop();
    Ok(true)
}

/// Every complete pull in the library — the MLX half of the downloaded list.
#[tauri::command]
pub fn mlx_downloaded(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    Ok(pulled_models(&app))
}

/// Delete a pull by repo id (jailed to `author/name` — see local_repo_dir).
#[tauri::command]
pub fn mlx_delete_cmd(app: AppHandle, repo: String) -> Result<(), String> {
    let dir = local_repo_dir(&app, &repo)?;
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("delete: {e}"))?;
    }
    if let Some(parent) = dir.parent() {
        let _ = std::fs::remove_dir(parent); // prune empty author dir
    }
    Ok(())
}
