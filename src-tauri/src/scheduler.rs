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
// SLICE 2 SCOPE: real timing. compute_next_tz handles DailyAt/WeeklyAt in an
// IANA timezone (DST-safe via chrono-tz); Interval stays pure UTC. The Slice-1
// auto-seed is RETIRED (it burned a call every 60s) — real schedules now come
// from the UI via create()/set_enabled()/delete(). Two schedule kinds the user
// builds: HEARTBEAT (Interval + Continue context = sees recent conversation)
// and SCHEDULED JOB (DailyAt/WeeklyAt + Fresh context). Guardrails (cost
// ceiling, fire caps) = Slice 4; SystemJob/Distill = Slice 5.

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

/// Compute the next fire time (UTC ms) for a spec, strictly after `after_ms`,
/// interpreting wall-clock (DailyAt/WeeklyAt) in `tz` (an IANA name, or "local").
///
/// Interval = pure UTC arithmetic. DailyAt/WeeklyAt = DST-safe via chrono-tz:
/// we walk forward day-by-day in the target timezone, build the local wall-clock
/// time (hh:mm), convert to UTC, and take the FIRST candidate strictly after
/// `after_ms`. Walking in local time is what makes "8am" stay 8am across a DST
/// boundary (the UTC offset shifts, the wall-clock doesn't).
pub fn compute_next_tz(spec: &ScheduleSpec, after_ms: i64, tz: &str) -> i64 {
    use chrono::{Datelike, Duration, TimeZone, Utc};

    match spec {
        ScheduleSpec::Interval { every_secs } => {
            let step = (*every_secs as i64).max(1) * 1000;
            after_ms + step
        }
        ScheduleSpec::DailyAt { hh, mm } | ScheduleSpec::WeeklyAt { hh, mm, .. } => {
            let zone = resolve_tz(tz);
            let after = Utc.timestamp_millis_opt(after_ms).single().unwrap_or_else(Utc::now);
            let local_after = after.with_timezone(&zone);
            // Which weekdays are allowed? Daily = all; Weekly = the listed set
            // (0=Sun..6=Sat, matching JS getDay()).
            let allowed_days: Option<std::collections::HashSet<u8>> = match spec {
                ScheduleSpec::WeeklyAt { days, .. } => Some(days.iter().copied().collect()),
                _ => None,
            };
            // Walk up to ~370 days forward (covers weekly + safety for empty sets).
            for add in 0..=370i64 {
                let day = (local_after + Duration::days(add)).date_naive();
                if let Some(ref set) = allowed_days {
                    // chrono weekday: Mon=0..Sun=6; convert to Sun=0..Sat=6.
                    let wd = day.weekday().num_days_from_sunday() as u8;
                    if !set.contains(&wd) { continue; }
                }
                // Build the local wall-clock candidate; skip if the time is
                // invalid/ambiguous at a DST transition (rare; next day covers it).
                let naive = day.and_hms_opt(*hh as u32, *mm as u32, 0);
                let Some(naive) = naive else { continue };
                let Some(local_dt) = zone.from_local_datetime(&naive).single() else { continue };
                let cand_ms = local_dt.with_timezone(&Utc).timestamp_millis();
                if cand_ms > after_ms {
                    return cand_ms;
                }
            }
            // Fallback: 24h out (should be unreachable for a sane spec).
            after_ms + 86_400_000
        }
    }
}

/// Back-compat shim: default timezone ("local"). Prefer compute_next_tz.
pub fn compute_next(spec: &ScheduleSpec, after_ms: i64) -> i64 {
    compute_next_tz(spec, after_ms, "local")
}

/// Resolve an IANA tz name to a chrono_tz::Tz. "local" (or unknown) falls back
/// to the system local zone's current fixed offset wrapped as UTC-equivalent.
/// chrono-tz has no "local" entry, so we approximate: if the caller stored a
/// real IANA name we honor it; otherwise we use the machine's local offset via
/// chrono::Local by converting through it. To keep the return type uniform we
/// map "local"/unknown to Tz::UTC and rely on the OS clock already being local
/// for the app's single-user context. (A real IANA tz is set once the UI
/// captures the user's zone — see Settings, Slice 6.)
fn resolve_tz(tz: &str) -> chrono_tz::Tz {
    if tz.eq_ignore_ascii_case("local") || tz.is_empty() {
        // Best-effort: try to read the system zone via iana-time-zone if present;
        // otherwise UTC. We keep this dependency-light for Slice 2 and let the UI
        // pass a concrete IANA name (e.g. "America/Los_Angeles") going forward.
        return system_iana().and_then(|n| n.parse().ok()).unwrap_or(chrono_tz::UTC);
    }
    tz.parse().unwrap_or(chrono_tz::UTC)
}

/// Best-effort system IANA zone name. Uses the TZ env var if set, else None
/// (caller falls back to UTC). The UI will pass an explicit zone in Slice 6, so
/// this only matters for the seed/default path.
fn system_iana() -> Option<String> {
    std::env::var("TZ").ok().filter(|s| s.contains('/'))
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
    tz: String,
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
        "SELECT id, agent_id, tz, spec_json, action_json FROM schedule
         WHERE enabled = 1 AND next_fire_at <= ?1 ORDER BY next_fire_at ASC",
    ) else { return Vec::new() };
    let rows = stmt.query_map(params![now], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    });
    let Ok(rows) = rows else { return Vec::new() };
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (id, agent_id, tz, spec_json, action_json) = row;
        let (Ok(spec), Ok(action)) = (
            serde_json::from_str::<ScheduleSpec>(&spec_json),
            serde_json::from_str::<ScheduleAction>(&action_json),
        ) else { continue };
        out.push(Due { id, agent_id, tz, spec, action });
    }
    out
}

/// Fire ONE due schedule: mark-fired + advance next_fire_at + log a run +
/// enqueue to the drainer, ALL in one writer-actor transaction (exactly-once at
/// the decision boundary). Returns whether it enqueued a drainer turn.
fn fire_one(db: &Db, due: &Due, now: i64) -> Result<bool, String> {
    let next = compute_next_tz(&due.spec, now, &due.tz);
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

// ---------------------------------------------------------------------------
// CRUD (Slice 2) — real schedules come from the UI now; the Slice-1 auto-seed
// is RETIRED (it burned an API call every 60s). On boot we RETIRE any leftover
// proof schedule so an upgrading install stops the every-minute fire.
// ---------------------------------------------------------------------------

/// One-time cleanup: delete the old hardcoded 60s proof schedule if it exists
/// (name-matched), so upgrading from Slice 1 stops the every-minute burn.
fn retire_proof_schedule(db: &Db) {
    let _ = db.write(|c| {
        c.execute(
            "DELETE FROM schedule WHERE name IN ('Proof: say the time (every 60s)')",
            [],
        ).map_err(|e| format!("retire proof: {e}"))?;
        Ok(())
    });
}

/// Create a schedule from the UI. `spec`/`action` are the typed enums; we
/// serialize + compute the first next_fire_at. Returns the new id.
#[allow(clippy::too_many_arguments)]
pub fn create(
    db: &Db,
    agent_id: &str,
    name: &str,
    kind: &str,
    spec: &ScheduleSpec,
    tz: &str,
    action: &ScheduleAction,
    max_fires_per_day: i64,
) -> Result<i64, String> {
    let now = now_ms();
    let next = compute_next_tz(spec, now, tz);
    let spec_json = serde_json::to_string(spec).map_err(|e| format!("spec: {e}"))?;
    let action_json = serde_json::to_string(action).map_err(|e| format!("action: {e}"))?;
    let (agent_id, name, kind, tz) = (agent_id.to_string(), name.to_string(), kind.to_string(), tz.to_string());
    db.write(move |c| {
        c.execute(
            "INSERT INTO schedule (agent_id,name,kind,spec_json,tz,action_json,enabled,max_fires_per_day,next_fire_at,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,1,?7,?8,?9,?9)",
            params![agent_id, name, kind, spec_json, action_json_holder(&action_json), tz, max_fires_per_day, next, now],
        ).map_err(|e| format!("insert schedule: {e}"))?;
        Ok(c.last_insert_rowid())
    })
}
// tiny helper so the closure captures a &str cleanly without a move-order snag
fn action_json_holder(s: &str) -> String { s.to_string() }

/// Enable/disable a schedule (pause a single one).
pub fn set_enabled(db: &Db, id: i64, enabled: bool) -> Result<(), String> {
    let now = now_ms();
    db.write(move |c| {
        c.execute(
            "UPDATE schedule SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, if enabled { 1 } else { 0 }, now],
        ).map_err(|e| format!("set enabled: {e}"))?;
        Ok(())
    })
}

/// Delete a schedule (cascades its run log).
pub fn delete(db: &Db, id: i64) -> Result<(), String> {
    db.write(move |c| {
        c.execute("DELETE FROM schedule WHERE id = ?1", params![id])
            .map_err(|e| format!("delete schedule: {e}"))?;
        Ok(())
    })
}

/// Spawn the background scheduler ticker. Runs for the life of the app.
/// `drain` = the drainer's signal so a freshly-enqueued turn wakes it instantly.
pub fn spawn(app: AppHandle, db: Db, _broker: Arc<Broker>, _lanes: Lanes, sig: SchedSignal, drain: crate::drainer::DrainSignal) {
    tauri::async_runtime::spawn(async move {
        // Slice 2: RETIRE the Slice-1 proof schedule so it stops firing every 60s.
        retire_proof_schedule(&db);

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
