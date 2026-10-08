// AYGENT — Subscription turn policy (slice 3, pure logic).
//
// Failover + exhausted-detection for the agent_stream wiring (lib.rs slice).
// Kept dependency-free so the policy is unit-testable without Tauri/HTTP:
//   - exhausted? -> try next profile of the same kind (personal -> work)
//   - usage 100% or explicit 429/quota error -> same path
// Streaming itself reuses provider.rs / openai_provider.rs with an OAuth
// bearer (wiring slice) — this file only decides WHEN to switch.

/// True when the error means "this profile's subscription window is spent"
/// (vs a real failure). Matched generously — a false positive just tries the
/// next profile, a false negative strands the user on an exhausted profile.
pub fn is_exhausted_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("429")
        || e.contains("rate_limited")
        || e.contains("rate limited")
        || e.contains("quota")
        || e.contains("usage")
        || e.contains("limit reached")
        || e.contains("messages limit")
        || e.contains("window")
        || e.contains("exhausted")
}

/// True when cached usage says the 5h window is spent.
pub fn usage_exhausted(pct_5h: Option<f64>) -> bool {
    matches!(pct_5h, Some(p) if p >= 100.0)
}

/// Pick the next profile id to try: first non-excluded profile of the same
/// kind, preferring one whose cached 5h is not exhausted. `order` is the
/// caller's failover order (label-sorted). Returns None when all tried.
pub fn pick_failover(order: &[String], tried: &[String], exhausted: &dyn Fn(&str) -> bool) -> Option<String> {
    for id in order {
        if tried.contains(id) {
            continue;
        }
        if exhausted(id) {
            continue;
        }
        return Some(id.clone());
    }
    // Second pass: spent is better than nothing (surfaces the real 429).
    order.iter().find(|id| !tried.contains(id)).cloned()
}

/// Display default models per subscription kind (v1: provider default).
/// Kept as a function so the Agents picker can show honest labels later.
pub fn default_model_label(kind: &str) -> &'static str {
    match kind {
        "claude-code" => "Auto (subscription default)",
        "codex" => "Auto (subscription default)",
        _ => "Auto",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_matches_429_and_quota() {
        assert!(is_exhausted_error("claude usage 429: too many"));
        assert!(is_exhausted_error("Messages limit reached"));
        assert!(is_exhausted_error("quota exceeded for codex"));
        assert!(!is_exhausted_error("connection refused"));
    }

    #[test]
    fn failover_skips_tried_and_spent() {
        let order = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let tried = vec!["a".to_string()];
        let spent = |id: &str| id == "b";
        assert_eq!(
            pick_failover(&order, &tried, &spent),
            Some("c".to_string())
        );
    }
}
