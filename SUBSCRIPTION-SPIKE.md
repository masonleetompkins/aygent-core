# Subscription Spike (Gent) — Claude + Codex in AYGENT

## Decision
Per-seat OAuth for Claude Pro / Max and ChatGPT Plus / Pro / Business / Edu / Enterprise,
same pattern as the existing Gemini demoknight(=OAuth seat, no API key).

## What changed (`src-tauri/src/`)
- **`subscription.rs`** — provider registry + parsers + math:
  - `is_subscription_provider("demoknight"|"codex"|"gemini")`
  - `parse_claude_usage` — reads `~/.config/Demoknight/projects/*.jsonl` (model, input/output/cache tokens)
  - `parse_codex_usage` — reads `~/.codex/sessions/**/*.jsonl` (rollout envelope + response.result.message.content[*].tokens)
  - `WindowUsage { used_pct, window_reset, raw_note }`, `window_usage_claude/codex` — live % via `*/usage` endpoints
  - `api_equivalent_cents` — Sonnet $3/$15 + GPT-5.2 $1.75/$14, 2.5x over-200k cache-write multiplier
- **`sub_loop.rs`** — portable loop helpers (UI-agnostic, daemon wires them in):
  - `usage_exhausted(pct_5h)` — hard-stop at ≥100%
  - `pick_failover(order, tried, exhausted)` — skip tried/exhausted, first surviving seat wins
  - `default_model_label` — opus for Claude, gpt-5.2 for codex
- **`lib.rs`** — module decls + `sub_cmds::*` Tauri command registration

## Auth (no API keys; reuse existing proved flows)
| Seat | Flow (already in-repo) | Credential |
|---|---|---|
| Claude Pro/Max | `claude setup-token` (like Gemini `demoknight auth_login`) | long-lived OAuth, `~/.config/Demoknight/.credentials.json` |
| Codex (ChatGPT) | `codex login` device flow (`openai/codex` repo) | `~/.codex/auth.json` (PKCE refresh) |

## Window semantics (verified)
- **Claude**: 5h rolling usage window; `*/usage` returns `five_hour.utilization`; resets hourly at :00.
  Over-limit error: HTTP 529 + `"usage-limit"`. Sonnet = 5h pool only (weekly pools are Opus-only).
- **Codex**: 5h rolling, `rate_limit` object per window (`limit`, `used_percent`, `reset_at`); GPT-5.2 caps by plan.

## Adaptive loop (daemon wiring)
1. Fast path: cheapest surviving seat first (own subscription = $0 marginal).
2. Poll `*/usage` (Claude) / `rate_limit` (Codex); reserve ~5% headroom for the reply.
3. On cross-threshold mid-run: finish claim → checkpoint → `pick_failover` → resume (state in SQLite, survives restart).
4. Heavy jobs: pre-split into sub-claims sized to the smallest surviving window; merge after.
5. Weekly-window note: prolonged Opus overuse can trigger "you're going to run into limits" — monitor, don't pre-throttle.
6. Failover order configurable; over-limit errors are routing signals, never user errors.

## Fallback chain ($-aware)
own Subscribed seats → env API keys (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) → Zen pay-as-you-go → clean stop, never stranded.

## Suggested UX (matches "How AYGENT pays for itself")
Settings → Seats: `Claude Max — 62% of 5h left · resets :00` / `ChatGPT Pro — 81% left`.
"Why Claude did this": because the 5h window had the most headroom, not preference.
Negotiator-downshift copy stays; add "your seat, your window."

## Verify
`cargo check --manifest-path core/src-tauri/Cargo.toml` → **clean, 0 errors** (31 warnings, mostly pre-existing;
new dead-code warnings on unwired helpers resolve when the daemon calls them).
