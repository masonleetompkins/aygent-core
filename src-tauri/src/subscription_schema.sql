-- AYGENT subscription providers — SCHEMA v16 (forward-only).
-- Profiles are metadata only; OAuth secrets live in the Keychain vault
-- under slot sub:<profile_id> (see subscription.rs key_slot).

CREATE TABLE IF NOT EXISTS subscription_profile (
  id          TEXT PRIMARY KEY,            -- e.g. sub_xxx (nanoid)
  kind        TEXT NOT NULL,               -- 'claude-code' | 'codex'
  label       TEXT NOT NULL DEFAULT '',    -- 'Personal' | 'Work'
  created_at  INTEGER NOT NULL DEFAULT 0,
  updated_at  INTEGER NOT NULL DEFAULT 0
);

-- Per-turn subscription log: $0 billed, tokens + api-equiv kept for display.
CREATE TABLE IF NOT EXISTS subscription_turn (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_id          TEXT NOT NULL,
  profile_id        TEXT NOT NULL,
  kind              TEXT NOT NULL,
  input_tokens      INTEGER NOT NULL DEFAULT 0,
  output_tokens     INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  api_equiv_cents   INTEGER NOT NULL DEFAULT 0,
  created_at        INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY (agent_id) REFERENCES agent(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_sub_turn_agent ON subscription_turn(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_sub_turn_profile ON subscription_turn(profile_id, created_at DESC);
