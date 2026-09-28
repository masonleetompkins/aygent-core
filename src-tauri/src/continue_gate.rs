// AYGENT — task_continue v2 (Mason 09-22): the timer picker is GONE.
//
// Two modes, chosen by the model per call:
//   poll — "check back in 30s automatically, repeat until done." The wake-up is
//          enqueued exactly like before (mailbox self-send, 30s due) and the
//          drainer runs it headlessly. No human involvement, no modal.
//          Guardrail: after MAX_AUTO_POLLS delivered auto-wake-ups for one
//          conversation (~12 min of checking), the next poll call parks as a
//          MANUAL wait instead — a runaway process must surface to the human
//          rather than burn API calls forever. The model is told this happened.
//   wait — "pause until the human clicks Continue." Recorded in the manual map
//          and announced on `task-continue-waiting`; the chat renders an inline
//          Continue button under the message. Clicking consumes the record and
//          runs the resume turn. The button disappearing IS the state.

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{Emitter, Manager};

use crate::writer::Db;

/// Fixed re-check interval for poll mode (seconds). Not model-tunable: one
/// predictable rhythm the human can reason about.
pub const POLL_SECS: u64 = 30;
/// Auto-wake-ups per conversation before a poll chain parks for the human.
/// 24 × 30s ≈ 12 minutes of autonomous checking.
pub const MAX_AUTO_POLLS: i64 = 24;

/// Manually-parked continuations: conv_id -> note. Consumed by the Continue
/// button (task_continue_consume). No timeouts, no pickers.
#[derive(Default)]
pub struct ContinueGate {
    manual: Mutex<HashMap<String, String>>,
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
    let mode = input.get("mode").and_then(|m| m.as_str()).unwrap_or("poll").trim().to_string();
    let note = input.get("note").and_then(|n| n.as_str()).unwrap_or("").trim().to_string();
    if note.is_empty() {
        return ("task_continue refused: a non-empty note is required (say what to check on wake-up / resume)".to_string(), true);
    }
    if conv.trim().is_empty() {
        return ("task_continue refused: no conversation to resume into".to_string(), true);
    }
    match mode.as_str() {
        "wait" => park_manual(app, agent_id, &conv, &note),
        _ => poll_or_park(app, db, agent_id, &conv, &note, &mode).await,
    }
}

/// Poll mode (and any unknown mode, which falls back to poll): wake
/// automatically in POLL_SECS unless this conversation already burned through
/// MAX_AUTO_POLLS autonomous checks — then park for the human instead.
async fn poll_or_park(
    app: &tauri::AppHandle,
    db: &Db,
    agent_id: &str,
    conv: &str,
    note: &str,
    mode: &str,
) -> (String, bool) {
    if mode != "poll" {
        // Unknown mode value (e.g. a stale "delay_secs"-era call): say so once,
        // then do the predictable thing rather than erroring the turn.
        eprintln!("[aygent][continue] unknown mode `{mode}` — treating as poll");
    }
    if auto_polls_used(db, agent_id, conv) >= MAX_AUTO_POLLS {
        park_manual(app, agent_id, conv, note);
        return (format!(
            "you've checked automatically {MAX_AUTO_POLLS} times on this thread — I'm parking this for the human instead of polling forever. Tell them plainly what's still open, then end your turn; they'll click Continue when ready (or tell you to stop)."
        ), false);
    }
    match crate::mailbox::enqueue_continue(db, agent_id, &format!("conv:{conv}\n{note}"), POLL_SECS) {
        Ok(_) => (format!(
            "I'll wake you automatically in {POLL_SECS}s with your note — end your turn now. On wake-up, check the process: if it's done, continue working; if not, call task_continue again (mode \"poll\") and end your turn. Repeat until done, then give your final summary."
        ), false),
        Err(e) => (format!("task_continue failed: {e}"), true),
    }
}

/// How many auto-wake-ups already DELIVERED for this (agent, conv) recently.
/// Counts delivered `continue:` rows whose body opens with this conv's id.
/// Windowed so an old poll storm doesn't permanently taint the thread.
fn auto_polls_used(db: &Db, agent_id: &str, conv: &str) -> i64 {
    let (aid, prefix) = (agent_id.to_string(), format!("conv:{conv}\n"));
    db.reader().ok().and_then(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM mailbox WHERE to_agent = ?1 AND status = 'delivered' \
             AND from_agent LIKE 'continue:%' AND body LIKE ?2 || '%' \
             AND delivered_at > ?3",
            rusqlite::params![aid, prefix, epoch_secs() - 7200],
            |r| r.get::<_, i64>(0),
        ).ok()
    }).unwrap_or(0)
}

/// Park a manual wait: record it + tell the UI to show the Continue button.
fn park_manual(app: &tauri::AppHandle, agent_id: &str, conv: &str, note: &str) -> (String, bool) {
    {
        let gate = app.state::<ContinueGate>();
        let mut m = gate.manual.lock().expect("continue gate poisoned");
        m.insert(conv.to_string(), note.to_string());
    }
    let _ = app.emit("task-continue-waiting", serde_json::json!({
        "conv": conv, "agentId": agent_id, "note": note,
    }));
    (format!(
        "paused — the human sees a Continue button under your message. When they click it you'll be woken with your note in a full working turn. End your turn now with a one-line status (what's open, what happens next)."
    ), false)
}

/// Does this conversation have a parked manual wait? (UI reads it on conv open
/// so the button survives reloads.)
#[tauri::command]
pub fn task_continue_pending(gate: tauri::State<'_, ContinueGate>, conv: String) -> Option<String> {
    gate.manual.lock().ok()?.get(&conv).cloned()
}

/// Take a parked wait (the Continue button calls this as it fires the resume).
/// Returns the note so the resume turn carries the model's own context.
#[tauri::command]
pub fn task_continue_consume(gate: tauri::State<'_, ContinueGate>, conv: String) -> Option<String> {
    gate.manual.lock().ok()?.remove(&conv)
}

/// Epoch seconds (mailbox times are seconds, not ms).
fn epoch_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
