// PRO HOOK (open core) — free tier, always.
//
// This file is a PUBLIC STUB. Pro builds (aygent-pro, private) REPLACE it at
// assemble time with the real entitlement implementation (license check vs the
// billing backend, tier unlocks, managed-model routing). Nothing proprietary
// ever lives in this repo; CI byte-verifies the stub is intact on main.
//
// Contract both sides honor: module path `crate::pro`, functions `is_pro()`
// and `tier_label()` with exactly these signatures.

/// Core builds are never Pro.
pub fn is_pro() -> bool {
    false
}

/// Human-readable tier for UI badges and logs.
pub fn tier_label() -> &'static str {
    "Core"
}

/// UI-callable tier snapshot: `{ tier, pro }`. Always Core here.
#[tauri::command]
pub fn pro_status() -> serde_json::Value {
    serde_json::json!({ "tier": tier_label(), "pro": is_pro() })
}

/// UI-callable refresh. Core has no backend: always false, never blocks.
#[tauri::command]
pub async fn pro_refresh() -> bool {
    is_pro()
}
