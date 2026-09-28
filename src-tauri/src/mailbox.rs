// AYGENT — Inter-agent messaging (M1.4 #7, Atlas B).
//
// Agents are aware of each other and can MESSAGE each other. The primitive
// (Atlas's decision): a durable SQLite `mailbox` table + a `send_message` TOOL
// the sender invokes, delivered as a NEW ASYNC TURN on the recipient's lane.
// NEVER synchronous — a sync "wait for B's reply" while B messages A deadlocks
// (A-waits-B-waits-A) and wedges a lane. It's a conversation, not an RPC.
//
// SAFETY RAILS (all required, per Atlas):
//   - depth (hop-count TTL): decrement per relay, drop at 0 -> kills infinite
//     ping-pong.
//   - per-tree token/turn BUDGET (mailbox_budget.cap): the runaway-cost backstop
//     -> two agents can't bankrupt an account overnight.
//   - cycle guard: each message carries its ancestry (agent-id chain); drop if
//     the target already appears -> no A->B->A->B loops.
//   - no self-send.
//   - USER TURNS PREEMPT: delivery runs on the recipient lane via
//     Lanes::acquire_low (yields to any human turn) — the human never waits
//     behind a chatty agent pair.
//
// This module is the DATA layer (enqueue + drain + guard checks). The actual
// turn execution (feeding a delivered message into agent_stream on the
// recipient's lane) is driven from lib.rs where the provider loop lives.

use crate::writer::Db;
use rusqlite::{params, OptionalExtension};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Default hop-count TTL for a fresh inter-agent message chain.
pub const DEFAULT_DEPTH: i64 = 4;
/// Default per-tree turn budget (max inter-agent turns in one conversation tree).
pub const DEFAULT_BUDGET: i64 = 12;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Message {
    pub id: i64,
    pub from_agent: String,
    pub to_agent: String,
    pub body: String,
    pub root_id: i64,
    pub depth: i64,
    pub ancestry: String,
    pub status: String,
    pub created_at: i64,
}

/// Result of an enqueue attempt — the UI/tool surfaces the reason on refusal.
#[derive(Debug, Clone, serde::Serialize)]
pub enum SendResult {
    Queued { id: i64, root_id: i64 },
    Refused { reason: String },
}

/// Enqueue a message from one agent to another with all guards applied.
/// `parent_id` = the message being replied to (0 for a fresh chain from a human-
/// triggered agent turn). Guards: no self-send, depth TTL, cycle, tree budget.
pub fn send(
    db: &Db,
    from_agent: &str,
    to_agent: &str,
    body: &str,
    parent_id: i64,
) -> Result<SendResult, String> {
    if from_agent == to_agent {
        return Ok(SendResult::Refused { reason: "an agent cannot message itself".into() });
    }
    if to_agent.trim().is_empty() {
        return Ok(SendResult::Refused { reason: "no recipient".into() });
    }

    // Recipient must exist (FK would reject anyway; nicer message here).
    {
        let conn = db.reader()?;
        let exists: bool = conn.query_row("SELECT 1 FROM agent WHERE id = ?1 AND archived = 0",
            params![to_agent], |_| Ok(true)).optional().map_err(|e| format!("check recipient: {e}"))?.unwrap_or(false);
        if !exists {
            return Ok(SendResult::Refused { reason: "recipient agent not found or archived".into() });
        }
    }

    // Resolve the parent chain (root_id, depth, ancestry, parent from/to) if
    // this is a reply. parent_from/parent_to let us block only a TRUE no-op
    // (re-sending the exact same hop) without killing a legitimate reply.
    let (root_id_opt, depth, ancestry_in, parent_from, parent_to): (Option<i64>, i64, String, String, String) = if parent_id > 0 {
        let conn = db.reader()?;
        conn.query_row(
            "SELECT root_id, depth, ancestry, from_agent, to_agent FROM mailbox WHERE id = ?1",
            params![parent_id],
            |r| Ok((Some(r.get::<_, i64>(0)?), r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)),
        ).optional().map_err(|e| format!("parent lookup: {e}"))?
         .unwrap_or((None, DEFAULT_DEPTH, String::new(), String::new(), String::new()))
    } else {
        (None, DEFAULT_DEPTH, String::new(), String::new(), String::new())
    };

    // Depth TTL: a reply gets parent.depth - 1.
    let new_depth = if parent_id > 0 { depth - 1 } else { DEFAULT_DEPTH };
    if new_depth <= 0 {
        return Ok(SendResult::Refused { reason: "message hop limit reached (loop guard)".into() });
    }

    // Cycle guard — CORRECTED AGAIN (bug #2 from testing, Mason's 07-28 screenshot):
    // A REPLY to the original sender is exactly what we want (A asks C -> C replies
    // to A). ancestry stores the SENDER chain, so ancestry.last() is the PREVIOUS
    // SENDER, not the previous recipient. Checking `ancestry.last() == to_agent`
    // wrongly refused C->A, because A was the original sender = last in ancestry.
    // That killed the loop-back (work done, but agent 1 never heard back).
    //
    // True runaway (A<->B forever) is still stopped by the DEPTH TTL + per-tree
    // BUDGET. The ONLY thing we hard-block here is a genuine no-op: re-sending the
    // EXACT SAME HOP as the parent (same from AND same to) — that carries no new
    // information and is the double-send the model sometimes emits in one turn.
    if parent_id > 0 && parent_from == from_agent && parent_to == to_agent {
        return Ok(SendResult::Refused { reason: "that message was already sent in this step".into() });
    }
    let ancestry_list: Vec<&str> = ancestry_in.split(',').filter(|s| !s.is_empty()).collect();
    let mut new_ancestry_parts: Vec<String> = ancestry_list.iter().map(|s| s.to_string()).collect();
    new_ancestry_parts.push(from_agent.to_string());
    let new_ancestry = new_ancestry_parts.join(",");

    // Insert + budget check, atomic in the writer actor.
    let (from_s, to_s, body_s) = (from_agent.to_string(), to_agent.to_string(), body.to_string());
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;

        // Insert the row first so a fresh chain gets its own id as root_id.
        tx.execute(
            "INSERT INTO mailbox (from_agent,to_agent,body,root_id,depth,ancestry,status,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,'pending',?7)",
            params![from_s, to_s, body_s, root_id_opt.unwrap_or(0), new_depth, new_ancestry, now()],
        ).map_err(|e| format!("insert mailbox: {e}"))?;
        let id = tx.last_insert_rowid();
        let root_id = root_id_opt.unwrap_or(id);
        if root_id_opt.is_none() {
            tx.execute("UPDATE mailbox SET root_id = ?1 WHERE id = ?1", params![id])
                .map_err(|e| format!("set root: {e}"))?;
            tx.execute(
                "INSERT OR IGNORE INTO mailbox_budget (root_id, turns, cap, created_at) VALUES (?1, 0, ?2, ?3)",
                params![id, DEFAULT_BUDGET, now()],
            ).map_err(|e| format!("init budget: {e}"))?;
        }

        // Budget check: refuse (mark dead) if the tree has spent its turns.
        let (turns, cap): (i64, i64) = tx.query_row(
            "SELECT turns, cap FROM mailbox_budget WHERE root_id = ?1",
            params![root_id], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().map_err(|e| format!("budget read: {e}"))?.unwrap_or((0, DEFAULT_BUDGET));
        if turns >= cap {
            tx.execute("UPDATE mailbox SET status = 'dead' WHERE id = ?1", params![id])
                .map_err(|e| format!("mark dead: {e}"))?;
            tx.commit().map_err(|e| format!("commit: {e}"))?;
            return Ok(SendResult::Refused { reason: format!("conversation budget reached ({cap} turns) — inter-agent chain stopped") });
        }

        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(SendResult::Queued { id, root_id })
    })
}

/// task_continue (2026-08-01): enqueue a SELF-addressed wake-up, delivered no
/// earlier than now + delay_secs. Bypasses peer guards deliberately (self-send
/// is the point); origin sentinel "continue:<epoch_ms_due>" — the drainer skips
/// it until due, and run_headless_turn frames it as a wake-up, not a peer msg.
/// Fresh root/budget per continuation chain (cap stops infinite self-loops).
pub fn enqueue_continue(db: &Db, agent_id: &str, note: &str, delay_secs: u64) -> Result<i64, String> {
    // mailbox::now() is epoch SECONDS (unlike scheduler::now_ms) — the units
    // bug that broke the first live test (90s became 25h). Everything in this
    // module stays in seconds.
    let due_at = now() + delay_secs as i64;
    let (agent_s, note_s) = (agent_id.to_string(), note.to_string());
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        tx.execute(
            "INSERT INTO mailbox (from_agent,to_agent,body,root_id,depth,ancestry,status,created_at)
             VALUES (?1,?2,?3,0,6,'','pending',?4)",
            params![format!("continue:{due_at}"), agent_s, note_s, now()],
        ).map_err(|e| format!("enqueue continue: {e}"))?;
        let mid = tx.last_insert_rowid();
        tx.execute("UPDATE mailbox SET root_id = ?1 WHERE id = ?1", params![mid])
            .map_err(|e| format!("set root: {e}"))?;
        tx.execute(
            "INSERT OR IGNORE INTO mailbox_budget (root_id, turns, cap, created_at) VALUES (?1, 0, 12, ?2)",
            params![mid, now()],
        ).map_err(|e| format!("init budget: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(mid)
    })
}

/// AYGENT REMOTE (R4): enqueue a remote-originated USER prompt. Origin
/// sentinel "remote:<turn_uuid>" — run_headless_turn frames it as the USER
/// speaking (via phone), not a peer/task message. Delivered immediately
/// (the 'continue:' due-time skip doesn't match this prefix). Fresh root +
/// budget per turn, same shape as enqueue_continue.
pub fn enqueue_remote(db: &Db, turn: &str, agent_id: &str, body: &str) -> Result<i64, String> {
    let (from_s, agent_s, body_s) = (format!("remote:{turn}"), agent_id.to_string(), body.to_string());
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        tx.execute(
            "INSERT INTO mailbox (from_agent,to_agent,body,root_id,depth,ancestry,status,created_at)
             VALUES (?1,?2,?3,0,6,'','pending',?4)",
            params![from_s, agent_s, body_s, now()],
        ).map_err(|e| format!("enqueue remote: {e}"))?;
        let mid = tx.last_insert_rowid();
        tx.execute("UPDATE mailbox SET root_id = ?1 WHERE id = ?1", params![mid])
            .map_err(|e| format!("set root: {e}"))?;
        tx.execute(
            "INSERT OR IGNORE INTO mailbox_budget (root_id, turns, cap, created_at) VALUES (?1, 0, 12, ?2)",
            params![mid, now()],
        ).map_err(|e| format!("init budget: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(mid)
    })
}

/// Pull the next pending message for a recipient (oldest first), marking it
/// delivered + incrementing the tree budget, all atomic. Returns None if the
/// recipient has no pending mail. The caller then runs it as a turn on the
/// recipient's lane. (Budget is charged at delivery, so a refused-past-cap
/// message is never processed.)
pub fn take_next_for(db: &Db, to_agent: &str) -> Result<Option<Message>, String> {
    let to_s = to_agent.to_string();
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        // task_continue: a self-wake row encodes its due time in from_agent as
        // "continue:<epoch_secs>". Skip rows not yet due ('continue:' is 9 chars,
        // substr is 1-based -> position 10). Other messages are always due.
        let row: Option<(i64, String, String, i64, i64, String)> = tx.query_row(
            "SELECT id, from_agent, body, root_id, depth, ancestry FROM mailbox
             WHERE to_agent = ?1 AND status = 'pending'
               AND (from_agent NOT LIKE 'continue:%' OR CAST(substr(from_agent, 10) AS INTEGER) <= ?2)
             ORDER BY id ASC LIMIT 1",
            params![to_s, now()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        ).optional().map_err(|e| format!("take next: {e}"))?;

        let Some((id, from_agent, body, root_id, depth, ancestry)) = row else {
            tx.commit().ok();
            return Ok(None);
        };

        // Charge the tree budget at delivery; if it's now over cap, drop instead.
        let (turns, cap): (i64, i64) = tx.query_row(
            "SELECT turns, cap FROM mailbox_budget WHERE root_id = ?1",
            params![root_id], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().map_err(|e| format!("budget: {e}"))?.unwrap_or((0, DEFAULT_BUDGET));
        if turns >= cap {
            tx.execute("UPDATE mailbox SET status = 'dead' WHERE id = ?1", params![id])
                .map_err(|e| format!("drop over-budget: {e}"))?;
            tx.commit().map_err(|e| format!("commit: {e}"))?;
            return Ok(None);
        }
        tx.execute("UPDATE mailbox_budget SET turns = turns + 1 WHERE root_id = ?1", params![root_id])
            .map_err(|e| format!("charge budget: {e}"))?;
        tx.execute("UPDATE mailbox SET status = 'delivered', delivered_at = ?2 WHERE id = ?1",
            params![id, now()]).map_err(|e| format!("mark delivered: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;

        Ok(Some(Message {
            id, from_agent, to_agent: to_s.clone(), body, root_id, depth, ancestry,
            status: "delivered".into(), created_at: now(),
        }))
    })
}

/// The list of OTHER agents this agent can message (id + name), so the model's
/// send_message tool knows valid recipients and the UI can show the roster.
pub fn pending_is_telegram(db: &Db, agent_id: &str) -> Result<bool, String> {
    let conn = db.reader()?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM mailbox WHERE to_agent = ?1 AND status = 'pending' AND from_agent LIKE 'telegram:%'",
        rusqlite::params![agent_id], |r| r.get(0)).map_err(|e| format!("pending_is_telegram: {e}"))?;
    Ok(n > 0)
}

pub fn roster(db: &Db, self_id: &str) -> Result<Vec<(String, String)>, String> {
    let conn = db.reader()?;
    let mut stmt = conn.prepare(
        "SELECT id, name FROM agent WHERE archived = 0 AND id != ?1 ORDER BY created_at ASC",
    ).map_err(|e| format!("prepare roster: {e}"))?;
    let rows = stmt.query_map(params![self_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("query roster: {e}"))?;
    let mut out = Vec::new();
    for r in rows { out.push(r.map_err(|e| format!("row: {e}"))?); }
    Ok(out)
}

/// Count pending inbound messages per agent (for a switcher badge).
pub fn pending_counts(db: &Db) -> Result<Vec<(String, i64)>, String> {
    let conn = db.reader()?;
    let mut stmt = conn.prepare(
        // Exclude task_continue wake-ups that aren't due yet (due epoch SECONDS
        // encoded in from_agent after the 9-char 'continue:' prefix) — otherwise
        // the drainer sees a phantom count and spins on its 1.5s poll.
        "SELECT to_agent, COUNT(*) FROM mailbox WHERE status = 'pending'
           AND (from_agent NOT LIKE 'continue:%' OR CAST(substr(from_agent, 10) AS INTEGER) <= strftime('%s','now'))
         GROUP BY to_agent",
    ).map_err(|e| format!("prepare counts: {e}"))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| format!("query counts: {e}"))?;
    let mut out = Vec::new();
    for r in rows { out.push(r.map_err(|e| format!("row: {e}"))?); }
    Ok(out)
}
