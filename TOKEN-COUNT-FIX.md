# Token-count accuracy fix — the cached-tokens bug

## Symptoms (Mason, 2026-08-12, Opus 4.8)
1. Top meter shows "2 / 1M" used — absurdly low for a real chat.
2. Top meter number != per-message stamp number (19 tok, 5.9k tok).
3. Bar stays full regardless of actual fill.
4. Long chat shows $74.40 — wildly inflated cost.

## ROOT CAUSE (proven live)
Real Anthropic usage for a prompt w/ a 4000-token cached system:
  input_tokens: 14
  cache_creation_input_tokens: 4004   <- the real context, IGNORED by the meter
  cache_read_input_tokens: 0          <- huge on later turns
  output_tokens: 10

My code treated `input` = `input_tokens` ONLY. With prompt caching on (which we
use), almost the entire prompt is billed as cache_creation (first time) or
cache_read (subsequent), and the fresh `input_tokens` field is tiny (single
digits). So:
- "used" tokens = fresh input only => showed 2/14 instead of ~4000+.
- The meter (fresh input) and the stamp (input+output) used DIFFERENT formulas
  => two different numbers.
- Cost summed every turn's usage, and each turn re-counts the whole cached
  history; billing cached tokens at FULL input price across N turns => $74.40.

## THE FIX
### Definition
"Context tokens used this turn" = input_tokens + cache_read_input_tokens +
cache_creation_input_tokens. That's the true size of what the model processed =
the window fill.

### Backend (provider.rs) — already emits input/output/cache_read/cache_write.
Those fields are RIGHT (cache_write = cache_creation). The UI just has to ADD
them to get context fill. No backend change strictly needed, BUT: make the
Usage event also carry a `context_input` = in + cr + cw so the UI has one clean
number and can't re-derive it wrong. (Add field; keep the components for cost.)

### UI (Chat.tsx + turns.ts)
1. CONTEXT FILL (top meter) = latest turn's (input + cacheRead + cacheWrite),
   NOT input alone. This is the denominator-fill.
2. PER-MESSAGE STAMP "tokens" = same context-input + output for that turn, so it
   MATCHES the top when it's the latest turn. Currently stamp = input+output
   (missing cache) => mismatch. Fix both to include cache.
3. COST: bill each component at its own rate (already the formula) — the bug is
   the fill number, not the cost formula per se, BUT the $74.40 came from
   cache_creation being counted as fresh `input` price historically? No — check:
   cost currently uses u.input * price.input + u.cacheRead*cache_read +
   u.cacheWrite*cache_write. If cacheWrite (creation) was 0 in the old data and
   everything landed in `input`, that's fine. The inflation is because EVERY
   turn's input re-includes the full prior history as fresh input in the
   provider `input_tokens`? NO — caching means it's cache_read at 10%. Re-derive
   after the fill fix; the cost should drop dramatically once cache_read is
   priced at 10% (Anthropic) instead of... verify price.cache_read is set.
4. BAR: ctxPct = contextFill / window. Once contextFill is right, the bar tracks.

### Bar-stays-full
The bar width uses ctxPct. With ctxTokens=2 and window=1M, ctxPct rounds to 0 —
so the bar should be EMPTY, not full. The screenshot shows it full => the bar is
reading a DIFFERENT value than the text, OR min-width. CHECK the bar's width
binding: `width: ${ctxPct}%`. 0% => empty. If it shows full, ctxPct is 100 =>
ctxTokens >= window on some path (maybe lastInput from a stale big number).
Must reconcile: ONE contextFill value feeds BOTH the text and the bar.

## Verify after fix
- Fresh "Hey Cleo" turn: top ≈ few-k tokens (system+tools+soul), bar a sliver,
  cost a few cents. Stamp on the reply == top (latest turn).
- Long chat: top = last turn's real input (grows), cost realistic (cached reads
  cheap), bar proportional.
