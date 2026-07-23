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

/// Info the Rust side hands the daemon (via env at spawn) so it can connect.
pub struct BrokerWsInfo {
    pub port: u16,
    pub token: String,
}

/// Start the broker WS server on loopback:0 (ephemeral). Returns the bound
/// port; serves for the app lifetime on the tokio runtime.
pub async fn start(broker: Arc<Broker>, token: String) -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    eprintln!("[aygent] broker-ws listening 127.0.0.1:{port}");

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let broker = broker.clone();
            let token = token.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_conn(stream, broker, token).await {
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
    token: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut tx, mut rx) = ws.split();

    // First frame MUST be the auth token (Atlas C6 — localhost is not authz).
    let mut authed = false;

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
    }
    Ok(())
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
        "read" => match broker.resolve(agent, path, Mode::Read) {
            Ok(real) => match std::fs::read_to_string(&real) {
                Ok(content) => serde_json::json!({ "ok": true, "content": content }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("io: {e}") }),
            },
            Err(e) => refuse!(e),
        },
        "write" => {
            let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
            match broker.resolve(agent, path, Mode::Write) {
                Ok(real) => {
                    if let Some(parent) = real.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&real, content) {
                        Ok(_) => serde_json::json!({ "ok": true }),
                        Err(e) => serde_json::json!({ "ok": false, "error": format!("io: {e}") }),
                    }
                }
                Err(e) => refuse!(e),
            }
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
