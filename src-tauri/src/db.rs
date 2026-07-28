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
pub const SCHEMA_VERSION: i64 = 5;

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

    if v < 3 {
        // M1.7 Slice 1 — Vault-native memory READ PATH. note+link+vec are a
        // DERIVED CACHE over the markdown vault (source of truth = files). Fully
        // rebuildable: delete these rows and re-ingest and nothing is lost
        // (ethos: user owns the files). See memory.rs.
        conn.execute_batch(SCHEMA_V3)
            .map_err(|e| format!("migrate v3: {e}"))?;
        set_version(conn, 3)?;
        v = 3;
    }

    if v < 4 {
        // M1.8 SCHEDULER — per-agent cron/heartbeat/interval that fires headless
        // turns. Atlas: "the drainer with a clock in front of it." schedule holds
        // the timing (typed spec as JSON + a derived next_fire_at the ticker
        // sorts on) + action + guardrails; schedule_run is the durable run log
        // for observability (last/next/status/history). See scheduler.rs.
        conn.execute_batch(SCHEMA_V4)
            .map_err(|e| format!("migrate v4: {e}"))?;
        set_version(conn, 4)?;
        v = 4;
    }

    if v < 5 {
        // M1.8 Slice 4: global scheduler kill switch. One bool on the app_state
        // singleton (no new table for a single flag). ALTER ADD is fine here —
        // no FK/constraint change. Existing row keeps default 0 (not paused).
        conn.execute_batch(SCHEMA_V5)
            .map_err(|e| format!("migrate v5: {e}"))?;
        set_version(conn, 5)?;
        v = 5;
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

/// SCHEMA v3 (M1.7 Slice 1) — vault-native memory READ PATH.
///
/// These three tables are a DERIVED INDEX over the markdown vault. The vault
/// files are the source of truth; every row here is rebuildable by re-ingesting.
/// Ownership uses (owner_kind, owner_id) — same C5 privacy boundary as mem_chunk
/// — so an isolated agent's graph can never leak into a pool via a NULL bug.
///
/// `note`  = one row per markdown file: its type/agent/pool (from frontmatter),
///           a content hash (skip re-embed when unchanged), and status.
/// `link`  = the backlink graph: (src -> dst, kind). kind = wikilink|embed|tag
///           |suggested|supersedes. Queryable from BOTH ends = graph expansion.
/// `vec`   = the embedding per note (nomic-embed, 768-dim, local). Stored as a
///           raw little-endian f32 BLOB; cosine is computed in Rust for Slice 1
///           (sqlite-vec KNN is a drop-in upgrade later — shape already fits).
const SCHEMA_V3: &str = r#"
-- One row per ingested markdown note. Derived from the file; source of truth is
-- the file on disk. sha lets ingest skip unchanged files (no needless re-embed).
CREATE TABLE IF NOT EXISTS note (
  owner_kind  TEXT NOT NULL,            -- 'agent' | 'pool'
  owner_id    TEXT NOT NULL,
  path        TEXT NOT NULL,            -- vault-relative path (the note's identity)
  sha         TEXT NOT NULL DEFAULT '',-- content hash of the raw file bytes
  title       TEXT NOT NULL DEFAULT '',
  ntype       TEXT NOT NULL DEFAULT 'note', -- frontmatter `type`
  agent       TEXT NOT NULL DEFAULT '',     -- frontmatter `agent` (provenance)
  pool        TEXT NOT NULL DEFAULT '',     -- frontmatter `pool` (null-> '')
  confidence  REAL NOT NULL DEFAULT 0.0,
  status      TEXT NOT NULL DEFAULT 'active',
  body        TEXT NOT NULL DEFAULT '',     -- markdown body (frontmatter stripped)
  created     TEXT NOT NULL DEFAULT '',
  updated     TEXT NOT NULL DEFAULT '',
  indexed_at  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (owner_kind, owner_id, path)
);
CREATE INDEX IF NOT EXISTS idx_note_owner ON note(owner_kind, owner_id);
CREATE INDEX IF NOT EXISTS idx_note_type  ON note(owner_kind, owner_id, ntype);

-- The backlink graph. src/dst are vault-relative note paths (dst may be
-- unresolved = a dangling link; we still record it so de-orphaning can see it).
CREATE TABLE IF NOT EXISTS link (
  owner_kind  TEXT NOT NULL,
  owner_id    TEXT NOT NULL,
  src_path    TEXT NOT NULL,
  dst_path    TEXT NOT NULL,
  kind        TEXT NOT NULL DEFAULT 'wikilink', -- wikilink|embed|tag|suggested|supersedes
  PRIMARY KEY (owner_kind, owner_id, src_path, dst_path, kind)
);
CREATE INDEX IF NOT EXISTS idx_link_src ON link(owner_kind, owner_id, src_path);
CREATE INDEX IF NOT EXISTS idx_link_dst ON link(owner_kind, owner_id, dst_path);

-- One embedding per note. dim + a raw f32 LE blob; cosine in Rust for Slice 1.
CREATE TABLE IF NOT EXISTS vec (
  owner_kind  TEXT NOT NULL,
  owner_id    TEXT NOT NULL,
  path        TEXT NOT NULL,
  dim         INTEGER NOT NULL DEFAULT 0,
  embedding   BLOB NOT NULL,
  PRIMARY KEY (owner_kind, owner_id, path)
);
"#;

/// SCHEMA v4 (M1.8) — the SCHEDULER. Two tables. Timing-critical fields
/// (next_fire_at, enabled) are real indexed columns the ticker queries hot; the
/// evolving typed enums (ScheduleSpec, ScheduleAction) ride as JSON so adding a
/// variant later needs NO migration (serde forward-compat). All timestamps are
/// UTC epoch MILLISECONDS (INTEGER); local-tz interpretation lives in spec+tz.
const SCHEMA_V4: &str = r#"
-- One row per user schedule. `spec_json` (Interval|DailyAt|WeeklyAt) is the
-- source of truth for recomputation; `next_fire_at` is the derived index the
-- ticker sorts on. `action_json` = AgentTurn{prompt,context} | SystemJob{kind}.
CREATE TABLE IF NOT EXISTS schedule (
  id                     INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_id               TEXT    NOT NULL,
  name                   TEXT    NOT NULL,
  kind                   TEXT    NOT NULL,            -- 'cron' | 'heartbeat' | 'interval'
  spec_json              TEXT    NOT NULL,            -- serialized ScheduleSpec
  tz                     TEXT    NOT NULL DEFAULT 'local',
  action_json            TEXT    NOT NULL,            -- serialized ScheduleAction
  enabled                INTEGER NOT NULL DEFAULT 1,
  catch_up_policy        TEXT    NOT NULL DEFAULT 'coalesce', -- coalesce|skip|fire_each
  max_fires_per_day      INTEGER NOT NULL DEFAULT 2,
  max_cost_units_per_day INTEGER,                     -- NULL = no ceiling
  next_fire_at           INTEGER NOT NULL,            -- UTC ms; ticker sorts on this
  last_fired_at          INTEGER,                     -- UTC ms; NULL until first fire
  daily_fire_count       INTEGER NOT NULL DEFAULT 0,
  cost_units_today       INTEGER NOT NULL DEFAULT 0,
  count_reset_day        INTEGER NOT NULL DEFAULT 0,  -- local YYYYMMDD of last reset
  created_at             INTEGER NOT NULL DEFAULT 0,
  updated_at             INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY (agent_id) REFERENCES agent(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_schedule_due ON schedule (enabled, next_fire_at);

-- Durable run log — the observability spine. Every fire AND every skip (with
-- reason) gets a row, so "why didn't it fire?" always has an on-screen answer.
CREATE TABLE IF NOT EXISTS schedule_run (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  schedule_id     INTEGER NOT NULL,
  agent_id        TEXT    NOT NULL,
  fired_at        INTEGER NOT NULL,      -- when the ticker decided to fire (UTC ms)
  started_at      INTEGER,               -- when the drainer began the turn
  finished_at     INTEGER,
  state           TEXT    NOT NULL,      -- queued|running|ok|error|skipped
  skip_reason     TEXT,                  -- rate_limited|paused|cost_ceiling
  cost_units      INTEGER,
  result_snippet  TEXT,
  conversation_id TEXT,
  FOREIGN KEY (schedule_id) REFERENCES schedule(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_run_by_schedule ON schedule_run (schedule_id, fired_at DESC);
"#;

/// SCHEMA v5 (M1.8 Slice 4) — global scheduler pause (kill switch). A single
/// bool on the app_state singleton; the ticker checks it every tick and fires
/// nothing while set. Durable so a pause survives relaunch (schedules never
/// silently resume). ALTER ADD COLUMN is forward-only + safe (no constraint).
const SCHEMA_V5: &str = r#"
ALTER TABLE app_state ADD COLUMN scheduler_paused INTEGER NOT NULL DEFAULT 0;
"#;
