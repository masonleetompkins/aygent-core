// AYGENT — Cloud model PRICING + CONTEXT WINDOWS (context meter + $ cost).
//
// The chat UI needs two numbers the model itself never volunteers cleanly:
//   1. the model's CONTEXT WINDOW (tokens) — so we can show "62% of context
//      used" and warn BEFORE a turn is rejected for being too long, and
//   2. the model's PRICE ($/million tokens, in + out + cache) — so we can show
//      a running $ cost for the session.
//
// This is a CURATED, best-effort table keyed by substrings of the model id
// (e.g. "claude-3-5-sonnet", "gpt-4o", "opus"). It is deliberately conservative:
// an unknown model returns a sane default window + zero price (cost shows "—"
// rather than a wrong number). Prices are USD per 1,000,000 tokens and reflect
// published list prices at time of writing — they can drift, so they live here
// in ONE place, easy to update, and never block a turn if stale.
//
// catalog.rs::context_window handles LOCAL (GGUF) models; this handles the CLOUD
// providers (Anthropic / OpenAI / OpenRouter / Meta). Kept separate because the
// local path derives its window from the GGUF filename, not a price list.

/// A model's cost profile, USD per 1,000,000 tokens.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Price {
    /// Fresh input tokens (prompt), per Mtok.
    pub input: f64,
    /// Output tokens (completion), per Mtok.
    pub output: f64,
    /// Cache-READ input tokens, per Mtok (Anthropic bills ~10% of input).
    pub cache_read: f64,
    /// Cache-WRITE (creation) input tokens, per Mtok (Anthropic ~125% of input).
    pub cache_write: f64,
}

impl Price {
    const fn simple(input: f64, output: f64) -> Self {
        // Default Anthropic cache economics: read = 10% of input, write = 125%.
        Price { input, output, cache_read: input * 0.1, cache_write: input * 1.25 }
    }
    const fn zero() -> Self {
        Price { input: 0.0, output: 0.0, cache_read: 0.0, cache_write: 0.0 }
    }
    /// Dollar cost of one turn's token counts. Cache reads/writes are billed at
    /// their own rates; `input` here is FRESH (non-cached) input only.
    #[allow(dead_code)]
    pub fn cost(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
        let m = 1_000_000.0;
        (input as f64) * self.input / m
            + (output as f64) * self.output / m
            + (cache_read as f64) * self.cache_read / m
            + (cache_write as f64) * self.cache_write / m
    }
}

/// Full pricing + window record for a model.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ModelInfo {
    /// Context window in tokens (0 = unknown → UI shows a conservative default).
    pub context_tokens: u32,
    pub price: Price,
    /// True when this came from the curated table (vs a fallback guess).
    pub known: bool,
}

// Context windows are advertised in DECIMAL thousands (200,000 / 1,000,000) by
// every cloud provider — NOT binary. Using 1024 here made 200*K = 204,800 render
// as "205k" and 1000*K = 1,024,000 render as "1024k" (Mason caught both). K = 1000.
const K: u32 = 1000;

/// Look up a model's context window + price by its id (case-insensitive
/// substring match, most-specific patterns FIRST). Cloud providers only.
pub fn lookup(model_id: &str) -> ModelInfo {
    let id = model_id.to_ascii_lowercase();

    // (substring, context_tokens, Price) — ORDER MATTERS: specific → general.
    // Prices are USD / Mtok, list prices at time of writing.
    let table: &[(&str, u32, Price)] = &[
        // ── Anthropic Claude ──────────────────────────────────────────────
        // Claude 3.5 / 3.7 Sonnet: 200k window, $3 in / $15 out.
        ("claude-3-7-sonnet", 200 * K, Price::simple(3.0, 15.0)),
        ("claude-3-5-sonnet", 200 * K, Price::simple(3.0, 15.0)),
        ("claude-3-5-haiku",  200 * K, Price::simple(0.80, 4.0)),
        ("claude-3-opus",     200 * K, Price::simple(15.0, 75.0)),
        ("claude-3-sonnet",   200 * K, Price::simple(3.0, 15.0)),
        ("claude-3-haiku",    200 * K, Price::simple(0.25, 1.25)),
        // Newer families (Sonnet 4 / Opus 4 / Haiku 4 etc.) — 200k default,
        // priced like their tier. The 1M-context Sonnet beta still reports 200k
        // billing tiers here; we key the window off the id below when present.
        ("sonnet-4", 200 * K, Price::simple(3.0, 15.0)),
        ("opus-4",   200 * K, Price::simple(15.0, 75.0)),
        ("haiku-4",  200 * K, Price::simple(1.0, 5.0)),
        ("opus",     200 * K, Price::simple(15.0, 75.0)),
        ("haiku",    200 * K, Price::simple(0.80, 4.0)),
        ("sonnet",   200 * K, Price::simple(3.0, 15.0)),
        // ── OpenAI ────────────────────────────────────────────────────────
        ("gpt-4o-mini", 128 * K, Price::simple(0.15, 0.60)),
        ("gpt-4o",      128 * K, Price::simple(2.50, 10.0)),
        ("gpt-4.1-mini",  1024 * K, Price::simple(0.40, 1.60)),
        ("gpt-4.1",       1024 * K, Price::simple(2.0, 8.0)),
        ("gpt-4-turbo", 128 * K, Price::simple(10.0, 30.0)),
        ("gpt-4",        8 * K, Price::simple(30.0, 60.0)),
        ("gpt-3.5",     16 * K, Price::simple(0.50, 1.50)),
        ("o3-mini",    200 * K, Price::simple(1.10, 4.40)),
        ("o1-mini",    128 * K, Price::simple(1.10, 4.40)),
        ("o1",         200 * K, Price::simple(15.0, 60.0)),
        // ── Meta (Muse) / Llama-API ───────────────────────────────────────
        // Muse Spark family (1.1, 1.2, 1.2 contributor) = 1M context window.
        // FALLBACK only — chat_model_info tries muse_model_info first (dynamic).
        // Muse Spark = 1M window; per-tier pricing FROM META'S DOCS (verified
        // 2026-08-12, $/1M tokens). Tier is keyed by the EXACT id, so the
        // CONTRIBUTOR id (discounted) MUST match before the generic muse-spark.
        //   Standard (muse-spark-1.1/-1.2): cached 0.15 / in 1.25 / out 4.25
        //   Contributor (muse-spark-1.2-contributor): cached 0.002 / in 0.10 / out 0.20
        ("muse-spark-1.2-contributor", 1000 * K, Price { input: 0.10, output: 0.20, cache_read: 0.002, cache_write: 0.0 }),
        ("contributor",                1000 * K, Price { input: 0.10, output: 0.20, cache_read: 0.002, cache_write: 0.0 }),
        ("muse-spark",                 1000 * K, Price { input: 1.25, output: 4.25, cache_read: 0.15, cache_write: 0.0 }),
        ("muse",                       1000 * K, Price { input: 1.25, output: 4.25, cache_read: 0.15, cache_write: 0.0 }),
        ("llama",                       128 * K, Price::zero()),
    ];

    for (pat, ctx, price) in table {
        if id.contains(pat) {
            // A "-1m" / "1m" variant advertises a 1,000k window regardless of tier.
            let ctx = if id.contains("1m") { 1000 * K } else { *ctx };
            return ModelInfo { context_tokens: ctx, price: *price, known: true };
        }
    }

    // Unknown model: a conservative 128k window so the % meter still renders,
    // and zero price so we show "—" instead of a fabricated cost.
    ModelInfo { context_tokens: 128 * K, price: Price::zero(), known: false }
}

/// Just the context window (tokens) for a model id — convenience for callers
/// that only need the meter denominator.
#[allow(dead_code)]
pub fn context_window(model_id: &str) -> u32 {
    lookup(model_id).context_tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn muse_tiers_price_correctly() {
        // Standard tier: muse-spark-1.1 / -1.2 => in 1.25 / out 4.25 / cached 0.15
        let std12 = lookup("muse-spark-1.2");
        assert_eq!(std12.price.input, 1.25, "1.2 input");
        assert_eq!(std12.price.output, 4.25, "1.2 output");
        assert_eq!(std12.price.cache_read, 0.15, "1.2 cached");
        assert_eq!(std12.context_tokens, 1_000_000, "1.2 window");
        let std11 = lookup("muse-spark-1.1");
        assert_eq!(std11.price.input, 1.25, "1.1 input");
        // Contributor tier MUST NOT get standard pricing (order-sensitive).
        let contrib = lookup("muse-spark-1.2-contributor");
        assert_eq!(contrib.price.input, 0.10, "contributor input");
        assert_eq!(contrib.price.output, 0.20, "contributor output");
        assert_eq!(contrib.price.cache_read, 0.002, "contributor cached");
        assert_eq!(contrib.context_tokens, 1_000_000, "contributor window");
    }
    #[test]
    fn anthropic_fallback_window_is_decimal() {
        // Fallback table (dynamic API is primary): a Claude id still resolves
        // to a sane DECIMAL window, not a 1024-inflated one.
        let s = lookup("claude-3-5-sonnet");
        assert_eq!(s.context_tokens, 200_000);
    }
}
