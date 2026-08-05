// AYGENT REMOTE — the device runtime (R4 glue).
//
// Owns the LIVE loop: rt client events → open/reassemble → dispatch WebMsg →
// engine → translate StreamEvents → seal → rt client. remote_bridge owns the
// pure logic (protocol, coalescing, dedupe); this module owns the tokio task
// and the AppHandle wiring, so the testable core stays AppHandle-free.
//
// LIFECYCLE: spawned at boot IF paired (remote::is_paired()), and (re)started
// by the Settings card after a successful pair. Shutdown on unpair/quit.
//
// TURN EXECUTION: a remote prompt runs through the SAME per-session lane +
// agent_stream-equivalent loop as a local turn, by emitting on the SAME
// conversation channel — a locally open pane watching that conversation sees
// the remote turn stream live (and vice versa: the phone sees what the desk
// started). The bridge listens on that channel and forwards.
//
// CAPABILITY (v1): remote turns get the agent's normal tool inventory MINUS
// Pro-Mode shell — shell_run/shell_spawn/etc are refused with a named reason
// unless the user enables "Allow shell from remote" in Settings (device-local
// gate, same one-gate discipline as connections).

use std::sync::Arc;
use tauri::{Emitter, Listener, Manager};

use crate::remote::{Reassembler, Sealer};
use crate::remote_bridge::{
    parse_web_msg, send_dev_msg, AgentInfo, Coalescer, DevMsg, TurnDedupe, WebMsg,
};
use crate::remote_rt::{spawn as rt_spawn, RtCommand, RtEvent};
use crate::{drainer, mailbox, repo, writer};

/// Control scope id for non-turn replies (agent lists, conv history, pong).
const CTL: &str = "ctl";

/// Managed state: lets Settings stop/restart the runtime and shows status.
#[derive(Default)]
pub struct RemoteRuntime {
    inner: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<RtCommand>>>,
}

impl RemoteRuntime {
    pub fn is_running(&self) -> bool {
        self.inner.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    pub fn shutdown(&self) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(tx) = g.take() {
                let _ = tx.try_send(RtCommand::Shutdown);
            }
        }
    }

    /// Shut down any prior session and install the new command sender.
    /// (Method, not inline locking: keeps the State borrow inside one call —
    /// a `State<T>` temporary + MutexGuard across statements trips E0597.)
    pub fn replace(&self, tx: tokio::sync::mpsc::Sender<RtCommand>) {
        self.shutdown();
        if let Ok(mut g) = self.inner.lock() {
            *g = Some(tx);
        }
    }
}

/// Keep trying to bring the session up until it succeeds (or unpaired).
/// WHY: at first-time pair the browser key does not exist yet — the browser
/// publishes it AFTER the Mac claims the code. A one-shot start at pair/boot
/// time therefore always misses; this loop closes that gap (15s cadence).
pub fn spawn_autostart(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if !crate::remote::is_paired() {
                break;
            }
            if app.state::<RemoteRuntime>().is_running() {
                break;
            }
            match start_if_paired(app.clone()).await {
                Ok(true) => {
                    eprintln!("[aygent][remote] realtime session up");
                    break;
                }
                Ok(false) => {} // paired, browser key not published yet — retry
                Err(e) => eprintln!("[aygent][remote] autostart retry: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        }
    });
}

/// Start the runtime if the device is paired AND the browser key exchange has
/// completed (we need the browser pubkey to build the Sealer). Safe to call
/// repeatedly — an already-running runtime is shut down and replaced.
pub async fn start_if_paired(app: tauri::AppHandle) -> Result<bool, String> {
    let Some(meta) = crate::remote::load_meta() else { return Ok(false) };
    let Some(jwt) = crate::remote::load_jwt() else { return Ok(false) };

    // Fetch the browser pubkey from the device row via the site (the browser
    // publishes it on first /remote load; until then we can't seal).
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http: {e}"))?;
    let resp: serde_json::Value = client
        .get(format!("{}/api/remote/device", meta.site))
        .bearer_auth(&jwt)
        .send()
        .await
        .map_err(|e| format!("device status: {e}"))?
        .json()
        .await
        .map_err(|e| format!("device status decode: {e}"))?;
    let browser_pub = resp
        .get("device")
        .and_then(|d| d.get("browser_pubkey"))
        .and_then(|k| k.as_str())
        .unwrap_or("")
        .to_string();
    if browser_pub.is_empty() {
        // Paired but no browser yet — not an error; Settings shows "waiting
        // for first browser connection." The runtime starts on next attempt.
        return Ok(false);
    }

    let sealer = Arc::new(Sealer::new(&browser_pub)?);

    // Supabase coordinates come from the site metadata (same project the site
    // uses; the anon key is public by definition).
    let supa_url = resp.get("supabase_url").and_then(|u| u.as_str()).unwrap_or("").to_string();
    let supa_anon = resp.get("supabase_anon").and_then(|k| k.as_str()).unwrap_or("").to_string();
    if supa_url.is_empty() || supa_anon.is_empty() {
        return Err("site did not return Supabase coordinates (deploy the aygent-remote branch)".into());
    }

    let (client, events) = rt_spawn(supa_url, supa_anon, meta.channel.clone(), jwt);

    // Replace any prior runtime.
    app.state::<RemoteRuntime>().replace(client.tx.clone());

    tokio::spawn(run_loop(app, client.tx, events, sealer));
    Ok(true)
}

/// The main dispatch loop. One instance per pairing session.
async fn run_loop(
    app: tauri::AppHandle,
    tx: tokio::sync::mpsc::Sender<RtCommand>,
    mut events: tokio::sync::mpsc::Receiver<RtEvent>,
    sealer: Arc<Sealer>,
) {
    let mut reasm = Reassembler::default();
    let mut dedupe = TurnDedupe::default();

    while let Some(evt) = events.recv().await {
        match evt {
            RtEvent::Connected => {
                let _ = app.emit("remote-status", &serde_json::json!({ "connected": true }));
                // Announce ourselves. The browser may have joined first and
                // already sent its one-shot hello into an empty channel; an
                // unsolicited hello on every (re)connect means whoever joins
                // last still completes the handshake.
                send_hello(&app, &tx, &sealer).await;
            }
            RtEvent::Disconnected { retry_in_secs } => {
                let _ = app.emit(
                    "remote-status",
                    &serde_json::json!({ "connected": false, "retry_in": retry_in_secs }),
                );
            }
            RtEvent::Inbound(env) => {
                // Only web-originated envelopes; our own sends echo back off.
                if env.from != "web" {
                    continue;
                }
                let plain = match sealer.open(&env) {
                    Ok(p) => p,
                    Err(_) => continue, // wrong key / tamper — drop silently
                };
                let Some(full) = reasm.feed(&env, plain) else { continue };
                let Some(msg) = parse_web_msg(&full) else { continue };
                handle_msg(&app, &tx, &sealer, &mut dedupe, msg).await;
            }
        }
    }
    let _ = app.emit("remote-status", &serde_json::json!({ "connected": false }));
}

/// Build + send the hello (device name + live agent roster).
async fn send_hello(
    app: &tauri::AppHandle,
    tx: &tokio::sync::mpsc::Sender<RtCommand>,
    sealer: &Arc<Sealer>,
) {
    let db = app.state::<writer::Db>();
    let agents = repo::list_agents(&db)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !a.archived)
        .map(|a| AgentInfo { id: a.id, name: a.name, icon: a.icon, color: a.color })
        .collect::<Vec<_>>();
    let device_name = hostname();
    let _ = send_dev_msg(sealer, tx, CTL, &DevMsg::Hello { device_name, agents }).await;
}

fn hostname() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "My Mac".to_string())
}

/// Dispatch one decrypted web message.
async fn handle_msg(
    app: &tauri::AppHandle,
    tx: &tokio::sync::mpsc::Sender<RtCommand>,
    sealer: &Arc<Sealer>,
    dedupe: &mut TurnDedupe,
    msg: WebMsg,
) {
    let db = app.state::<writer::Db>();
    match msg {
        WebMsg::Ping => {
            let _ = send_dev_msg(sealer, tx, CTL, &DevMsg::Pong).await;
        }
        WebMsg::Hello | WebMsg::ListAgents => {
            send_hello(app, tx, sealer).await;
        }
        WebMsg::ListConvs { agent } => {
            let convs = repo::list_conversations(&db, &agent)
                .map(|l| serde_json::to_value(l).unwrap_or_default())
                .unwrap_or_default();
            let _ = send_dev_msg(sealer, tx, CTL, &DevMsg::Convs { agent, convs }).await;
        }
        WebMsg::OpenConv { id } => {
            match repo::load_conversation(&db, &id) {
                Ok(c) => {
                    let m = DevMsg::ConvHistory { id: c.id, title: c.title, msgs: c.msgs };
                    let _ = send_dev_msg(sealer, tx, CTL, &m).await;
                }
                Err(e) => {
                    let _ = send_dev_msg(sealer, tx, CTL, &DevMsg::Error { msg: e }).await;
                }
            }
        }
        WebMsg::NewConv { agent } => {
            // Conversation ids are minted device-side, same shape the UI uses.
            let id = format!("{}-{}", now_millis(), &agent[..agent.len().min(6)]);
            let m = DevMsg::ConvHistory { id, title: "New chat".into(), msgs: serde_json::json!([]) };
            let _ = send_dev_msg(sealer, tx, CTL, &m).await;
        }
        WebMsg::Stop { turn } => {
            // The turn's cancel flag is registered under its conversation
            // channel; remote turns register under "remote:<turn>".
            let reg = app.state::<crate::cancel::CancelRegistry>();
            reg.request_stop(&format!("remote:{turn}"));
        }
        WebMsg::Prompt { turn, agent, conv, text } => {
            if !dedupe.accept(&turn) {
                return; // at-least-once replay — already running/ran
            }
            let _ = send_dev_msg(sealer, tx, &turn, &DevMsg::TurnStart { turn: turn.clone() }).await;
            run_remote_turn(app.clone(), tx.clone(), sealer.clone(), turn, agent, conv, text);
        }
    }
}

/// Run one remote-originated turn. Reuses the mailbox headless path: the
/// prompt is enqueued as an origin-tagged message and the drainer runs it on
/// the agent's own lane with its own provider/model/tools — the exact code
/// path scheduled + inter-agent turns already exercise. The bridge subscribes
/// to the conversation's stream channel and forwards translated events.
fn run_remote_turn(
    app: tauri::AppHandle,
    tx: tokio::sync::mpsc::Sender<RtCommand>,
    sealer: Arc<Sealer>,
    turn: String,
    agent: String,
    conv: String,
    text: String,
) {
    // 1) Subscribe to the conversation's stream channel BEFORE dispatch so no
    //    early event is missed. Forwarding runs on a small mpsc so the Tauri
    //    listener callback stays sync + cheap.
    let (fwd_tx, mut fwd_rx) = tokio::sync::mpsc::channel::<serde_json::Value>(256);
    let stream_channel = conv.clone();
    let listener_id = app.listen(stream_channel.clone(), move |ev| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(ev.payload()) {
            let _ = fwd_tx.try_send(v);
        }
    });

    // 2) Forwarder task: translate + coalesce + seal until Done/Error.
    let app2 = app.clone();
    let turn2 = turn.clone();
    tokio::spawn(async move {
        let mut co = Coalescer::new(&turn2);
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            tokio::select! {
                ev = fwd_rx.recv() => {
                    let Some(ev) = ev else { break };
                    let msgs = crate::remote_bridge::translate_event(&turn2, &ev, &mut co);
                    let ended = msgs.iter().any(|m| matches!(m, DevMsg::TurnEnd { .. } | DevMsg::Error { .. }));
                    for m in &msgs {
                        let _ = send_dev_msg(&sealer, &tx, &turn2, m).await;
                    }
                    if ended { break }
                }
                _ = tick.tick() => {
                    if let Some(m) = co.flush() {
                        let _ = send_dev_msg(&sealer, &tx, &turn2, &m).await;
                    }
                }
            }
        }
        app2.unlisten(listener_id);
    });

    // 3) Dispatch the turn through the mailbox (origin remote:<turn> — framed
    //    as a direct task, not a peer message; body carries the conv routing
    //    the same way task_continue does).
    let body = format!("conv:{conv}\n{text}");
    let db = app.state::<writer::Db>().inner().clone();
    if let Err(e) = mailbox::enqueue_remote(&db, &turn, &agent, &body) {
        eprintln!("[aygent][remote] enqueue failed: {e}");
    }
    app.state::<drainer::DrainSignal>().nudge();
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
