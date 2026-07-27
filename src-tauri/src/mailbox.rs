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

    // Resolve the parent chain (root_id, depth, ancestry) if this is a reply.
    let (root_id_opt, depth, ancestry_in): (Option<i64>, i64, String) = if parent_id > 0 {
        let conn = db.reader()?;
        conn.query_row(
            "SELECT root_id, depth, ancestry FROM mailbox WHERE id = ?1",
            params![parent_id],
            |r| Ok((Some(r.get::<_, i64>(0)?), r.get::<_, i64>(1)?, r.get::<_, String>(2)?)),
        ).optional().map_err(|e| format!("parent lookup: {e}"))?
         .unwrap_or((None, DEFAULT_DEPTH, String::new()))
    } else {
        (None, DEFAULT_DEPTH, String::new())
    };

    // Depth TTL: a reply gets parent.depth - 1.
    let new_depth = if parent_id > 0 { depth - 1 } else { DEFAULT_DEPTH };
    if new_depth <= 0 {
        return Ok(SendResult::Refused { reason: "message hop limit reached (loop guard)".into() });
    }

    // Cycle guard: if the recipient already appears in the ancestry, drop it.
    let ancestry_list: Vec<&str> = ancestry_in.split(',').filter(|s| !s.is_empty()).collect();
    if ancestry_list.contains(&to_agent) {
        return Ok(SendResult::Refused { reason: "cycle detected (recipient already in this chain)".into() });
    }
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

/// Pull the next pending message for a recipient (oldest first), marking it
/// delivered + incrementing the tree budget, all atomic. Returns None if the
/// recipient has no pending mail. The caller then runs it as a turn on the
/// recipient's lane. (Budget is charged at delivery, so a refused-past-cap
/// message is never processed.)
pub fn take_next_for(db: &Db, to_agent: &str) -> Result<Option<Message>, String> {
    let to_s = to_agent.to_string();
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        let row: Option<(i64, String, String, i64, i64, String)> = tx.query_row(
            "SELECT id, from_agent, body, root_id, depth, ancestry FROM mailbox
             WHERE to_agent = ?1 AND status = 'pending' ORDER BY id ASC LIMIT 1",
            params![to_s], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
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
        "SELECT to_agent, COUNT(*) FROM mailbox WHERE status = 'pending' GROUP BY to_agent",
    ).map_err(|e| format!("prepare counts: {e}"))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| format!("query counts: {e}"))?;
    let mut out = Vec::new();
    for r in rows { out.push(r.map_err(|e| format!("row: {e}"))?); }
    Ok(out)
}
