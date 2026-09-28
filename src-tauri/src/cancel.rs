// AYGENT — turn cancellation (Send->Stop button).
//
// THE PROBLEM Mason hit: once a turn goes off the rails (loops, wanders, spins
// on a model that won't behave), there was NO way to stop it short of quitting
// the whole app. The Send button needs to become a Stop button while a turn is
// running, and clicking it needs to ACTUALLY interrupt the in-flight network
// stream -- not just hide the UI's spinner while the backend keeps burning
// tokens/tool calls in the background.
//
// DESIGN: a tiny global registry of cancel flags keyed by the same `channel`
// string every provider path already uses for its per-turn event channel (the
// UI's `myConvId`). `agent_stop(channel)` (a Tauri command) flips the flag;
// every provider streaming loop (Anthropic + OpenAI/OpenRouter) checks it once
// per received chunk and bails out cleanly the INSTANT it's set. The flag is
// registered when a turn starts and removed when it ends (via CancelGuard's
// Drop, so every return path -- early `?`, `return Ok`, `return Err`, natural
// fall-through -- cleans up automatically), so a stopped turn's flag can't
// linger and false-cancel a LATER turn that happens to reuse the same
// conversation id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct CancelRegistry {
    map: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

/// A live handle a provider loop polls each chunk. Cheap to clone/check.
pub type CancelFlag = Arc<AtomicBool>;

impl CancelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called once at the top of a turn (agent_stream). Creates (or resets) the
    /// flag for this channel and returns the handle the provider loop polls.
    pub fn begin(&self, channel: &str) -> CancelFlag {
        let flag = Arc::new(AtomicBool::new(false));
        self.map.lock().expect("cancel map poisoned").insert(channel.to_string(), flag.clone());
        flag
    }

    /// Called once the turn ends (success, error, or cancellation) so a stale
    /// entry can't outlive its turn and confuse a later one on the same channel.
    pub fn end(&self, channel: &str) {
        self.map.lock().expect("cancel map poisoned").remove(channel);
    }

    /// The Stop button's action: flip the flag for a running channel. Returns
    /// true if a live turn was found and signaled, false if there was nothing
    /// to stop (already finished, or an unknown channel -- never an error, a
    /// stop request racing a just-finished turn is a completely normal no-op).
    pub fn request_stop(&self, channel: &str) -> bool {
        match self.map.lock().expect("cancel map poisoned").get(channel) {
            Some(flag) => { flag.store(true, Ordering::SeqCst); true }
            None => false,
        }
    }
}

/// RAII guard: calls `end()` when dropped, so every return path out of
/// agent_stream cleans up its cancel-flag entry without having to remember to
/// call `end()` at each one.
pub struct CancelGuard {
    reg: CancelRegistry,
    channel: String,
}

impl CancelGuard {
    pub fn new(reg: CancelRegistry, channel: String) -> (Self, CancelFlag) {
        let flag = reg.begin(&channel);
        (Self { reg, channel }, flag)
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.reg.end(&self.channel);
    }
}
