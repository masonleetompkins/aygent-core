# Subscription providers — wiring checklist (Core feat/subscription-providers)

 landed: `src-tauri/src/subscription.rs` (profiles, CLI import, 5h usage cached 90s, api-equiv)
 landed: `src-tauri/src/subscription_schema.sql` (v16 tables)

 ## Still to wire (needs full checkout + cargo check — lib.rs/db.rs exceed API edit size)
 1. `lib.rs`: add `mod subscription;` + commands:
    - `sub_profiles_list(kind) -> [{id,kind,label,has_token,pct_5h,reset_at_ms,weekly_pct}]`
    - `sub_profile_create(kind,label) -> {id}` (writes subscription_profile row)
    - `sub_profile_delete(id)` (row + `keychain::delete_key(sub:id)`)
    - `sub_profile_import_cli(id)` (import_cli(kind) -> keychain::set_key(slot, json))
    - `sub_profile_connect(kind)` v1 = instructions to run `claude login` / `codex login`
      in terminal, then import_cli. (Native OAuth later.)
    - `sub_usage(id)` (keychain tokens -> subscription::usage_cached, 90s TTL)
 2. `db.rs`: SCHEMA_VERSION 15->16, run subscription_schema.sql + jullie:
    `ALTER TABLE agent ADD COLUMN subscription_profile TEXT NOT NULL DEFAULT '';`
 3. `agent_stream`: when provider is claude-code|codex, resolve profile tokens,
    call Messages/Responses with OAuth headers (Bearer + anthropic-beta
    oauth-2025-04-20 / ChatGPT bearer), emit Usage with billing_mode=subscription,
    insert subscription_turn row (api_equivalent_cents), bill $0.
    Failover: on 429/exhausted try next profile same kind, surface notice.
 4. UI: Settings Providers += Claude Code + Codex (profiles, Import/Connect/Test,
    5h bar); Agents form += provider options + profile picker; Chat footer +=
    `Subscription · 62% · resets 14:20 · 12.4k tok (~$1.23 API value)`.
 5. Live verify on Mason's machine (both CLIs logged in): token-loop uses sub
    credits (not API $), usage endpoints parse, 429 paths failover.

 ## Pro (feat/subscription-providers)
 - `usage_events.source='subscription'`, wallet untouched, bypass tier_lock + $25/d.
 - model_registry note + proxy-config: never meter subscription turns.
