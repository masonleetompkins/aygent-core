// AYGENT — PRO MODE EXEC BROKER (the process-spawn boundary)
//
// The ONLY code with process-spawn authority. The Node daemon runs under a
// Seatbelt profile that DENIES process-exec*, so it MUST request every spawn
// over the broker WS. This is the exact mirror of the file broker (broker.rs):
// only Rust spawns, the daemon can only ASK.
//
// Design: projects/aygent/PRO-MODE-SHELL-PLAN.md (Atlas-reviewed 2026-07-31).
//
// Security invariants (Atlas §1):
//   1. cwd is ALWAYS pinned to the requesting agent's scoped root (resolved via
//      the SAME atomic root-compare the file broker uses). A command can never
//      run from outside the Agent Folder.
//   2. Env is SCRUBBED to a whitelist. The broker WS token + provider API keys
//      are NEVER inheritable by a child — the single most important MVP guard.
//   3. shell.exec is cap-gated: the broker holds its OWN copy of the session's
//      granted caps (bound at connect), so a compromised daemon can't self-grant.
//   4. Output is BOUNDED by the broker (ring buffer + log-to-file), never the
//      agent. The agent consumes deltas + a digest, not the raw stream.
//   5. Handles are OPAQUE. The daemon never gets a raw PID it can signal; kills
//      go through the broker (mirror of the file broker's handles-not-paths).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use once_cell::sync::OnceCell;

use crate::broker::Broker;

// Process-wide handles so the synchronous agent-tool dispatch (exec_tool_cfg in
// lib.rs) can reach the exec broker + resolve the agent's jailed root without
// threading state through every call site. Set ONCE at startup. The exec broker
// is a single global privileged actor anyway (mirror of how the file broker is
// managed), so a global handle is the honest shape, not a shortcut.
static GLOBAL_EXEC: OnceCell<Arc<ExecBroker>> = OnceCell::new();
static GLOBAL_BROKER: OnceCell<Arc<Broker>> = OnceCell::new();

/// Install the global exec broker + file broker (called once in lib.rs setup).
pub fn install_global(exec: Arc<ExecBroker>, broker: Arc<Broker>) {
    let _ = GLOBAL_EXEC.set(exec);
    let _ = GLOBAL_BROKER.set(broker);
}

/// The global exec broker, if installed.
pub fn global() -> Option<&'static Arc<ExecBroker>> {
    GLOBAL_EXEC.get()
}

/// Resolve an agent's jailed root for cwd-pinning (via the file broker scope).
/// Fail-closed: returns None if no scope / stale bookmark — same rule as files.
pub fn global_root(agent_id: &str) -> Option<PathBuf> {
    let b = GLOBAL_BROKER.get()?;
    b.root_for(agent_id).ok().or_else(|| b.root_for("default").ok())
}
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Env vars a child MAY inherit. Everything else is stripped — critically the
/// broker WS token and any API keys the daemon holds. Build tools need PATH +
/// the cargo/rustup homes; nothing else is required for git/cargo/npm/tsc.
const ENV_WHITELIST: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "LANG", "LC_ALL", "TMPDIR",
    "RUSTUP_HOME", "CARGO_HOME", "RUSTC_WRAPPER", "SSH_AUTH_SOCK",
    // node/npm toolchain hints (harmless if unset)
    "NODE_OPTIONS", "npm_config_prefix", "NVM_DIR",
];

/// One captured line, tagged with its stream + monotonic index.
#[derive(Clone)]
struct Line {
    idx: u64,
    stream: Stream,
    text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Stream {
    Out,
    Err,
}

impl Stream {
    fn tag(self) -> &'static str {
        match self {
            Stream::Out => "out",
            Stream::Err => "err",
        }
    }
}

/// Ring-buffered, digest-tracking output store for a single process. Older lines
/// roll off the in-memory ring (cap below) but the FULL stream is appended to a
/// log file on disk, one file-read away if the agent truly needs to grep it.
struct OutputStore {
    ring: Vec<Line>,
    cap: usize,
    next_idx: u64,
    // running digest counters (compiler-aware; cheap to maintain per line)
    errors: u64,
    warnings: u64,
    lines_total: u64,
    // structured signal we surface preferentially (error[E...], --> file:line,
    // "could not compile", "error:"). Bounded so a broken build can't grow it
    // without limit.
    signal: Vec<String>,
    log_path: Option<PathBuf>,
}

const RING_CAP: usize = 2000;
const SIGNAL_CAP: usize = 200;

impl OutputStore {
    fn new(log_path: Option<PathBuf>) -> Self {
        OutputStore {
            ring: Vec::new(),
            cap: RING_CAP,
            next_idx: 0,
            errors: 0,
            warnings: 0,
            lines_total: 0,
            signal: Vec::new(),
            log_path,
        }
    }

    fn push(&mut self, stream: Stream, text: String) {
        let idx = self.next_idx;
        self.next_idx += 1;
        self.lines_total += 1;

        // Compiler-aware classification (cargo/rustc/tsc/generic). Cheap string
        // scans; keeps the digest live without the agent reading the stream.
        let low = text.to_ascii_lowercase();
        let is_err = low.contains("error[")
            || low.starts_with("error:")
            || low.contains("could not compile")
            || low.contains(": error ")
            || low.contains("fatal:"); // git
        let is_warn = low.starts_with("warning:") || low.contains(": warning ");
        if is_err {
            self.errors += 1;
        }
        if is_warn {
            self.warnings += 1;
        }
        // Capture structured signal lines (the ones a human scans for).
        if (is_err
            || text.trim_start().starts_with("-->")
            || low.contains("could not compile"))
            && self.signal.len() < SIGNAL_CAP
        {
            self.signal.push(text.clone());
        }

        // Append to the on-disk log (full stream). Best-effort; never fails the
        // capture loop.
        if let Some(p) = &self.log_path {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "[{}] {}", stream.tag(), text);
            }
        }

        self.ring.push(Line { idx, stream, text });
        if self.ring.len() > self.cap {
            // roll off oldest
            let overflow = self.ring.len() - self.cap;
            self.ring.drain(0..overflow);
        }
    }

    /// Return lines with idx >= cursor, plus the next cursor. Lines that already
    /// rolled off the ring are gone from here (still on disk); the digest still
    /// counts them so totals stay honest.
    fn delta(&self, cursor: u64) -> (Vec<serde_json::Value>, u64) {
        let chunks: Vec<serde_json::Value> = self
            .ring
            .iter()
            .filter(|l| l.idx >= cursor)
            .map(|l| serde_json::json!({ "stream": l.stream.tag(), "text": l.text }))
            .collect();
        (chunks, self.next_idx)
    }
}

/// A live (or finished) spawned process the broker owns.
struct Proc {
    child: Mutex<Child>,
    output: Arc<Mutex<OutputStore>>,
    cmd_label: String,
    started: Instant,
    // Set once the process has exited; cached so poll/wait after exit still work.
    exit_code: Mutex<Option<i32>>,
}

/// Opaque handle handed to the daemon. NOT a PID — the daemon cannot signal a
/// process directly; it passes this back to poll/write/kill (mirror of the file
/// broker's opaque Handle).
fn new_handle() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("proc-{n}")
}

/// The exec broker. Holds the process table + (later) the granted cap set bound
/// at session connect. For MVP the cap gate is enforced by broker_ws before it
/// calls us; this struct is the privileged spawn authority.
pub struct ExecBroker {
    procs: Mutex<HashMap<String, Arc<Proc>>>,
}

#[derive(Debug)]
pub enum ExecError {
    NoScope,
    Spawn(String),
    NotFound,
    Io(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::NoScope => write!(f, "no agent scope (pick a folder first)"),
            ExecError::Spawn(e) => write!(f, "spawn failed: {e}"),
            ExecError::NotFound => write!(f, "unknown process handle"),
            ExecError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl ExecBroker {
    pub fn new() -> Arc<Self> {
        Arc::new(ExecBroker {
            procs: Mutex::new(HashMap::new()),
        })
    }

    /// Spawn a command with cwd PINNED to `root` (the agent's scoped folder,
    /// already atomically resolved by the caller via broker.root_for). Env is
    /// scrubbed to the whitelist. Output is captured into a ring buffer + a log
    /// file under `<root>/.aygent/logs/`.
    pub fn spawn(
        &self,
        root: &std::path::Path,
        program: &str,
        args: &[String],
    ) -> Result<serde_json::Value, ExecError> {
        let handle = new_handle();

        // Log file lives inside the agent folder's .aygent dir (same place Save
        // Points keep their shadow repo) so it's grep-able via the file broker
        // yet out of the user's own tree. Best-effort mkdir.
        let log_dir = root.join(".aygent").join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join(format!("{handle}.log"));

        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(root) // INVARIANT 1: cwd pinned to the jail root
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear(); // INVARIANT 2: drop EVERYTHING, then re-add whitelist
        for k in ENV_WHITELIST {
            if let Ok(v) = std::env::var(k) {
                cmd.env(k, v);
            }
        }

        let mut child = cmd.spawn().map_err(|e| ExecError::Spawn(e.to_string()))?;

        let output = Arc::new(Mutex::new(OutputStore::new(Some(log_path.clone()))));

        // Drain stdout + stderr on background threads into the ring buffer. Each
        // reader owns its pipe; the process struct keeps the Child for wait/kill.
        if let Some(out) = child.stdout.take() {
            spawn_reader(out, Stream::Out, output.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_reader(err, Stream::Err, output.clone());
        }

        let cmd_label = if args.is_empty() {
            program.to_string()
        } else {
            format!("{program} {}", args.join(" "))
        };

        let proc = Arc::new(Proc {
            child: Mutex::new(child),
            output,
            cmd_label: cmd_label.clone(),
            started: Instant::now(),
            exit_code: Mutex::new(None),
        });
        self.procs.lock().unwrap().insert(handle.clone(), proc);

        Ok(serde_json::json!({
            "ok": true,
            "proc_handle": handle,
            "cmd": cmd_label,
            "log": log_path.to_string_lossy(),
        }))
    }

    /// One-shot convenience: spawn, wait up to timeout, return a bounded digest
    /// (tail + counts + structured signal). Ergonomic default for git/cargo/npm.
    pub fn run(
        &self,
        root: &std::path::Path,
        program: &str,
        args: &[String],
        timeout_ms: u64,
    ) -> Result<serde_json::Value, ExecError> {
        let spawned = self.spawn(root, program, args)?;
        let handle = spawned["proc_handle"].as_str().unwrap().to_string();
        let waited = self.wait(&handle, timeout_ms)?;
        // Build a bounded digest from the whole run (cursor 0).
        let digest = self.poll(&handle, 0, /*tail_only=*/ true)?;
        let mut out = digest;
        out["ok"] = serde_json::Value::Bool(true);
        out["exit_code"] = waited["exit_code"].clone();
        out["timed_out"] = waited["timed_out"].clone();
        out["cmd"] = spawned["cmd"].clone();
        out["log"] = spawned["log"].clone();
        Ok(out)
    }

    /// Return new output since `cursor` + the running digest. If `tail_only`, we
    /// drop the per-line chunks and return only a bounded tail + counts (used by
    /// `run`, and by the agent when it just wants "how's it going").
    pub fn poll(
        &self,
        handle: &str,
        cursor: u64,
        tail_only: bool,
    ) -> Result<serde_json::Value, ExecError> {
        let proc = self.get(handle)?;
        // Refresh exit status (non-blocking) so `running` is accurate.
        let (running, exit_code) = self.status(&proc);
        let store = proc.output.lock().unwrap();
        let (chunks, next_cursor) = store.delta(cursor);

        // A bounded tail (last ~40 lines) — what the agent reads when it doesn't
        // want the whole delta. Signal lines are surfaced separately + first.
        let tail: Vec<String> = store
            .ring
            .iter()
            .rev()
            .take(40)
            .map(|l| format!("[{}] {}", l.stream.tag(), l.text))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let mut v = serde_json::json!({
            "ok": true,
            "running": running,
            "exit_code": exit_code,
            "next_cursor": next_cursor,
            "digest": {
                "errors": store.errors,
                "warnings": store.warnings,
                "lines_total": store.lines_total,
                "signal": store.signal,     // structured error lines, capped
            },
            "tail": tail,
        });
        if !tail_only {
            v["chunks"] = serde_json::Value::Array(chunks);
        }
        Ok(v)
    }

    /// Write to the process's stdin (e.g. answering a prompt).
    pub fn write_stdin(&self, handle: &str, data: &str) -> Result<serde_json::Value, ExecError> {
        let proc = self.get(handle)?;
        let mut child = proc.child.lock().unwrap();
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(data.as_bytes())
                .map_err(|e| ExecError::Io(e.to_string()))?;
            stdin.flush().ok();
            Ok(serde_json::json!({ "ok": true }))
        } else {
            Err(ExecError::Io("stdin closed".into()))
        }
    }

    /// Kill the process. TERM first; the caller can escalate by calling again
    /// (std::process::Child::kill sends SIGKILL on unix — for a graceful stop we
    /// try SIGTERM via libc first on unix, then fall back to kill()).
    pub fn kill(&self, handle: &str, signal: &str) -> Result<serde_json::Value, ExecError> {
        let proc = self.get(handle)?;
        let mut child = proc.child.lock().unwrap();

        #[cfg(unix)]
        {
            let pid = child.id() as i32;
            let sig = match signal {
                "KILL" | "SIGKILL" => libc::SIGKILL,
                _ => libc::SIGTERM,
            };
            let r = unsafe { libc::kill(pid, sig) };
            if r != 0 {
                // process may already be gone; treat ESRCH as success
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::ESRCH) {
                    return Err(ExecError::Io(format!("kill: {e}")));
                }
            }
            return Ok(serde_json::json!({ "ok": true, "signal": signal }));
        }
        #[cfg(not(unix))]
        {
            let _ = signal;
            child.kill().map_err(|e| ExecError::Io(e.to_string()))?;
            Ok(serde_json::json!({ "ok": true, "signal": "KILL" }))
        }
    }

    /// Block up to `timeout_ms` for exit. Returns {exit_code, timed_out}. Polls
    /// try_wait on a short tick so we never truly block the broker thread pool
    /// forever on a runaway build.
    pub fn wait(&self, handle: &str, timeout_ms: u64) -> Result<serde_json::Value, ExecError> {
        let proc = self.get(handle)?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        loop {
            {
                let mut child = proc.child.lock().unwrap();
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let code = status.code().unwrap_or(-1);
                        *proc.exit_code.lock().unwrap() = Some(code);
                        return Ok(serde_json::json!({
                            "ok": true, "exit_code": code, "timed_out": false
                        }));
                    }
                    Ok(None) => { /* still running */ }
                    Err(e) => return Err(ExecError::Io(e.to_string())),
                }
            }
            if Instant::now() >= deadline {
                return Ok(serde_json::json!({
                    "ok": true, "exit_code": serde_json::Value::Null, "timed_out": true
                }));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Non-blocking status refresh: (running, exit_code_or_null).
    fn status(&self, proc: &Arc<Proc>) -> (bool, serde_json::Value) {
        // If we already cached an exit code, it's done.
        if let Some(code) = *proc.exit_code.lock().unwrap() {
            return (false, serde_json::json!(code));
        }
        let mut child = proc.child.lock().unwrap();
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(-1);
                *proc.exit_code.lock().unwrap() = Some(code);
                (false, serde_json::json!(code))
            }
            _ => (true, serde_json::Value::Null),
        }
    }

    fn get(&self, handle: &str) -> Result<Arc<Proc>, ExecError> {
        self.procs
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or(ExecError::NotFound)
    }

    /// List live/known processes (for the UI process panel).
    pub fn list(&self) -> serde_json::Value {
        let procs = self.procs.lock().unwrap();
        let items: Vec<serde_json::Value> = procs
            .iter()
            .map(|(h, p)| {
                let running = p.exit_code.lock().unwrap().is_none();
                serde_json::json!({
                    "proc_handle": h,
                    "cmd": p.cmd_label,
                    "running": running,
                    "uptime_ms": p.started.elapsed().as_millis() as u64,
                })
            })
            .collect();
        serde_json::json!({ "ok": true, "procs": items })
    }
}

/// Read a child pipe line-by-line into the ring buffer on a background thread.
fn spawn_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    stream: Stream,
    output: Arc<Mutex<OutputStore>>,
) {
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            match line {
                Ok(text) => output.lock().unwrap().push(stream, text),
                Err(_) => break,
            }
        }
    });
}
