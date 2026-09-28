// AYGENT — MCP client (2026-08-06). Makes the long-reserved "mcp" capability
// real: AYGENT spawns an MCP server as a child process, speaks JSON-RPC 2.0 over
// its stdio, discovers its tools (tools/list), and routes agent tool calls
// (tools/call) to it. The platform seam — ANY MCP server works, Premiere first.
//
// WHY NOT THE EXEC BROKER: the exec broker returns a BOUNDED digest (tail +
// counts), which can't carry full JSON-RPC replies. MCP needs a real protocol
// client: a persistent child with piped stdio + a reader thread that frames
// messages by line and routes replies by id (the remote_rt pump pattern). The
// exec broker still runs the INSTALL steps (npm i -g); only the RUNNING server
// is managed here.
//
// Transport: MCP over stdio = newline-delimited JSON-RPC 2.0 on stdin/stdout;
// servers may log to stderr (we drain + log it).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// A running MCP server. All mutable state is behind Mutex so a single Arc is
/// shared across the agent loop + the reader thread (Child/ChildStdin aren't
/// Clone, so we never reconstruct — we mutate in place).
pub struct McpServer {
    #[allow(dead_code)] // identity metadata; read by future MCP mgmt UI
    pub key: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next_id: AtomicI64,
    pending: Arc<Mutex<HashMap<i64, Sender<serde_json::Value>>>>,
    tools: Mutex<Vec<serde_json::Value>>,
}

impl McpServer {
    fn request(&self, method: &str, params: serde_json::Value, timeout: Duration) -> Result<serde_json::Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let line = format!("{}\n", serde_json::to_string(&msg).map_err(|e| format!("encode {method}: {e}"))?);
        if let Err(e) = self.send_raw(&line) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let reply = rx.recv_timeout(timeout).map_err(|_| {
            self.pending.lock().unwrap().remove(&id);
            format!("MCP {method}: timed out after {}s", timeout.as_secs())
        })?;
        if let Some(err) = reply.get("error") {
            let m = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown MCP error");
            return Err(format!("MCP {method} error: {m}"));
        }
        Ok(reply.get("result").cloned().unwrap_or(serde_json::json!({})))
    }

    fn notify(&self, method: &str, params: serde_json::Value) -> Result<(), String> {
        let msg = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let line = format!("{}\n", serde_json::to_string(&msg).map_err(|e| format!("encode {method}: {e}"))?);
        self.send_raw(&line)
    }

    fn send_raw(&self, line: &str) -> Result<(), String> {
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(line.as_bytes()).map_err(|e| format!("write stdin: {e}"))?;
        stdin.flush().map_err(|e| format!("flush stdin: {e}"))?;
        Ok(())
    }

    /// The discovered MCP tool descriptors (name/description/inputSchema).
    pub fn tools(&self) -> Vec<serde_json::Value> {
        self.tools.lock().unwrap().clone()
    }

    /// Call an MCP tool; return (text, is_error) for the agent loop.
    pub fn call_tool(&self, name: &str, args: &serde_json::Value, timeout: Duration) -> (String, bool) {
        match self.request("tools/call", serde_json::json!({ "name": name, "arguments": args }), timeout) {
            Ok(result) => {
                let is_err = result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false);
                (mcp_content_to_text(&result), is_err)
            }
            Err(e) => (e, true),
        }
    }

    fn kill(&self) {
        if let Ok(mut c) = self.child.lock() { let _ = c.kill(); let _ = c.wait(); }
    }
}

fn mcp_content_to_text(result: &serde_json::Value) -> String {
    if let Some(arr) = result.get("content").and_then(|c| c.as_array()) {
        let mut out = String::new();
        for item in arr {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("text") => { if let Some(t) = item.get("text").and_then(|t| t.as_str()) { out.push_str(t); out.push('\n'); } }
                Some(other) => out.push_str(&format!("[{other} content]\n")),
                None => {}
            }
        }
        if !out.trim().is_empty() { return out.trim().to_string(); }
    }
    serde_json::to_string(result).unwrap_or_default()
}

// ── registry of running servers ──────────────────────────────────────────────

static SERVERS: OnceLock<Mutex<HashMap<String, Arc<McpServer>>>> = OnceLock::new();
fn servers() -> &'static Mutex<HashMap<String, Arc<McpServer>>> {
    SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn is_running(key: &str) -> bool { servers().lock().unwrap().contains_key(key) }
pub fn get(key: &str) -> Option<Arc<McpServer>> { servers().lock().unwrap().get(key).cloned() }

/// Spawn + handshake an MCP server, register it under `key`, return the handle.
/// Blocks until initialize + tools/list complete so the caller knows the tools.
pub fn start(
    key: &str,
    program: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&std::path::Path>,
) -> Result<Arc<McpServer>, String> {
    if let Some(s) = get(key) { return Ok(s); }

    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env { cmd.env(k, v); }
    if let Some(d) = cwd { cmd.current_dir(d); }
    let mut child = cmd.spawn().map_err(|e| format!("spawn MCP server `{program}`: {e}"))?;

    let stdin = child.stdin.take().ok_or("no stdin on MCP child")?;
    let stdout = child.stdout.take().ok_or("no stdout on MCP child")?;
    let stderr = child.stderr.take();
    let pending: Arc<Mutex<HashMap<i64, Sender<serde_json::Value>>>> = Arc::new(Mutex::new(HashMap::new()));

    // reader thread: route id-bearing replies to their waiter.
    {
        let pending = pending.clone();
        let key_owned = key.to_string();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let line = line.trim();
                if line.is_empty() { continue; }
                let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                if let Some(id) = val.get("id").and_then(|i| i.as_i64()) {
                    if let Some(tx) = pending.lock().unwrap().remove(&id) { let _ = tx.send(val); }
                }
            }
            eprintln!("[aygent][mcp:{key_owned}] reader ended (server closed stdout)");
        });
    }
    if let Some(stderr) = stderr {
        let key_owned = key.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() { eprintln!("[aygent][mcp:{key_owned}][stderr] {line}"); }
            }
        });
    }

    let server = Arc::new(McpServer {
        key: key.to_string(),
        child: Mutex::new(child),
        stdin: Mutex::new(stdin),
        next_id: AtomicI64::new(1),
        pending,
        tools: Mutex::new(Vec::new()),
    });

    // handshake
    let init_params = serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "clientInfo": { "name": "AYGENT", "version": "0.1" },
    });
    if let Err(e) = server.request("initialize", init_params, Duration::from_secs(30)) {
        server.kill();
        return Err(format!("MCP initialize failed: {e}"));
    }
    let _ = server.notify("notifications/initialized", serde_json::json!({}));
    let listed = match server.request("tools/list", serde_json::json!({}), Duration::from_secs(30)) {
        Ok(v) => v,
        Err(e) => { server.kill(); return Err(format!("MCP tools/list failed: {e}")); }
    };
    let tools = listed.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    eprintln!("[aygent][mcp:{key}] connected — {} tools", tools.len());
    *server.tools.lock().unwrap() = tools;

    servers().lock().unwrap().insert(key.to_string(), server.clone());
    Ok(server)
}

/// Stop + deregister a running MCP server.
pub fn stop(key: &str) {
    if let Some(server) = servers().lock().unwrap().remove(key) {
        server.kill();
        eprintln!("[aygent][mcp:{key}] stopped");
    }
}

/// Stop every running MCP server (app shutdown).
pub fn stop_all() {
    let keys: Vec<String> = servers().lock().unwrap().keys().cloned().collect();
    for k in keys { stop(&k); }
}
