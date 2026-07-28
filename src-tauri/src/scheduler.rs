// AYGENT — Scheduler (M1.8). "The drainer with a clock in front of it." (Atlas)
//
// One boot-spawned tokio ticker (mirrors drainer.rs). It does NOT execute turns
// itself — it decides WHAT is due, marks it fired EXACTLY ONCE through the
// writer actor (mark-fired + enqueue in one txn), and hands the fire to the
// EXISTING drainer via the mailbox. So a scheduled turn is just another mailbox
// item that flows drainer → acquire_low lane (yields to humans) →
// run_headless_turn. Differs from a human/inter-agent turn only by an `origin`.
//
// TICK MODEL: sleep-until-next, NOT poll. Compute MIN(next_fire_at) across
// enabled schedules, sleep until then (capped at 60s so a wall-clock jump /
// laptop wake / DST is re-evaluated), fire everything due, recompute, re-sleep.
// A tokio::Notify wakes the loop on any hot edit (add/edit/delete/enable/pause).
//
// SLICE 1 SCOPE (the proof): spawn the ticker; on boot, SEED one hardcoded
// Interval{60s} AgentTurn{Fresh,"say the current time"} for the active agent IF
// no schedules exist yet; fire it → enqueue to drainer → watch a turn appear in
// a pane every ~60s, unattended. compute_next for DailyAt/WeeklyAt + guardrails
// + UI arrive in later slices; the timing math here is Interval-only for now.

use crate::broker::Broker;
use crate::lanes::Lanes;
use crate::writer::Db;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::Notify;

/// UTC epoch milliseconds — the unit every timestamp column uses.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Typed schedule spec + action (serialized as JSON in the row so adding a
// variant later needs NO migration — serde forward-compat).
// ---------------------------------------------------------------------------

/// The timing source of truth; `next_fire_at` is derived from this.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleSpec {
    Interval { every_secs: u64 },
    DailyAt { hh: u8, mm: u8 },
    WeeklyAt { days: Vec<u8>, hh: u8, mm: u8 }, // days: 0=Sun..6=Sat
}

/// What a fire actually does.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleAction {
    /// Fire a model turn at the agent (costs API money). Runs via the drainer.
    AgentTurn {
        prompt_template: String,
        /// "fresh" = clean isolated turn (cron); "continue" = sees recent
        /// conversation (heartbeat). Slice 1 only uses fresh.
        #[serde(default = "default_ctx")]
        context: String,
    },
    /// Deterministic built-in Rust job (usually $0, bypasses the model). Wired
    /// in Slice 5 (Distill/Reconcile). Present now so the enum shape is frozen.
    SystemJob { job: String },
}
fn default_ctx() -> String { "fresh".into() }

/// Compute the next fire time (UTC ms) for a spec, strictly after `after_ms`.
/// Slice 1: Interval only (pure UTC arithmetic). Daily/Weekly land in Slice 2
/// with chrono-tz; until then they schedule 24h out as a safe placeholder so a
/// row is never left without a future next_fire_at.
pub fn compute_next(spec: &ScheduleSpec, after_ms: i64) -> i64 {
    match spec {
        ScheduleSpec::Interval { every_secs } => {
            let step = (*every_secs as i64).max(1) * 1000;
            after_ms + step
        }
        // Placeholder until Slice 2's chrono-tz math — never leave NULL/past.
        ScheduleSpec::DailyAt { .. } | ScheduleSpec::WeeklyAt { .. } => after_ms + 86_400_000,
    }
}

/// Shared wake handle (mirrors the drainer's DrainSignal). Any GUI timing edit
/// writes SQLite then calls `.nudge()`; the ticker recomputes wake_at.
#[derive(Clone)]
pub struct SchedSignal(Arc<Notify>);
impl SchedSignal {
    pub fn new() -> Self { SchedSignal(Arc::new(Notify::new())) }
    pub fn nudge(&self) { self.0.notify_one(); }
    fn notified(&self) -> impl std::future::Future<Output = ()> + '_ { self.0.notified() }
}

/// A due schedule the ticker is about to fire.
struct Due {
    id: i64,
    agent_id: String,
    spec: ScheduleSpec,
    action: ScheduleAction,
}

/// Read the earliest next_fire_at across enabled schedules (None if none).
fn earliest_next(db: &Db) -> Option<i64> {
    let conn = db.reader().ok()?;
    conn.query_row(
        "SELECT MIN(next_fire_at) FROM schedule WHERE enabled = 1",
        [],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
}

/// Read all enabled schedules whose next_fire_at <= now.
fn due_now(db: &Db, now: i64) -> Vec<Due> {
    let Ok(conn) = db.reader() else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, agent_id, spec_json, action_json FROM schedule
         WHERE enabled = 1 AND next_fire_at <= ?1 ORDER BY next_fire_at ASC",
    ) else { return Vec::new() };
    let rows = stmt.query_map(params![now], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    });
    let Ok(rows) = rows else { return Vec::new() };
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (id, agent_id, spec_json, action_json) = row;
        let (Ok(spec), Ok(action)) = (
            serde_json::from_str::<ScheduleSpec>(&spec_json),
            serde_json::from_str::<ScheduleAction>(&action_json),
        ) else { continue };
        out.push(Due { id, agent_id, spec, action });
    }
    out
}

/// Fire ONE due schedule: mark-fired + advance next_fire_at + log a run +
/// enqueue to the drainer, ALL in one writer-actor transaction (exactly-once at
/// the decision boundary). Returns whether it enqueued a drainer turn.
fn fire_one(db: &Db, due: &Due, now: i64) -> Result<bool, String> {
    let next = compute_next(&due.spec, now);
    let id = due.id;
    let agent_id = due.agent_id.clone();
    // Only AgentTurn enqueues to the drainer in Slice 1; SystemJob is Slice 5.
    let turn_body: Option<String> = match &due.action {
        ScheduleAction::AgentTurn { prompt_template, .. } => Some(prompt_template.clone()),
        ScheduleAction::SystemJob { .. } => None,
    };
    let body = turn_body.clone().unwrap_or_default();
    let recipient = agent_id.clone(); // the fired turn is addressed to this agent

    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        // Advance timing + count. daily counter logic is Slice 4; Slice 1 just
        // bumps last_fired + next_fire so the ticker doesn't re-fire instantly.
        tx.execute(
            "UPDATE schedule SET last_fired_at = ?2, next_fire_at = ?3, updated_at = ?2 WHERE id = ?1",
            params![id, now, next],
        ).map_err(|e| format!("advance schedule: {e}"))?;

        // Durable run row.
        let state = if turn_body.is_some() { "queued" } else { "ok" };
        tx.execute(
            "INSERT INTO schedule_run (schedule_id, agent_id, fired_at, state) VALUES (?1,?2,?3,?4)",
            params![id, agent_id, now, state],
        ).map_err(|e| format!("insert run: {e}"))?;
        let run_id = tx.last_insert_rowid();

        // Enqueue the turn into the SAME mailbox the drainer drains, tagged as
        // scheduler-origin via the from_agent sentinel + run_id in the body
        // envelope. (Slice 1: a plain self-addressed message; origin plumbing
        // richens in later slices. The drainer runs it headless.)
        if turn_body.is_some() {
            tx.execute(
                "INSERT INTO mailbox (from_agent,to_agent,body,root_id,depth,ancestry,status,created_at)
                 VALUES (?1,?2,?3,0,6,'','pending',?4)",
                params![format!("scheduler:{run_id}"), recipient, body, now],
            ).map_err(|e| format!("enqueue turn: {e}"))?;
            let mid = tx.last_insert_rowid();
            tx.execute("UPDATE mailbox SET root_id = ?1 WHERE id = ?1", params![mid])
                .map_err(|e| format!("set root: {e}"))?;
            tx.execute(
                "INSERT OR IGNORE INTO mailbox_budget (root_id, turns, cap, created_at) VALUES (?1, 0, 12, ?2)",
                params![mid, now],
            ).map_err(|e| format!("init budget: {e}"))?;
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(turn_body.is_some())
    })
}

/// SLICE 1 seed: if there are NO schedules yet and an active agent exists, seed
/// one 60s self-firing AgentTurn so the proof is visible out of the box. Idempo-
/// tent (only seeds when the table is empty). Real schedules come from the UI.
fn seed_proof_schedule(db: &Db) {
    let Ok(conn) = db.reader() else { return };
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schedule", [], |r| r.get(0))
        .unwrap_or(0);
    if count > 0 { return; }
    let active: Option<String> = conn
        .query_row("SELECT active_id FROM app_state WHERE id = 0", [], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    let Some(agent_id) = active else { return };

    let spec = ScheduleSpec::Interval { every_secs: 60 };
    let action = ScheduleAction::AgentTurn {
        prompt_template: "What is the current time? Reply in one short sentence.".into(),
        context: "fresh".into(),
    };
    let now = now_ms();
    let next = compute_next(&spec, now);
    let (spec_json, action_json) = (
        serde_json::to_string(&spec).unwrap_or_default(),
        serde_json::to_string(&action).unwrap_or_default(),
    );
    let _ = db.write(move |c| {
        c.execute(
            "INSERT INTO schedule (agent_id,name,kind,spec_json,tz,action_json,enabled,next_fire_at,created_at,updated_at)
             VALUES (?1,?2,'interval',?3,'local',?4,1,?5,?6,?6)",
            params![agent_id, "Proof: say the time (every 60s)", spec_json, action_json, next, now],
        ).map_err(|e| format!("seed schedule: {e}"))?;
        Ok(())
    });
}

/// Spawn the background scheduler ticker. Runs for the life of the app.
/// `drain` = the drainer's signal so a freshly-enqueued turn wakes it instantly.
pub fn spawn(app: AppHandle, db: Db, _broker: Arc<Broker>, _lanes: Lanes, sig: SchedSignal, drain: crate::drainer::DrainSignal) {
    tauri::async_runtime::spawn(async move {
        // Slice 1: seed the proof schedule once (empty table + active agent).
        seed_proof_schedule(&db);

        // Safety-net cap so a wall-clock jump / laptop wake / DST is re-evaluated
        // within 60s even if the next fire is far out.
        let max_sleep = std::time::Duration::from_secs(60);

        loop {
            // 1) When is the next fire? Sleep until then (capped), or until nudged.
            let now = now_ms();
            let sleep_dur = match earliest_next(&db) {
                Some(next) if next > now => {
                    let ms = (next - now).min(max_sleep.as_millis() as i64).max(0) as u64;
                    std::time::Duration::from_millis(ms)
                }
                Some(_) => std::time::Duration::from_millis(0), // something already due
                None => max_sleep, // no schedules — idle-wait, wake on nudge
            };

            tokio::select! {
                _ = tokio::time::sleep(sleep_dur) => {}
                _ = sig.notified() => { continue; } // hot edit — recompute
            }

            // 2) Fire everything due.
            let now = now_ms();
            let due = due_now(&db, now);
            let mut enqueued_any = false;
            for d in &due {
                match fire_one(&db, d, now) {
                    Ok(true) => { enqueued_any = true; }
                    Ok(false) => {}
                    Err(e) => eprintln!("[aygent] schedule {} fire failed: {e}", d.id),
                }
            }
            // 3) Wake the drainer so the just-enqueued turns run immediately.
            if enqueued_any {
                drain.nudge();
                let _ = tauri::Emitter::emit(&app, "agent-activity", &serde_json::json!({
                    "kind": "scheduler_fired", "count": due.len(),
                }));
            }
        }
    });
}
