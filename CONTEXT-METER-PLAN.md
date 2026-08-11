# Context Meter + Cost + Compaction — SHIPPED (staging, local)

Mason's ask: near the context-window limit a long turn vanished ("prompt too
long"). Wanted: (1) in-chat COMPACTION, (2) per-message tokens+cost under the
timestamp, (3) a running context-% + running $ near the agent name.
Follow-up: cover ALL providers; local models = context tracking, NO cost.

## Root cause found
The provider stream code threw away each provider's `usage` block, so the app
never knew how full the context was. Fixed at the source in every provider.

## What shipped
### Backend (Rust) — compiles clean, 0 warnings
- `pricing.rs` (NEW): cloud model context windows + $/Mtok (Anthropic, OpenAI,
  OpenRouter, Meta). `lookup(model)` → {context_tokens, price, known}. Unknown
  model → 128k default window + $0 price (shows no cost, never a wrong number).
- `provider.rs`: new `StreamEvent::Usage { input, output, cache_read,
  cache_write, context_window }`. `context_window` is 0 for cloud (UI uses the
  pricing table) and the REAL memory-capped window for local models.

- USAGE PARSING — ALL PROVIDERS:
  - `provider.rs` (Anthropic): message_start.usage (input + cache read/creation)
    + message_delta.usage (output). Emits per streamed turn (main + tail + net).
  - `openai_provider.rs` (OpenAI + OpenRouter): adds
    `stream_options.include_usage`; parses the trailing usage frame
    (prompt_tokens / completion_tokens / prompt_tokens_details.cached_tokens).
    fresh input = prompt − cached; cache_read = cached.
  - `meta_provider.rs` (Muse): response.completed → response.usage
    (input_tokens / output_tokens / input_tokens_details.cached_tokens).
  - `local_provider.rs` (llama.cpp): decode() now returns (prompt_tokens,
    generated_tokens); local_stream_turn emits Usage with the REAL ctx window it
    ran with. No $ cost (local model paths don't match the price table → $0).

- `lib.rs`:
  - `chat_model_info(model)` command → window + price for the UI meter.
  - `conv_compact(id)` command → summarizes the model-facing `history` via the
    agent's own provider/model into a small 2-message seed; VISIBLE transcript
    (msgs) untouched. Returns the shrunk history; UI persists it.
  - `flatten_history_for_summary` helper (bounded head+tail).
  - both commands registered in the invoke_handler.

### Frontend (TS/React) — tsc + vite build clean
- `turns.ts`: `TurnUsage` type (+ optional `contextWindow`) + `usage` on
  `TurnState`; `accumulateUsage()` folds Usage events (input = latest round =
  context fill; output/cacheWrite sum; contextWindow captured for local).
- `Chat.tsx`:
  - assistant `Msg` carries `usage` (persists in the msgs JSON automatically).
  - fetches `chat_model_info`; PREFERS a backend-reported window (local) over
    the pricing-table window.
  - derives per-conv cost + context-fill %, folds LIVE turn usage so the meter
    moves during a turn.
  - HEADER (under the chat title, by the agent name): mini context bar + "N%
    context" (amber ≥75%, red ≥90%) + running "$" (hidden when $0, e.g. local)
    + a "⚡ Compact" button, and a red at-the-wall warning at ≥85%.
  - STAMP (under each message timestamp): "N.Nk tok" + "$cost" for that turn
    (cost line hidden for local / unknown-price models).

## Provider coverage summary
| Provider              | Tokens | Context % | $ Cost |
|-----------------------|:------:|:---------:|:------:|
| Anthropic             |   ✓    |    ✓      |   ✓    |
| OpenAI                |   ✓    |    ✓      |   ✓    |
| OpenRouter            |   ✓*   |    ✓      |   ✓*   |
| Meta (Muse)           |   ✓    |    ✓      |  $0    |
| Local (llama.cpp GGUF)|   ✓    |    ✓ (real capped window) | — (by design) |

\* OpenRouter: depends on the upstream honoring include_usage; degrades to 0
   gracefully (meter just won't move for a model that doesn't report it).

## Follow-ups / notes for later
- Compaction is manual (a button). Could auto-offer at ≥90%.
- Prices are a curated table in pricing.rs — update as list prices drift.
- OpenRouter per-model cost accuracy depends on our price table matching the
  underlying model id substring; unknown ids show tokens + % but no cost.

## Build status
- `cargo check` — clean (0 warnings).
- `npx tsc --noEmit` — clean.
- `npm run build` — clean.
NOT yet: dev smoke test in the running app, and NOT committed to GitHub.
Waiting on Mason to eyeball in dev first.
