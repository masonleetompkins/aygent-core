// AYGENT — Per-session lanes (M1.1, BUILD-SPEC core/lanes).
//
// THE GUARANTEE: for any one session (conversation), turns execute STRICTLY ONE
// AT A TIME. A second "send" on a session while its turn is still running is
// serialized behind the first — never interleaved. This kills the whole class
// of tool/session races (two turns writing the same note, mangled streaming,
// double checkpoints) at the source instead of patching each symptom.
//
// SCOPE vs the writer actor:
//   - writer.rs serializes DB *writes* globally (data integrity).
//   - lanes.rs serializes *turn execution* per session (logical integrity).
//   Different axes: you can have many sessions running in parallel (independent
//   lanes), but within one session only one turn holds the lane at a time.
//
// DESIGN: one async Mutex per sessionId, created on first use. Acquiring the
// lane = holding that mutex for the duration of the turn. Cheap, correct, and
// it composes with the folder-level lock (CONTRACTS §4) that the broker owns —
// lanes gate *which turn runs*, the folder lock gates *which writer touches a
// path*. Subagents share their parent's lane by sharing the parent session id
// (Atlas S6: share parent scope by default).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Registry of per-session locks. Clone-cheap (Arc inside); held in Tauri state.
#[derive(Clone, Default)]
pub struct Lanes {
    // sessionId -> its serialization lock. The outer StdMutex only guards the
    // MAP (fast, never held across an await); the inner AsyncMutex is the lane
    // itself and IS held across the whole turn.
    map: Arc<StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

impl Lanes {
    pub fn new() -> Self {
        Lanes { map: Arc::new(StdMutex::new(HashMap::new())) }
    }

    /// Acquire the lane for `session_id`, waiting if another turn holds it.
    /// The returned guard MUST be held for the whole turn; drop = lane released,
    /// so the next queued turn proceeds. Returns an owned guard so callers can
    /// move it into an async task without lifetime gymnastics.
    pub async fn acquire(&self, session_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.map.lock().expect("lanes map poisoned");
            map.entry(session_id.to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        lock.lock_owned().await
    }

    /// True if a turn is currently running on this session (lane held). Used by
    /// the UI to show a busy state / refuse a duplicate send instead of queuing.
    pub fn is_busy(&self, session_id: &str) -> bool {
        let map = self.map.lock().expect("lanes map poisoned");
        match map.get(session_id) {
            Some(lock) => lock.try_lock().is_err(),
            None => false,
        }
    }

    /// Drop the lane entry for a deleted session so the map doesn't grow
    /// unbounded over a long run. Safe to call anytime; a live lane just gets
    /// recreated on next acquire.
    pub fn forget(&self, session_id: &str) {
        let mut map = self.map.lock().expect("lanes map poisoned");
        map.remove(session_id);
    }
}
