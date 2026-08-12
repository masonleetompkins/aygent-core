# Context-window accuracy fix — dynamic model metadata

## The bug (Mason, testing 1.0.1)
- Header showed 205k context window for **Opus 4.8**, which has a **1M** window.
- Root cause: `pricing.rs` is a HARDCODED substring table. `claude-opus-4-8`
  matches the generic `"opus"` rule → 200k (renders 205k after k-rounding).
- Same class of staleness: whoami reported "Opus 4" not "4.8" — an approximate
  id somewhere, not the real one.
- Mason's directive: FETCH details dynamically (models change!), don't hardcode.

## Ground truth (verified against the Anthropic SDK spec)
`GET /v1/models?beta=true` → `BetaModelInfo` per model carries:
- `id` — exact model id
- `display_name` — proper human name ("Claude Opus 4.8")
- **`max_input_tokens`** — the REAL context window (1,000,000 for Opus 4.8)
- `max_tokens` — max output tokens
The STANDARD `/v1/models` does NOT include max_input_tokens — must use `?beta=true`.

OpenAI/OpenRouter: `/models` does NOT reliably return a context window on OpenAI;
OpenRouter's `/models` DOES include `context_length` per model. So:
- OpenAI: no dynamic window endpoint → fall back to a small curated map, but the
  AUTHORITATIVE number is the usage we already stream (input tokens). We can't get
  the ceiling from OpenAI's API, so keep a minimal fallback ONLY for the denominator.
- OpenRouter: fetch `context_length` from its `/models` list (dynamic).
- Meta/Muse: `/models` shape unknown for context; fall back, window not critical.
- Local: already correct (reports the real ctx it ran with).

## Plan
1. Rust: `anthropic_model_info(model_id)` — hits `/v1/models?beta=true`, returns
   {context_window: max_input_tokens, display_name, max_output_tokens}. Cache it
   (per-process) so we don't refetch every render.
2. Rust: `openrouter` window from its `/models` `context_length`.
3. New/expanded command `chat_model_info(provider, model)` that:
   - anthropic → live beta models lookup (fallback to table on error/offline)
   - openrouter → live /models context_length
   - openai → curated fallback (no API field) — clearly the ONLY hardcoded bit,
     and documented as such
   - local → 0 (UI already uses the real reported window)
   PRICE stays a curated table (no API exposes price) — but the WINDOW is live.
4. UI: pass provider + model to chat_model_info; nothing else changes (it already
   prefers a backend-reported window from Usage for local).
5. whoami: make sure it reports the agent's ACTUAL configured model id verbatim.

## UI polish (Mason)
- Compact button: remove ⚡, text → "Compact Context", tooltip →
  "Save context by summarizing chat history with fewer tokens."

## Note on caching
Model metadata rarely changes within a session; cache per (provider, model) in a
process-global map with a short TTL so the meter is instant but still refreshes.
