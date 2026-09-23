//! aygent_runner — headless Cloud Shell / Cloud Runs worker (1.1.0 M4).
//!
//! Single-shot: boot per run (Fly scale-to-zero), do the work, push the
//! snapshot, mark the run row, exit. Machine auto-stops.
//!
//! Env:
//!   RUN_ID, AGENT_ID (required)
//!   COMMAND (optional shell command; runs ONLY if ALLOW_SHELL=1)
//!   ALLOW_SHELL=1 enables COMMAND (mirrors the desktop Allow Shell Access gate)
//!   SNAPSHOT_URL (optional presigned GET .tar.gz), SNAPSHOT_PUT_URL (optional PUT)
//!   SUPABASE_URL + SUPABASE_SERVICE_KEY (optional: marks runs row ok/error)
//!
//! Cloud Shell v1 = files + shell. Full autonomous agent turns (drainer loop)
//! are M4b — the drainer still needs AppHandle decoupling.

use aygent_lib::broker::{Broker, Mode};
use aygent_lib::exec::ExecBroker;

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}

fn sh(cmd: &str, args: &[&str], cwd: &std::path::Path) -> Result<(), String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("spawn {cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{cmd} failed: {}", String::from_utf8_lossy(&out.stderr)))
    }
}

async fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent("aygent-runner/1.1.0")
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("get: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }
    resp.bytes()
        .await
        .map_err(|e| format!("body: {e}"))
        .map(|b| b.to_vec())
}

async fn http_put(url: &str, body: Vec<u8>) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("aygent-runner/1.1.0")
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp = client
        .put(url)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("put: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("upload failed: HTTP {}", resp.status()));
    }
    Ok(())
}

async fn mark_run(status: &str) {
    let (base, key, run) = (env("SUPABASE_URL"), env("SUPABASE_SERVICE_KEY"), env("RUN_ID"));
    if base.is_empty() || key.is_empty() || run.is_empty() {
        return;
    }
    let client = match reqwest::Client::builder()
        .user_agent("aygent-runner/1.1.0")
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let _ = client
        .patch(format!("{base}/rest/v1/runs?id=eq.{run}"))
        .header("apikey", &key)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .body(format!(
            "{{\"status\":\"{status}\",\"finished_at\":\"{}\"}}",
            chrono::Utc::now().to_rfc3339()
        ))
        .send()
        .await;
}

async fn run() -> Result<(), String> {
    let run_id = env("RUN_ID");
    let agent_id = env("AGENT_ID");
    if run_id.is_empty() || agent_id.is_empty() {
        return Err("RUN_ID and AGENT_ID are required".into());
    }
    let home = std::env::temp_dir().join(format!("aygent-job-{run_id}"));
    if home.exists() {
        std::fs::remove_dir_all(&home).map_err(|e| format!("clean workdir: {e}"))?;
    }
    std::fs::create_dir_all(&home).map_err(|e| format!("mkdir workdir: {e}"))?;
    let home = std::fs::canonicalize(&home).map_err(|e| format!("canonical workdir: {e}"))?; // /tmp is a symlink on macOS

    // 1. Snapshot in (.tar.gz) or fresh dir.
    let snap_url = env("SNAPSHOT_URL");
    if !snap_url.is_empty() {
        let bytes = http_get(&snap_url).await?;
        let tarball = home.join("in.tar.gz");
        std::fs::write(&tarball, bytes).map_err(|e| format!("write snapshot: {e}"))?;
        sh("tar", &["-xzf", "in.tar.gz"], &home)?;
        let _ = std::fs::remove_file(&tarball);
    }

    // 2. Jail the agent to the workdir (same broker as desktop).
    let broker = Broker::new();
    broker.set_scope(&agent_id, home.clone(), false);
    let exec = ExecBroker::new();

    // 3. Work: shell command if explicitly allowed, else proof-of-life file.
    let command = env("COMMAND");
    if env("ALLOW_SHELL") == "1" && !command.is_empty() {
        let out = exec.run(
            &home,
            "sh",
            &["-c".to_string(), command.clone()],
            120_000,
        )
        .map_err(|e| format!("shell: {e:?}"))?;
        let dest = broker
            .resolve(&agent_id, "RUN_RESULT.md", Mode::Write)
            .map_err(|e| format!("jail: {e:?}"))?;
        std::fs::write(&dest, format!("# run {run_id}\n\n```\n{out}\n```\n"))
            .map_err(|e| format!("write result: {e}"))?;
    } else {
        let dest = broker
            .resolve(&agent_id, "RUN_RESULT.md", Mode::Write)
            .map_err(|e| format!("jail: {e:?}"))?;
        std::fs::write(
            &dest,
            format!("# run {run_id}\n\nRunner alive. Shell disabled (ALLOW_SHELL!=1).\n"),
        )
        .map_err(|e| format!("write result: {e}"))?;
    }

    // 4. Snapshot out.
    let put_url = env("SNAPSHOT_PUT_URL");
    if !put_url.is_empty() {
        sh("tar", &["-czf", "out.tar.gz", "."], &home)?;
        let bytes =
            std::fs::read(home.join("out.tar.gz")).map_err(|e| format!("read snapshot: {e}"))?;
        http_put(&put_url, bytes).await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    match run().await {
        Ok(()) => {
            mark_run("ok").await;
            println!("run {} ok", env("RUN_ID"));
        }
        Err(e) => {
            mark_run("error").await;
            eprintln!("run {} error: {e}", env("RUN_ID"));
            std::process::exit(1);
        }
    }
}
