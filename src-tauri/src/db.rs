// AYGENT — SQLite state spine (M1.1).
//
// WHY RUST-SIDE, NOT THE DAEMON:
//   The BUILD-SPEC diagram put SQLite "in the daemon." That was written before
//   the jail reality landed: the daemon runs under a macOS Seatbelt profile that
//   DENIES ambient fs — it literally cannot open() a DB file. Putting the DB
//   there would force either (a) every write across the broker (a security edge
//   that must not carry app-state traffic) or (b) an app-data hole in
//   folder-mode.sb (weakening the one load-bearing jail). Both trade security
//   for a diagram. The working code already made the right call: APP STATE lives
//   Rust-side; the daemon stays a pure jailed brain. This module honors that.
//
// WHY SQLITE OVER THE JSON FILES IT REPLACES:
//   Real transactions, the single-writer guarantee (Atlas C6), the
//   (owner_kind, owner_id) privacy boundary for shared pools (Atlas C5), and no
//   more read-whole-file / rewrite-whole-file races. SQLite is compiled IN
//   (rusqlite "bundled") — zero user setup, same philosophy as git2/llama.cpp.
//
// THREADING MODEL:
//   All WRITES funnel through ONE writer actor (see writer.rs) so there is
//   exactly one mutator; WAL lets reads run concurrently without blocking it.
//   This module owns connection setup + schema + migrations only.

use rusqlite::Connection;
use std::path::Path;

/// Current schema version. Bump when adding a migration step below.
pub const SCHEMA_VERSION: i64 = 2;

/// The DB file name under <app_data>.
pub const DB_FILE: &str = "aygent.db";

/// Open (creating if needed) the app-state database at <app_data>/aygent.db,
/// apply pragmas, and run migrations forward to SCHEMA_VERSION.
///
/// Pragmas (Atlas C6 / SQLite multi-writer hardening):
///   - journal_mode=WAL      -> readers never block the single writer.
///   - busy_timeout=5000     -> a contended write waits instead of erroring.
///   - foreign_keys=ON       -> cascade deletes keep the graph consistent.
///   - synchronous=NORMAL    -> safe with WAL, far faster than FULL.
pub fn open(app_data: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(app_data).map_err(|e| format!("mkdir app_data: {e}"))?;
    let path = app_data.join(DB_FILE);
    let conn = Connection::open(&path).map_err(|e| format!("open db: {e}"))?;

    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("pragma wal: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .map_err(|e| format!("busy_timeout: {e}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("pragma fk: {e}"))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| format!("pragma sync: {e}"))?;

    migrate(&conn)?;
    Ok(conn)
}

/// Open a READ connection with the same pragmas (WAL readers are concurrent).
/// Callers that only read use this; all writes go through the writer actor.
pub fn open_reader(app_data: &Path) -> Result<Connection, String> {
    let path = app_data.join(DB_FILE);
    let conn = Connection::open(&path).map_err(|e| format!("open db(r): {e}"))?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .map_err(|e| format!("busy_timeout: {e}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("pragma fk: {e}"))?;
    Ok(conn)
}

/// Read the stored user_version; 0 = brand-new db.
fn current_version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
        .map_err(|e| format!("read user_version: {e}"))
}

fn set_version(conn: &Connection, v: i64) -> Result<(), String> {
    conn.pragma_update(None, "user_version", v)
        .map_err(|e| format!("set user_version: {e}"))
}

/// Forward-only migration runner. Each step is idempotent-safe within a txn and
/// bumps user_version. Never rewrite an applied step — add a new one.
fn migrate(conn: &Connection) -> Result<(), String> {
    let mut v = current_version(conn)?;

    if v < 1 {
        conn.execute_batch(SCHEMA_V1)
            .map_err(|e| format!("migrate v1: {e}"))?;
        set_version(conn, 1)?;
        v = 1;
    }

    if v < 2 {
        // M1.4: mailbox (agent-to-agent messaging) + agent_context (uploaded
        // context docs index). mem_chunk gains no FK retroactively (SQLite can't
        // ALTER-ADD a FK), so we enforce mem_chunk cleanup in delete_agent's txn
        // instead (see repo::delete_agent). Both new tables are created fresh
        // WITH the agent FK + ON DELETE CASCADE.
        conn.execute_batch(SCHEMA_V2)
            .map_err(|e| format!("migrate v2: {e}"))?;
        set_version(conn, 2)?;
        v = 2;
    }

    let _ = v;
    Ok(())
}

/// SCHEMA v1 — the Phase-1 spine. Adapted from BUILD-SPEC Part B to the state
/// surface actually in use today (agents, conversations/messages, per-agent
/// settings). Checkpoints stay in git2 keyed by folder (NOT here) — the spec's
/// `checkpoint` index table is deferred until the timeline UI (M1.5) needs it.
///
/// PRIVACY BOUNDARY (Atlas C5): memory/session ownership uses (owner_kind,
/// owner_id), never one overloaded nullable column, so isolated-agent data can
/// never leak into a shared pool via a NULL-handling bug. mem_chunk is created
/// now (empty) so the shape is frozen before M1.7 fills it.
const SCHEMA_V1: &str = r#"
-- Agent profiles. Mirrors agents.rs::AgentProfile. The organizing unit.
CREATE TABLE IF NOT EXISTS agent (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  icon          TEXT NOT NULL DEFAULT '🤖',
  color         TEXT NOT NULL DEFAULT '#5b8cff',
  folder_path   TEXT NOT NULL DEFAULT '',
  model         TEXT NOT NULL DEFAULT '',
  provider      TEXT NOT NULL DEFAULT '',
  context_mode  TEXT NOT NULL DEFAULT 'isolated',  -- 'isolated' | 'shared:<poolId>'
  system_prompt TEXT NOT NULL DEFAULT '',
  created_at    INTEGER NOT NULL DEFAULT 0,
  updated_at    INTEGER NOT NULL DEFAULT 0,
  archived      INTEGER NOT NULL DEFAULT 0
);

-- Singleton row (id=0) holding app-wide pointers (active agent) + M1.4 knobs:
--   inter_agent_budget = max inter-agent turns per conversation chain before it
--     hard-stops (the runaway-cost backstop). Default 6.
--   max_concurrency    = max agents allowed to run headless turns at once.
--     Default 6.
CREATE TABLE IF NOT EXISTS app_state (
  id                  INTEGER PRIMARY KEY CHECK (id = 0),
  active_id           TEXT NOT NULL DEFAULT '',
  inter_agent_budget  INTEGER NOT NULL DEFAULT 6,
  max_concurrency     INTEGER NOT NULL DEFAULT 6
);
INSERT OR IGNORE INTO app_state (id, active_id, inter_agent_budget, max_concurrency) VALUES (0, '', 6, 6);

-- Conversations (chat threads). One agent owns many. `history` is the
-- provider-format message array the loop needs to continue; `msgs` is the
-- UI render list. Kept as JSON blobs (opaque to the backend) so the UI/loop
-- own their shape — same contract as conversations.rs, now transactional.
CREATE TABLE IF NOT EXISTS conversation (
  id         TEXT PRIMARY KEY,
  agent_id   TEXT NOT NULL,
  title      TEXT NOT NULL DEFAULT '',
  updated    INTEGER NOT NULL DEFAULT 0,
  pinned     INTEGER NOT NULL DEFAULT 0,
  ord        INTEGER NOT NULL DEFAULT 0,   -- manual sort key; 0 = unset
  msgs       TEXT NOT NULL DEFAULT '[]',   -- UI render list (JSON)
  history    TEXT NOT NULL DEFAULT '[]',   -- provider-format history (JSON)
  FOREIGN KEY (agent_id) REFERENCES agent(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_conv_agent ON conversation(agent_id);
CREATE INDEX IF NOT EXISTS idx_conv_updated ON conversation(updated DESC);

-- Per-agent settings (selected model/provider). Replaces the per-folder
-- settings.json. One row per agent; more keys can join as columns or a kv side
-- table later without a shape break.
CREATE TABLE IF NOT EXISTS agent_settings (
  agent_id   TEXT PRIMARY KEY,
  model      TEXT NOT NULL DEFAULT '',
  provider   TEXT NOT NULL DEFAULT '',
  FOREIGN KEY (agent_id) REFERENCES agent(id) ON DELETE CASCADE
);

-- Memory chunks — SHAPE FROZEN NOW, FILLED IN M1.7. Ownership via
-- (owner_kind, owner_id) so isolated vs pool queries are structurally distinct
-- (Atlas C5). Empty until the vault layer lands; created here so nothing
-- downstream has to migrate the shape in.
CREATE TABLE IF NOT EXISTS mem_chunk (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_kind    TEXT NOT NULL,          -- 'agent' | 'pool'
  owner_id      TEXT NOT NULL,
  source_path   TEXT NOT NULL,
  chunk_ordinal INTEGER NOT NULL,
  chunk_text    TEXT NOT NULL DEFAULT '',
  heading       TEXT,
  updated_at    INTEGER NOT NULL DEFAULT 0,
  UNIQUE(owner_kind, owner_id, source_path, chunk_ordinal)
);
"#;

/// SCHEMA v2 (M1.4) — inter-agent mailbox + per-agent context-document index.
/// Both carry an agent FK with ON DELETE CASCADE so deleting an agent cleans up
/// its messages + context rows (the orphan risk Atlas flagged). mem_chunk itself
/// predates the FK; delete_agent scrubs its rows in the same txn.
const SCHEMA_V2: &str = r#"
-- Inter-agent messages (Atlas B). A durable, ordered, inspectable mailbox.
-- Delivery = a NEW async turn on the recipient's lane (never synchronous).
-- depth = hop-count TTL (dropped at 0) to kill infinite ping-pong; root_id +
-- the tree budget row cap runaway cost; ancestry = cycle guard.
CREATE TABLE IF NOT EXISTS mailbox (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  from_agent    TEXT NOT NULL,
  to_agent      TEXT NOT NULL,
  body          TEXT NOT NULL DEFAULT '',
  root_id       INTEGER NOT NULL DEFAULT 0,   -- conversation-tree root (cost budget key)
  depth         INTEGER NOT NULL DEFAULT 0,   -- hops remaining (TTL)
  ancestry      TEXT NOT NULL DEFAULT '',     -- comma list of agent ids in this chain (cycle guard)
  status        TEXT NOT NULL DEFAULT 'pending', -- pending|delivered|dropped|dead
  created_at    INTEGER NOT NULL DEFAULT 0,
  delivered_at  INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY (to_agent) REFERENCES agent(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_mailbox_to ON mailbox(to_agent, status);
CREATE INDEX IF NOT EXISTS idx_mailbox_root ON mailbox(root_id);

-- Per-conversation-tree cost budget (Atlas B: the runaway-cost backstop).
-- One row per message tree; turns increments until it hits the cap, then
-- delivery is refused. Cheap, durable, inspectable.
CREATE TABLE IF NOT EXISTS mailbox_budget (
  root_id     INTEGER PRIMARY KEY,
  turns       INTEGER NOT NULL DEFAULT 0,
  cap         INTEGER NOT NULL DEFAULT 12,
  created_at  INTEGER NOT NULL DEFAULT 0
);

-- Per-agent uploaded context documents (Atlas C). Stored Rust-side in app-data
-- (OUT of the jail so they never pollute the folder's checkpoint stream). This
-- is the human-facing index; the extracted text lives chunked in mem_chunk
-- (owner_kind='agent') for M1.7 retrieval. `stored_path` is app-data-relative.
CREATE TABLE IF NOT EXISTS agent_context (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_id     TEXT NOT NULL,
  filename     TEXT NOT NULL,
  stored_path  TEXT NOT NULL,
  bytes        INTEGER NOT NULL DEFAULT 0,
  char_count   INTEGER NOT NULL DEFAULT 0,
  added_at     INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY (agent_id) REFERENCES agent(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_agent_context_agent ON agent_context(agent_id);
"#;
