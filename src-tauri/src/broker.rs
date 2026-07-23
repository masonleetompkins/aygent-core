// AYGENT — PATH BROKER (the trust boundary, Atlas C1/C2)
//
// This is the ONLY code with filesystem authority. The Node daemon runs under
// a Seatbelt profile that denies file access, so it MUST call these functions
// over the WS bridge. The broker returns opaque HANDLES, never re-openable
// path strings (kills TOCTOU). Resolution is atomic and macOS-aware.
//
// Contract: docs/CONTRACTS.md §2. Do not change the RPC shape without migration.

use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Opaque handle handed back to the daemon. NOT a path. The daemon cannot
/// derive a filesystem path from this — it can only pass it back to read/write.
#[derive(Clone, Debug)]
pub struct Handle {
    pub id: String, // random opaque id -> maps to a live RawFd server-side
}

pub enum Mode { Read, Write, ReadWrite }

/// One scoped root per agent (from the macOS security-scoped bookmark).
pub struct AgentScope {
    pub agent_id: String,
    pub root: PathBuf,          // canonical, data-volume-resolved
    pub bookmark_stale: bool,   // Atlas C2: must be handled, fail closed if stale
}

pub struct Broker {
    scopes: Mutex<HashMap<String, AgentScope>>,
    open_fds: Mutex<HashMap<String, RawFd>>,
    // one per-folder RW lock (Atlas S3) lives here; see lock.rs (TODO M0.2)
}

impl Broker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            scopes: Mutex::new(HashMap::new()),
            open_fds: Mutex::new(HashMap::new()),
        })
    }

    /// Atomically resolve + open. Returns an opaque Handle or refuses.
    /// SECURITY-CRITICAL — the escape suite (daemon/test) targets this.
    pub fn open(&self, agent_id: &str, requested: &str, _mode: Mode) -> Result<Handle, BrokerError> {
        let scope = self.scope_for(agent_id)?;
        if scope.bookmark_stale {
            // Atlas C2: never silently widen scope on a stale bookmark.
            return Err(BrokerError::StaleBookmark);
        }
        // TODO(M0.2): the REAL implementation must:
        //  1. reject absolute paths outside root + `..` traversal (component walk)
        //  2. openat() each component with O_NOFOLLOW on the final component
        //  3. per-component symlink check (no escape mid-path, TOCTOU-safe)
        //  4. component-BOUNDARY root compare (NOT string startsWith)
        //  5. canonicalize against the data volume (firmlinks: /tmp -> /private/tmp)
        //  6. case via inode identity, not naive lowercase
        //  7. on write: refuse st_nlink > 1 (hardlink escape)
        //  8. reject /tmp, /private/tmp, /var
        // Return a RawFd stored server-side; hand back only the opaque Handle.
        let _ = (requested, &scope.root);
        Err(BrokerError::NotYetImplemented)
    }

    fn scope_for(&self, agent_id: &str) -> Result<AgentScope, BrokerError> {
        let scopes = self.scopes.lock().unwrap();
        match scopes.get(agent_id) {
            Some(s) => Ok(AgentScope {
                agent_id: s.agent_id.clone(),
                root: s.root.clone(),
                bookmark_stale: s.bookmark_stale,
            }),
            None => Err(BrokerError::NoScope),
        }
    }

    /// Component-boundary containment check (Atlas C2: NOT string prefix).
    /// Exposed for direct unit testing by the escape suite.
    pub fn is_within(root: &Path, candidate: &Path) -> bool {
        let root_c: Vec<Component> = root.components().collect();
        let cand_c: Vec<Component> = candidate.components().collect();
        if cand_c.len() < root_c.len() { return false; }
        root_c.iter().zip(cand_c.iter()).all(|(a, b)| a == b)
    }
}

#[derive(Debug)]
pub enum BrokerError {
    NoScope,
    StaleBookmark,
    OutsideRoot,
    SymlinkEscape,
    HardlinkRefused,
    NotYetImplemented,
}
