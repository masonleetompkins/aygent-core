// AYGENT — Broker WS server (M0.2b, Atlas C1/C2).
// The PRIVILEGED side hosts this; the jailed daemon connects as an authed
// client. This is the crossing of the jail boundary: the daemon can only ASK
// (op: read|write|list|stat|resolve), never reach the filesystem itself.
//
// Trust model (BROKER-RPC-DECISION.md): separate WS from the UI channel.
// loopback + ephemeral port + per-session token. The daemon must send the
// token in its first frame or it's dropped.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::broker::{Broker, Mode};
use crate::exec::ExecBroker;

/// Start the broker WS server on loopback:0 (ephemeral). Returns the bound
/// port; serves for the app lifetime on the tokio runtime. The exec broker is
/// passed in so Pro-Mode `exec.*` ops resolve against the same privileged actor.
pub async fn start(
    broker: Arc<Broker>,
    exec_broker: Arc<ExecBroker>,
    token: String,
) -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    eprintln!("[aygent] broker-ws listening 127.0.0.1:{port}");

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let broker = broker.clone();
            let exec_broker = exec_broker.clone();
            let token = token.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_conn(stream, broker, exec_broker, token).await {
                    eprintln!("[aygent] broker-ws conn ended: {e}");
                }
            });
        }
    });

    Ok(port)
}

async fn handle_conn(
    stream: tokio::net::TcpStream,
    broker: Arc<Broker>,
    exec_broker: Arc<ExecBroker>,
    token: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut tx, mut rx) = ws.split();

    // First frame MUST be the auth token (Atlas C6 — localhost is not authz).
    let mut authed = false;
    // PRO MODE: the caps the daemon session declared at auth. The daemon can NOT
    // self-assert shell.exec per-call — it's bound ONCE here at connect, and the
    // broker enforces it. A compromised daemon still can't spawn without it.
    let mut granted_exec = false;

    while let Some(msg) = rx.next().await {
        let msg = msg?;
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if !authed {
            let ok = v.get("type").and_then(|t| t.as_str()) == Some("auth")
                && v.get("token").and_then(|t| t.as_str()) == Some(token.as_str());
            if !ok {
                let _ = tx.send(Message::Text(r#"{"type":"auth:err"}"#.into())).await;
                break; // fail closed
            }
            authed = true;
            // Bind the session's exec grant AT AUTH TIME. The daemon declares
            // whether the active agent holds shell.exec (Allow Shell Access). The broker
            // records it here and refuses every exec.* if it's false — the daemon
            // cannot flip it mid-session.
            granted_exec = v.get("caps")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().any(|c| c.as_str() == Some("shell.exec")))
                .unwrap_or(false);
            tx.send(Message::Text(r#"{"type":"auth:ok"}"#.into())).await?;
            continue;
        }

        // Authed: handle a broker op. Correlate by the client-provided id.
        if v.get("type").and_then(|t| t.as_str()) == Some("broker") {
            let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
            let reply = handle_op(&broker, &v);
            let mut obj = reply;
            obj["type"] = serde_json::Value::String("broker:reply".into());
            obj["id"] = id;
            tx.send(Message::Text(obj.to_string().into())).await?;
        }

        // PRO MODE: exec ops (spawn/run/poll/write/kill/wait/list). Same reply
        // envelope + id correlation as broker ops. Cap-gated: refused unless the
        // session was granted shell.exec at auth.
        if v.get("type").and_then(|t| t.as_str()) == Some("exec") {
            let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
            let reply = if !granted_exec {
                serde_json::json!({
                    "ok": false,
                    "error": "shell.exec not granted — enable Allow Shell Access for this agent"
                })
            } else {
                handle_exec(&broker, &exec_broker, &v)
            };
            let mut obj = reply;
            obj["type"] = serde_json::Value::String("exec:reply".into());
            obj["id"] = id;
            tx.send(Message::Text(obj.to_string().into())).await?;
        }
    }
    Ok(())
}

/// PRO MODE: dispatch one exec.* op. The cwd is ALWAYS resolved to the agent's
/// scoped root via broker.root_for (the same fail-closed/stale-bookmark rule as
/// files) — the daemon can NOT choose an arbitrary cwd. shell.exec is already
/// verified granted by the caller before we get here.
fn handle_exec(
    broker: &Arc<Broker>,
    exec_broker: &Arc<ExecBroker>,
    v: &serde_json::Value,
) -> serde_json::Value {
    let op = v.get("op").and_then(|o| o.as_str()).unwrap_or("");
    let agent = v.get("agentId").and_then(|a| a.as_str()).unwrap_or("default");

    // Resolve the jailed root for cwd-pinning. Ops that address an existing
    // process by handle (poll/write/kill/wait/list) don't need a root.
    let needs_root = matches!(op, "spawn" | "run");
    let root = if needs_root {
        match broker.root_for(agent) {
            Ok(r) => Some(r),
            Err(e) => {
                return serde_json::json!({
                    "ok": false,
                    "error": format!("no agent folder: {e:?}")
                })
            }
        }
    } else {
        None
    };

    let program = v.get("program").and_then(|p| p.as_str()).unwrap_or("");
    let args: Vec<String> = v
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let handle = v.get("proc_handle").and_then(|h| h.as_str()).unwrap_or("");

    let agent_opt = if agent != "default" && !agent.is_empty() { Some(agent) } else { None };
    let result = match op {
        "spawn" => exec_broker.spawn_for_agent(root.as_ref().unwrap(), agent_opt, program, &args),
        "run" => {
            let timeout_ms = v.get("timeout_ms").and_then(|t| t.as_u64()).unwrap_or(120_000);
            exec_broker.run_for_agent(root.as_ref().unwrap(), agent_opt, program, &args, timeout_ms)
        }
        "poll" => {
            let cursor = v.get("cursor").and_then(|c| c.as_u64()).unwrap_or(0);
            let tail_only = v.get("tail_only").and_then(|t| t.as_bool()).unwrap_or(false);
            exec_broker.poll(handle, cursor, tail_only)
        }
        "write" => {
            let data = v.get("data").and_then(|d| d.as_str()).unwrap_or("");
            exec_broker.write_stdin(handle, data)
        }
        "kill" => {
            let signal = v.get("signal").and_then(|s| s.as_str()).unwrap_or("TERM");
            exec_broker.kill(handle, signal)
        }
        "wait" => {
            let timeout_ms = v.get("timeout_ms").and_then(|t| t.as_u64()).unwrap_or(120_000);
            exec_broker.wait(handle, timeout_ms)
        }
        "list" => Ok(exec_broker.list()),
        _ => return serde_json::json!({ "ok": false, "error": "unknown exec op" }),
    };

    match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

/// Execute one broker op via the proven Broker::resolve, then do the actual
/// fs read/write on the ADMITTED canonical path. (The daemon never sees paths;
/// it gets content. openat/O_NOFOLLOW fd hardening is the next sub-step.)
fn handle_op(broker: &Arc<Broker>, v: &serde_json::Value) -> serde_json::Value {
    let op = v.get("op").and_then(|o| o.as_str()).unwrap_or("");
    let agent = v.get("agentId").and_then(|a| a.as_str()).unwrap_or("default");
    let path = v.get("path").and_then(|p| p.as_str()).unwrap_or("");

    macro_rules! refuse {
        ($e:expr) => {
            serde_json::json!({ "ok": false, "error": format!("{:?}", $e) })
        };
    }

    match op {
        "resolve" => match broker.resolve(agent, path, Mode::Read) {
            Ok(_) => serde_json::json!({ "ok": true }),
            Err(e) => refuse!(e),
        },
        // NOTE: broker_ws still uses resolve() + std::fs for now; the atomic
        // resolve_and_open path (M0.2c) is exercised by the agent loop in lib.rs.
        "read" => match broker.resolve_and_open(agent, path, Mode::Read) {
            // ATOMIC (M0.2c): opened with O_NOFOLLOW in the same step as the
            // check — no TOCTOU window. We read from the fd, never re-open a path.
            Ok(mut f) => {
                use std::io::Read;
                let mut content = String::new();
                match f.read_to_string(&mut content) {
                    Ok(_) => serde_json::json!({ "ok": true, "content": content }),
                    Err(e) => serde_json::json!({ "ok": false, "error": format!("io: {e}") }),
                }
            }
            Err(e) => refuse!(e),
        },
        "write" => {
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            // DIAG (Mason 08-06 "refused by jail on large writes / long replies
            // vanish"): log every write's path + byte length so a refusal or a
            // silent truncation is diagnosable from the terminal, not guessed.
            eprintln!("[aygent][broker][write] agent={agent} path={path:?} bytes={}", content.len());
            // Ensure parent dirs exist (resolve validates the parent is in-scope
            // via the resolution logic before we create anything).
            if let Ok(real) = broker.resolve(agent, path, Mode::Write) {
                if let Some(parent) = real.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
            // ATOMIC WRITE (Mason 08-06 — "long replies vanish / refused by jail
            // on large writes"). Two failure modes killed the old direct
            // O_TRUNC-then-write_all approach:
            //   (1) a partial write_all (broken pipe / disk pressure on a big
            //       buffer) left the file TRUNCATED — the reply "vanished".
            //   (2) rewriting an existing file with nlink > 1 (APFS clone, an
            //       editor safe-save, git object churn from Save Points) tripped
            //       the hardlink guard → "refused by jail".
            // Fix: write to a FRESH sibling temp path (always nlink == 1, so the
            // guard never fires), fsync it, then rename OVER the target. rename
            // is atomic and never truncates an aliased inode, so a failed write
            // can't destroy prior content and a legit hardlinked file isn't
            // falsely refused. Temp AND final paths resolve THROUGH THE BROKER,
            // so the jail governs every byte.
            let tmp_rel = format!("{path}.aygent-tmp-{}",
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos()).unwrap_or(0));
            let write_result: Result<(), serde_json::Value> = (|| {
                let mut f = broker.resolve_and_open(agent, &tmp_rel, Mode::Write).map_err(|e| {
                    eprintln!("[aygent][broker][write] REFUSED tmp for {path:?}: {e:?}");
                    refuse!(e)
                })?;
                use std::io::Write as _;
                f.write_all(content.as_bytes()).and_then(|_| f.flush()).map_err(|e| {
                    eprintln!("[aygent][broker][write] io error on {path:?}: {e}");
                    if let Ok(t) = broker.resolve(agent, &tmp_rel, Mode::Write) { let _ = std::fs::remove_file(&t); }
                    serde_json::json!({ "ok": false, "error": format!("io: {e}") })
                })?;
                let _ = f.sync_all();
                drop(f);
                let t = broker.resolve(agent, &tmp_rel, Mode::Write).map_err(|e| refuse!(e))?;
                let d = broker.resolve(agent, path, Mode::Write).map_err(|e| {
                    eprintln!("[aygent][broker][write] REFUSED dst {path:?}: {e:?}");
                    let _ = std::fs::remove_file(&t);
                    refuse!(e)
                })?;
                std::fs::rename(&t, &d).map_err(|e| {
                    eprintln!("[aygent][broker][write] rename {path:?} failed: {e}");
                    let _ = std::fs::remove_file(&t);
                    serde_json::json!({ "ok": false, "error": format!("io: {e}") })
                })?;
                Ok(())
            })();
            match write_result {
                Ok(()) => serde_json::json!({ "ok": true, "bytes": content.len() }),
                Err(err_json) => err_json,
            }
        }
        // @shared discovery: a bare "@shared" (or "@shared/") lists the mount
        // LABELS the agent can read — the entry point for browsing shared
        // context. Deeper "@shared/<label>/..." paths fall through to the normal
        // resolve() below, which maps them into the mount root.
        "list" if path == crate::broker::SHARED_ROOT || path == "@shared/" => {
            let entries: Vec<serde_json::Value> = broker
                .shared_labels_for(agent)
                .into_iter()
                .map(|label| serde_json::json!({ "name": label, "kind": "dir" }))
                .collect();
            serde_json::json!({ "ok": true, "entries": entries })
        }
        "list" => match broker.resolve(agent, path, Mode::Read) {
            Ok(real) => match std::fs::read_dir(&real) {
                Ok(rd) => {
                    let entries: Vec<serde_json::Value> = rd
                        .filter_map(|e| e.ok())
                        .map(|e| {
                            let kind = if e.path().is_dir() { "dir" } else { "file" };
                            serde_json::json!({ "name": e.file_name().to_string_lossy(), "kind": kind })
                        })
                        .collect();
                    serde_json::json!({ "ok": true, "entries": entries })
                }
                Err(e) => serde_json::json!({ "ok": false, "error": format!("io: {e}") }),
            },
            Err(e) => refuse!(e),
        },
        "stat" => match broker.resolve(agent, path, Mode::Read) {
            Ok(real) => match std::fs::metadata(&real) {
                Ok(m) => serde_json::json!({ "ok": true, "stat": {
                    "size": m.len(),
                    "kind": if m.is_dir() { "dir" } else { "file" },
                    "mtimeMs": 0
                }}),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("io: {e}") }),
            },
            Err(e) => refuse!(e),
        },
        _ => serde_json::json!({ "ok": false, "error": "unknown op" }),
    }
}
