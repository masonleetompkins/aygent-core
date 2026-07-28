// AYGENT — Inter-agent delivery engine (M1.4 fix, Atlas A).
//
// THE PROBLEM this solves: send_message enqueues a durable `mailbox` row, but
// nothing ever drained it — the recipient never woke up, so the whole
// agent-to-agent conversation was inert. The turn loop (agent_stream) is a
// UI-invoked streaming command; it only runs when a human is looking at a chat.
//
// THE FIX (Atlas's verdict): a Rust-side BACKGROUND tokio task that drains
// mailboxes and runs recipient turns HEADLESSLY — independent of any open pane.
//
// FLOW per tick:
//   1. Find agents with pending mail (mailbox::pending_counts).
//   2. For each (bounded by a semaphore = max_concurrency knob), spawn a task:
//      a. acquire the recipient's DEDICATED lane "inbox:{agentId}" via
//         acquire_low (yields to human turns — user always preempts).
//      b. mailbox::take_next_for -> the message (marks delivered, charges the
//         per-tree budget; over-budget rows are dropped, never run).
//      c. run the recipient's turn HEADLESSLY (its own model/provider/folder/
//         soul), with a BROADCAST event sink so any open UI pane can watch.
//      d. persist the turn to the recipient's conversation history (in-thread,
//         tagged from the sender) so it's visible when the user opens that pane.
//      e. if that turn called send_message, it's just another mailbox row the
//         drainer picks up next tick -> the reply loops back. The mailbox IS
//         the reply channel; no special plumbing.
//
// A tokio::Notify lets send() NUDGE the drainer so delivery is near-instant
// instead of poll-laggy. We still poll on a slow tick as a safety net.
//
// FAILURE MODES handled: dedicated inbox lanes bound the blast radius (a chatty
// pair never touches a human's chat lane); the semaphore bounds paid API calls;
// a panicking turn is caught (budget already charged = no infinite retry, per
// Atlas's "log + drop" minimal cut).

use crate::broker::Broker;
use crate::lanes::Lanes;
use crate::writer::Db;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::{Notify, Semaphore};

/// Shared handle the rest of the app uses to WAKE the drainer after enqueuing a
/// message. Cheap to clone.
#[derive(Clone)]
pub struct DrainSignal(Arc<Notify>);
impl DrainSignal {
    pub fn new() -> Self { DrainSignal(Arc::new(Notify::new())) }
    /// Wake the drainer now (called right after mailbox::send commits).
    pub fn nudge(&self) { self.0.notify_one(); }
    fn notified(&self) -> impl std::future::Future<Output = ()> + '_ { self.0.notified() }
}

/// Spawn the background drainer. Runs for the life of the app. `run_headless` is
/// the callback that executes one recipient turn (defined in lib.rs where the
/// provider loop lives) — dependency-injected so this module stays free of the
/// giant agent-loop code.
pub fn spawn(
    app: AppHandle,
    db: Db,
    broker: Arc<Broker>,
    lanes: Lanes,
    signal: DrainSignal,
) {
    tauri::async_runtime::spawn(async move {
        // Slow safety-net poll; the Notify nudge handles the fast path.
        let poll = std::time::Duration::from_millis(1500);
        loop {
            // Wait for a nudge OR the poll interval, whichever first.
            tokio::select! {
                _ = signal.notified() => {},
                _ = tokio::time::sleep(poll) => {},
            }

            // Who has pending mail?
            let counts = match crate::mailbox::pending_counts(&db) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if counts.is_empty() { continue; }

            // Concurrency bound from the Settings knob (default 6).
            let max = crate::repo::get_knobs(&db).map(|k| k.max_concurrency).unwrap_or(6).max(1) as usize;
            let sem = Arc::new(Semaphore::new(max));

            for (agent_id, n) in counts {
                if n <= 0 { continue; }
                let permit = match sem.clone().acquire_owned().await { Ok(p) => p, Err(_) => break };
                let (app2, db2, broker2, lanes2) = (app.clone(), db.clone(), broker.clone(), lanes.clone());
                tauri::async_runtime::spawn(async move {
                    // Dedicated inbox lane so delivery never contends a human's
                    // chat lane. acquire_low yields to any human turn.
                    let lane_key = format!("inbox:{agent_id}");
                    let _guard = lanes2.acquire_low(&lane_key).await;

                    // Drain ALL currently-pending messages for this agent in one
                    // hold of its lane (fair: other agents run on their own lanes
                    // in parallel; this agent's own inbox is serialized).
                    loop {
                        let msg = match crate::mailbox::take_next_for(&db2, &agent_id) {
                            Ok(Some(m)) => m,
                            _ => break,
                        };
                        // Run the recipient's turn headlessly. catch failures so a
                        // bad turn doesn't kill the drainer (budget already
                        // charged at take_next -> no infinite retry).
                        if let Err(e) = crate::run_headless_turn(&app2, &db2, &broker2, &lanes2, &agent_id, &msg).await {
                            eprintln!("[aygent] inbox turn for {agent_id} failed: {e}");
                        }
                    }
                    drop(permit);
                });
            }
        }
    });
}
