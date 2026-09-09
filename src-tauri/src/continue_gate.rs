// AYGENT — task_continue TIMER PICKER (Mason 09-08).
//
// The agent suggests a wake-up delay, but the HUMAN picks the actual duration:
// when any turn calls task_continue, the turn parks here and a modal in chat
// offers 1 / 3 / 5 / 10 / 15 minutes. The answer (or a timeout) resolves the
// delay, and the wake-up is enqueued exactly as before.
//
// Wiring mirrors browser.rs request_permission: a pending map of oneshot
// senders + a `task-continue-pick` event the UI answers via
// `task_continue_answer`. One resolution per id; 3 minutes of silence falls
// back to the agent's suggested delay so an unattended turn never wedges.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{Emitter, Manager};

use crate::writer::Db;

/// How long the picker waits for an answer before the suggestion stands.
const PICK_TIMEOUT_SECS: u64 = 180;
/// The exact options the chat modal offers (minutes).
pub const PICK_MINUTES: [u64; 5] = [1, 3, 5, 10, 15];

static SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct ContinueGate {
    pending: Mutex<HashMap<String, tokio::sync::oneshot::Sender<u64>>>,
}

/// The task_continue tool body, shared by all four dispatch sites
/// (agent_stream ×2 shapes, run_headless_turn ×2 providers).
/// Returns (result_text, is_err).
pub async fn handle_task_continue(
    app: &tauri::AppHandle,
    db: &Db,
    agent_id: &str,
    conv: String,
    input: &serde_json::Value,
) -> (String, bool) {
    let requested = input.get("delay_secs").and_then(|d| d.as_u64()).unwrap_or(60).clamp(5, 3600);
    let note = input.get("note").and_then(|n| n.as_str()).unwrap_or("").trim().to_string();
    if note.is_empty() {
        return ("task_continue refused: a non-empty note is required (say what to check on wake-up)".to_string(), true);
    }
    let agent_name = crate::repo::get_agent(db, agent_id).ok().flatten().map(|a| a.name).unwrap_or_else(|| agent_id.to_string());
    let id = {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        format!("tc-{t:x}-{n:x}")
    };
    let (tx, rx) = tokio::sync::oneshot::channel::<u64>();
    {
        let gate = app.state::<ContinueGate>();
        let mut p = gate.pending.lock().expect("continue gate poisoned");
        p.insert(id.clone(), tx);
    }
    let _ = app.emit("task-continue-pick", serde_json::json!({
        "id": id, "agentId": agent_id, "agentName": agent_name,
        "note": note, "requestedSecs": requested, "optionsMin": PICK_MINUTES,
    }));
    let (delay, picked) = match tokio::time::timeout(std::time::Duration::from_secs(PICK_TIMEOUT_SECS), rx).await {
        Ok(Ok(v)) => (v.clamp(5, 3600), true),
        _ => (requested, false),
    };
    {
        let gate = app.state::<ContinueGate>();
        let mut p = gate.pending.lock().expect("continue gate poisoned");
        p.remove(&id);
    }
    match crate::mailbox::enqueue_continue(db, agent_id, &format!("conv:{}\n{}", conv, note), delay) {
        Ok(_) => {
            let how = if picked { " (you picked this duration)" } else { " (no pick — my suggestion stood)" };
            (format!("wake-up scheduled in {delay}s{how} — end your turn now; you'll be woken with your note"), false)
        }
        Err(e) => (format!("task_continue failed: {e}"), true),
    }
}

/// The chat modal's answer: resolve a pending picker with the chosen delay.
#[tauri::command]
pub fn task_continue_answer(gate: tauri::State<'_, ContinueGate>, id: String, delay_secs: u64) -> Result<(), String> {
    let tx = gate.pending.lock().map_err(|_| "continue gate poisoned")?
        .remove(&id).ok_or("that timer picker already closed (answered or timed out)")?;
    tx.send(delay_secs.clamp(5, 3600)).map_err(|_| "the agent turn already moved on".to_string())
}
