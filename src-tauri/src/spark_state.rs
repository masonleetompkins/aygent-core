// AYGENT — SPARK STATE (the jailed KV persistence for interactive Sparks).
//
// WHY THIS EXISTS (Mason, 2026-08-12): Sparks were purely cosmetic — buttons
// didn't click, checklists were dead, and nothing survived a tab switch. Root
// cause: the Spark iframe runs in an OPAQUE origin (sandbox="allow-scripts",
// blob: URL, no allow-same-origin — the correct fail-closed isolation). In an
// opaque origin, ANY access to localStorage/sessionStorage/cookies THROWS, so a
// model's `localStorage.getItem(...)` on the first line aborted the whole script
// before a single event handler wired up.
//
// THE FIX keeps the sandbox exactly as isolated (no jail weakening — see
// SPARKS-INTERACTIVITY-FIX.md for why allow-same-origin on a same-origin blob is
// a jail ESCAPE). The Spark's injected runtime shims localStorage so it never
// throws, and persists via postMessage to the host, which lands here: a tiny
// jailed KV store at Sparks/<slug>/state.json — the SAME broker jail as every
// file tool. The Spark never touches disk directly; the host mediates, exactly
// like fetch_url mediates the network.
//
// SHAPE: state.json is a flat JSON object { key: <json value>, ... }. Values are
// arbitrary JSON (the shim's localStorage stores strings; window.spark.set can
// store any JSON). Bounded so a runaway Spark can't fill the disk.

use std::sync::Arc;
use crate::broker::{self, Broker};

/// Max size of a single Spark's state.json (256 KB). A KV store for a mini-app's
/// UI state — checklists, counters, small lists — never a database. Anything
/// bigger belongs in a real file the agent writes.
const MAX_STATE_BYTES: usize = 256 * 1024;

fn slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 80
        && slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Read a Spark's whole KV blob (jailed). Returns `{}` when the Spark has no
/// state yet — a fresh Spark reads empty, never errors.
pub fn read(broker: &Arc<Broker>, agent_id: &str, slug: &str) -> Result<serde_json::Value, String> {
    if !slug_ok(slug) { return Err("invalid spark name".into()); }
    let rel = format!("Sparks/{slug}/state.json");
    // Resolve READ; a missing file is not an error (empty state).
    match broker.resolve_and_open(agent_id, &rel, broker::Mode::Read) {
        Ok(mut f) => {
            use std::io::Read;
            let mut s = String::new();
            f.read_to_string(&mut s).map_err(|e| format!("read state: {e}"))?;
            if s.trim().is_empty() { return Ok(serde_json::json!({})); }
            serde_json::from_str(&s).or_else(|_| Ok(serde_json::json!({})))
        }
        // NotFound / not-yet-created → empty. Any jail refusal is a real error.
        Err(broker::BrokerError::NotFound) => Ok(serde_json::json!({})),
        Err(e) => {
            // resolve_and_open on a missing file returns Io(ENOENT) too — treat
            // "no such file" as empty state, surface anything else.
            let es = format!("{e:?}");
            if es.contains("NotFound") || es.contains("No such file") || es.contains("errno Some(2)") {
                Ok(serde_json::json!({}))
            } else {
                Err(format!("refused by jail: {es}"))
            }
        }
    }
}

/// Overwrite a Spark's whole KV blob (jailed). `values` must be a JSON object.
/// Bounded by MAX_STATE_BYTES. Creates Sparks/<slug>/ if needed.
pub fn write(broker: &Arc<Broker>, agent_id: &str, slug: &str, values: &serde_json::Value) -> Result<(), String> {
    if !slug_ok(slug) { return Err("invalid spark name".into()); }
    if !values.is_object() { return Err("spark state must be a JSON object".into()); }
    let text = serde_json::to_string(values).map_err(|e| format!("serialize state: {e}"))?;
    if text.len() > MAX_STATE_BYTES {
        return Err(format!("spark state too large ({} bytes > {MAX_STATE_BYTES} limit) — store big data in a file, not Spark state", text.len()));
    }
    let rel = format!("Sparks/{slug}/state.json");
    let abs = broker.resolve(agent_id, &rel, broker::Mode::Write)
        .map_err(|e| format!("refused by jail: {e:?}"))?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir spark dir: {e}"))?;
    }
    std::fs::write(&abs, text.as_bytes()).map_err(|e| format!("write state: {e}"))?;
    Ok(())
}

/// Set ONE key in a Spark's KV blob (read-modify-write, jailed). This is what the
/// shim's localStorage.setItem + window.spark.set marshal to.
pub fn set_key(broker: &Arc<Broker>, agent_id: &str, slug: &str, key: &str, value: serde_json::Value) -> Result<(), String> {
    let mut blob = read(broker, agent_id, slug)?;
    let obj = blob.as_object_mut().ok_or("state corrupt")?;
    if key.is_empty() || key.len() > 512 { return Err("invalid key".into()); }
    obj.insert(key.to_string(), value);
    write(broker, agent_id, slug, &blob)
}

/// Remove ONE key (localStorage.removeItem).
pub fn remove_key(broker: &Arc<Broker>, agent_id: &str, slug: &str, key: &str) -> Result<(), String> {
    let mut blob = read(broker, agent_id, slug)?;
    if let Some(obj) = blob.as_object_mut() { obj.remove(key); }
    write(broker, agent_id, slug, &blob)
}
