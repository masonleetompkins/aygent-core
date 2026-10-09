# Subscription providers — wiring checklist (Core feat/subscription-providers)

Slice 1: `subscription.rs` (profiles, CLI import, 5h usage cached 90s, api-equiv)
Slice 2: `sub_cmds.rs` (5 commands, profiles in Keychain slot sub:profiles — NO db yet)
Slice 3: `SubProfiles.tsx` (Settings section UI) + `sub_loop.rs` (failover policy, tested)

## The 5-line lib.rs diff (paste locally, then cargo check)
mod subscription; mod sub_cmds; mod sub_loop;
invoke_handler: sub_profiles_list, sub_profile_create, sub_profile_delete,
sub_profile_import_cli, sub_usage.
Settings Providers card: import SubProfiles + render <SubProfiles />.

## Still to wire (full checkout)
1. db.rs v16: run subscription_schema.sql + ALTER TABLE agent ADD COLUMN
   subscription_profile TEXT NOT NULL DEFAULT '';
2. agent_stream: provider claude-code|codex -> profile tokens, Messages/Responses
   with OAuth bearer (anthropic-beta oauth-2025-04-20 / ChatGPT bearer), Usage
   billing_mode=subscription + subscription_turn row, bill $0.
   Failover via sub_loop::pick_failover (personal->work).
3. Agents form: provider options + profile picker. Chat footer: Subscription
   62pct resets 14:20 12.4k tok (~$1.23 API value).
4. Live verify (both CLIs logged in): token-loop spends sub credits, usage
   endpoints parse, 429 fails over.

## Pro
usage_events.source=subscription, wallet untouched, bypass tier_lock + $25/d.
Proxy never sees these turns. Entitlement unchanged — Core AND Pro.
