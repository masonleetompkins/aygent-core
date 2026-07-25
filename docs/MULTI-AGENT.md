# Multi-Agent Architecture — Design (Phase 1 core)

_Freeze-first doc (project discipline: freeze the shape before churning code). Implements
BUILD-SCOPE §10. Started 2026-07-25._

## The one idea
**An Agent is just a config profile.** Everything composes around that. The daemon runs one
process; many agent profiles live inside it. Switching agents changes what you *view*; all
agents *run* independently (each a session lane) — enabled nearly free by the
Provider/Model/Runtime/Channel layer split.

## The migration problem (and the answer)
Today EVERYTHING is keyed by `folder_key(path)` — conversations, settings, tools-enabled,
tools-config, checkpoints. That made "folder" the implicit organizing unit.

Multi-agent makes **the AgentProfile the unit**, and a profile *has* a folder scope. So:

- **New key = `agentId`** (a stable ULID-ish string), NOT the folder hash.
- Per-agent stores move from `<store>/<folder_key>.json` → `<store>/<agentId>/...`.
- A profile records its `folderPath`; the broker still jails by that path (the security
  kernel is unchanged — it always jailed by path, never by "agent").
- **Back-compat:** on first launch after this lands, if a legacy `agent-folder.json` +
  per-folder stores exist, we synthesize **Agent 1** ("My Agent") pointing at that folder and
  migrate its conversations/settings/tools under the new `agentId`. Zero data loss, zero user
  action.

## The AgentProfile shape (FROZEN — changes need a migration note here)
```rust
// src-tauri/src/agents.rs
pub struct AgentProfile {
    pub id: String,             // stable id (ULID-ish); the NEW key for all per-agent state
    pub name: String,           // "Work", "Journal", "Research"
    pub icon: String,           // emoji or short glyph for the switcher
    pub color: String,          // "#rrggbb" accent for the switcher chip

    pub folder_path: String,    // WHICH folder/subfolder this agent is jailed to (broker scope)
    pub model: String,          // its own model id ("" = auto). LOCAL = absolute .gguf path
    pub provider: String,       // "" | "anthropic" | "openai" | "openrouter" | "local"

    pub context_mode: String,   // "isolated" | "shared:<poolId>"   (§10.3; pools = later sub-step)
    pub system_prompt: String,  // its own personality/instructions (overrides/extends AYGENT.md)

    pub created_at: i64,        // epoch seconds
    pub updated_at: i64,
    pub archived: bool,         // soft-hide from switcher without deleting state
}
```
JSON (serde) uses the same field names (snake_case) so the on-disk shape is self-describing.

## Storage layout
```
<app_data>/agents/
  index.json                      -> { "agents": [AgentProfile...], "activeId": "<id>" }
  <agentId>/
    conversations/ ...            (moved from settings-scheme conversations, re-keyed)
    settings.json                 (the FolderSettings: model/provider — now per agent)
    tools/enabled.json
    tools/config.json
```
Checkpoints stay keyed by **folder path** (a shadow git repo per real folder) — correct,
because two agents scoped to the SAME folder must share ONE checkpoint history + the
folder-wide write lock (CONTRACTS §4). Agent identity does not fork the git timeline.

## Commands (Tauri) — added, existing ones gain an agentId
```
agents_list() -> { agents: [AgentProfile], activeId }
agents_create(name, icon, color, folder_path, model, provider, context_mode, system_prompt) -> AgentProfile
agents_update(profile) -> ()
agents_delete(id) -> ()            // also drops per-agent state (keeps folder + checkpoints)
agents_set_active(id) -> ()
agents_get_active() -> AgentProfile | null
```
Existing per-folder commands (conv_*, settings, tools_*, agent_stream) shift from a `folder`
arg to an `agentId` arg; the daemon resolves `agentId -> folderPath` for the broker scope.
**Back-compat window:** keep accepting `folder` where trivial; new UI sends `agentId`.

## UI (§10.2)
- **Agent switcher** = a left rail of agent chips (icon + color), Slack-workspace style, above
  the existing sidebar. Click = set active + reload that agent's conversations/settings/tools.
- "+ New Agent" opens a small create form (name, icon, color, pick folder, model, provider).
- Per-agent settings live in the existing Settings screen, now scoped to the active agent.

## Phasing (honest, from §10.6)
- **This sub-step (now):** profile store + back-compat migration + switcher UI + per-agent
  model/folder/conversations/tools/settings. Multiple **isolated** agents, run-concurrently
  structurally (daemon lanes already exist per session).
- **Next sub-step:** shared context pools (`shared:<poolId>`) + per-agent schedule/heartbeat
  (rides on the Scheduler UI work) — both are fields already on the profile, so they're
  config lookups, not re-architecture.

## What we DON'T touch
- The broker / Seatbelt jail (still path-scoped — unchanged; the security proof holds).
- The checkpoint git model (folder-scoped, shared across agents on that folder — correct).
- The frozen contracts (ToolDef / Broker RPC / Capability enum / folder-lock) — profiles sit
  ABOVE them.
