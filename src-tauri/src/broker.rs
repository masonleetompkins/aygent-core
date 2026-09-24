// AYGENT — PATH BROKER (the trust boundary, Atlas C1/C2)
//
// The ONLY code with filesystem authority. The Node daemon runs under a
// Seatbelt profile that denies file access, so it MUST call these functions
// over the RPC bridge. The broker returns opaque HANDLES, never re-openable
// path strings (kills TOCTOU). Resolution is atomic and macOS-aware.
//
// Contract: docs/CONTRACTS.md §2. Do not change the RPC shape without migration.
//
// M0.2 — the 8 resolution rules (see resolve_within):
//   1. reject absolute paths outside root + `..` traversal (component walk)
//   2. openat() each component with O_NOFOLLOW on the final component
//   3. per-component symlink check (no escape mid-path, TOCTOU-safe)
//   4. component-BOUNDARY root compare (NOT string startsWith)
//   5. canonicalize against the data volume (firmlinks: /tmp -> /private/tmp)
//   6. case via inode identity, not naive lowercase
//   7. on write: refuse st_nlink > 1 (hardlink escape)
//   8. reject /tmp, /private/tmp, /var
//
// NOTE: real openat/O_NOFOLLOW fd handling is added when the RPC bridge lands
// (next M0.2 step). This file implements the PATH-RESOLUTION LOGIC — the part
// that decides admit/refuse — and is fully unit-testable in Rust alone.

use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::io::RawFd;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Opaque handle handed back to the daemon. NOT a path. The daemon cannot
/// derive a filesystem path from this — it can only pass it back to read/write.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Read,
    Write,
    ReadWrite,
}

impl Mode {
    fn is_write(self) -> bool {
        matches!(self, Mode::Write | Mode::ReadWrite)
    }
}

/// A READ-ONLY mount: another folder this agent may READ but never write.
///
/// This is how two agents share context while keeping separate homes (the
/// alternative — pointing both agents at one folder — makes them stomp each
/// other's memory/ and daily notes, and contaminates any A/B comparison
/// between models). A mount is resolved by the SAME fail-closed kernel as the
/// primary root; the only difference is that write modes are refused outright.
#[derive(Clone)]
pub struct Mount {
    /// Canonical, data-volume-resolved path (same contract as `root`).
    pub root: PathBuf,
    /// Display label for UI/diagnostics (e.g. the source agent's name).
    pub label: String,
}

/// One scoped root per agent (from the macOS security-scoped bookmark), plus
/// zero or more READ-ONLY mounts of other folders.
#[derive(Clone)]
pub struct AgentScope {
    pub root: PathBuf,        // canonical, data-volume-resolved
    pub bookmark_stale: bool, // Atlas C2: handle explicitly, fail closed if stale
    /// Read-only shared context. Tried ONLY after the primary root misses, and
    /// ONLY for read modes. Never writable — see resolve().
    pub mounts: Vec<Mount>,
}

pub struct Broker {
    scopes: Mutex<HashMap<String, AgentScope>>,
    #[cfg(unix)]
    #[allow(dead_code)] // used once the RPC fd bridge lands
    open_fds: Mutex<HashMap<String, RawFd>>,
}

/// The macOS system/forbidden roots we never admit (Atlas C2 rule 8).
/// Firmlinks mean /tmp == /private/tmp; we reject both canonical forms.
/// The virtual prefix for the read-only shared-context namespace. A request
/// path starting with this addresses a registered mount by label (see resolve).
/// Chosen so it can never collide with a real relative path an agent owns.
const SHARED_PREFIX: &str = "@shared/";
/// Also accept a bare "@shared" (no slash) for the discovery listing.
pub const SHARED_ROOT: &str = "@shared";

const FORBIDDEN_PREFIXES: &[&str] = &[
    "/tmp",
    "/private/tmp",
    "/private/var",
    "/var",
    "/etc",
    "/private/etc",
];

impl Broker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            scopes: Mutex::new(HashMap::new()),
            #[cfg(unix)]
            open_fds: Mutex::new(HashMap::new()),
        })
    }

    /// Register an agent's scoped root (called after the folder picker +
    /// security-scoped bookmark resolves). `root` must already be canonicalized
    /// by the caller against the data volume.
    pub fn set_scope(&self, agent_id: &str, root: PathBuf, bookmark_stale: bool) {
        let mut scopes = self.scopes.lock().unwrap();
        // Preserve any already-registered mounts: re-registering a scope (boot,
        // folder change, agent save) must not silently drop shared context.
        let mounts = scopes.get(agent_id).map(|s| s.mounts.clone()).unwrap_or_default();
        scopes.insert(
            agent_id.to_string(),
            AgentScope { root, bookmark_stale, mounts },
        );
    }

    /// Replace an agent's READ-ONLY mounts. Callers pass canonical paths (same
    /// contract as `set_scope`). A mount equal to the agent's own root is
    /// dropped — it would be a no-op that only confuses diagnostics.
    pub fn set_mounts(&self, agent_id: &str, mounts: Vec<Mount>) {
        let mut scopes = self.scopes.lock().unwrap();
        if let Some(scope) = scopes.get_mut(agent_id) {
            let own = scope.root.clone();
            scope.mounts = mounts.into_iter().filter(|m| m.root != own).collect();
        }
    }

    /// An agent's read-only mounts (empty if none / no scope).
    pub fn mounts_for(&self, agent_id: &str) -> Vec<Mount> {
        self.scopes.lock().unwrap().get(agent_id).map(|s| s.mounts.clone()).unwrap_or_default()
    }

    /// The mount LABELS an agent can address under the @shared/ namespace, in
    /// registration order. This is the discovery surface: list_files("@shared")
    /// returns one entry per label. De-duplicated + sanitized so two mounts with
    /// the same label don't produce an unaddressable collision (later dupes are
    /// suffixed -2, -3 ...). Empty if the agent has no mounts.
    pub fn shared_labels_for(&self, agent_id: &str) -> Vec<String> {
        let mounts = self.mounts_for(agent_id);
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut out = Vec::with_capacity(mounts.len());
        for m in &mounts {
            let base = sanitize_label(&m.label);
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            out.push(if *n == 1 { base } else { format!("{base}-{n}") });
        }
        out
    }

    /// Resolve a `@shared/<label>[/rest]` path to a mount root + the remainder,
    /// applying the SAME sanitize+dedupe scheme as shared_labels_for so a label
    /// the agent SEES in a listing is exactly the one it can address. Returns
    /// (mount_root, rest_path). Read-only: the caller refuses writes. `None` for
    /// an unknown label (caller maps to NotFound — never leaks which mounts
    /// exist beyond the labels it already published).
    fn resolve_shared_label(&self, agent_id: &str, label: &str) -> Option<PathBuf> {
        let mounts = self.mounts_for(agent_id);
        let labels = self.shared_labels_for(agent_id);
        labels.iter().position(|l| l == label).and_then(|i| mounts.get(i).map(|m| m.root.clone()))
    }

    /// Atomically resolve + admit/refuse. Returns the canonical in-scope path
    /// (the fd/Handle wrapping lands with the RPC bridge). SECURITY-CRITICAL —
    /// the escape suite targets this.
    pub fn resolve(&self, agent_id: &str, requested: &str, mode: Mode) -> Result<PathBuf, BrokerError> {
        let scope = self.scope_for(agent_id)?;
        if scope.bookmark_stale {
            return Err(BrokerError::StaleBookmark); // never silently widen scope
        }

        // @shared NAMESPACE (explicit, discoverable read-only mounts). A path
        // beginning `@shared/<label>[/rest]` addresses a REGISTERED mount by the
        // exact label the agent saw via list_files("@shared"). This is the fix
        // for "an agent can't SEE a mounted folder": own-root-wins meant a
        // colliding name could never reach the mount, and there was no way to
        // browse it. @shared can't collide with a real relative path the agent
        // owns (no agent file is addressed starting @shared/), so own-root-wins
        // for every NON-@shared path is untouched. Writes are REFUSED here —
        // shared context stays strictly read-only.
        if let Some(rest) = requested.strip_prefix(SHARED_PREFIX) {
            if mode.is_write() {
                return Err(BrokerError::Forbidden); // mounts are read-only
            }
            // Split "<label>/<rest...>" (or just "<label>").
            let rest = rest.trim_start_matches('/');
            let (label, sub) = match rest.split_once('/') {
                Some((l, s)) => (l, s),
                None => (rest, ""),
            };
            if label.is_empty() {
                // Bare "@shared" resolves to nothing openable; the LIST op
                // special-cases it to enumerate labels. A read here is a miss.
                return Err(BrokerError::NotFound);
            }
            let mount_root = self.resolve_shared_label(agent_id, label)
                .ok_or(BrokerError::NotFound)?;
            // Same fail-closed kernel inside the mount (traversal/symlink/etc).
            return Self::resolve_within(&mount_root, sub, mode);
        }

        // The agent's OWN root always wins: a mount can never shadow or
        // intercept a path the agent legitimately owns.
        let primary = Self::resolve_within(&scope.root, requested, mode);

        // WRITE ATTEMPTS NEVER FALL THROUGH TO A MOUNT. Shared context is
        // strictly read-only, so a write is always answered by the primary
        // root (admitted there, or refused there) — never redirected into
        // someone else's folder.
        if scope.mounts.is_empty() || mode.is_write() {
            return primary;
        }

        // READ FALLBACK. Subtlety worth stating, because it made the first
        // implementation of this a no-op: resolve_within ADMITS paths that
        // don't exist yet (it has to — that's how a file gets created). So a
        // read of "notes.md" always "succeeds" against the primary root even
        // when the agent has no such file, and the mounts would never be
        // consulted. The correct rule is about EXISTENCE, not admission:
        //   - primary path exists  -> use it (own root wins, always)
        //   - primary path missing -> the first mount that actually HAS the
        //                             file answers the read
        //   - nobody has it        -> return the primary result, so the error
        //                             (or the in-root miss) reads honestly and
        //                             no mount path leaks into the message
        if let Ok(p) = &primary {
            if p.exists() {
                return Ok(p.clone());
            }
        }
        for m in &scope.mounts {
            // Same fail-closed kernel per mount: traversal, symlink escape and
            // forbidden prefixes all still apply inside a mount.
            if let Ok(p) = Self::resolve_within(&m.root, requested, mode) {
                if p.exists() {
                    return Ok(p);
                }
            }
        }
        primary
    }

    /// Pure resolution logic — no I/O side effects beyond reading link/stat
    /// metadata. Extracted so the escape suite can hammer it directly.
    pub fn resolve_within(root: &Path, requested: &str, mode: Mode) -> Result<PathBuf, BrokerError> {
        let req = Path::new(requested);

        // Rule 1a: reject `..` traversal anywhere in the requested path.
        if req.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(BrokerError::Traversal);
        }

        // Build the candidate absolute path.
        // Absolute requests must be inside root; relative are joined to root.
        let candidate: PathBuf = if req.is_absolute() {
            req.to_path_buf()
        } else {
            root.join(req)
        };

        // Rule 8: reject forbidden system roots — BUT ONLY when the target is
        // NOT inside the agent's own root. On macOS a legit root often lives
        // under /var/folders (temp) or similar; a path inside root is fine even
        // if root sits under a "forbidden" prefix. We only forbid when the
        // request tries to REACH a system location OUTSIDE root.
        //
        // canonicalize root once for the containment tests below.
        let real_root = std::fs::canonicalize(root).map_err(|_| BrokerError::NoScope)?;
        if !is_within(&real_root, &candidate) {
            // target is outside root by lexical path -> apply forbidden check +
            // it will also fail the ancestor containment test below.
            let cand_str = candidate.to_string_lossy();
            for p in FORBIDDEN_PREFIXES {
                if cand_str == *p || cand_str.starts_with(&format!("{p}/")) {
                    return Err(BrokerError::Forbidden);
                }
            }
        }

        // Rule 2/3: walk each component; if an intermediate component is a
        // symlink that escapes root, refuse. The FINAL component is opened
        // O_NOFOLLOW at the fd layer; here we detect symlink escapes via
        // canonicalization of existing ancestors.
        //
        // We canonicalize the deepest EXISTING ancestor (a new file being
        // written won't exist yet) and require it to stay within root.
        let existing_ancestor = deepest_existing(&candidate);
        let real_ancestor = match std::fs::canonicalize(&existing_ancestor) {
            Ok(p) => p,
            Err(_) => return Err(BrokerError::NotFound),
        };

        // Rule 4/5/6: component-boundary compare against the canonical root
        // (canonicalize resolves firmlinks + case via the filesystem itself).
        // real_root computed above.
        if !is_within(&real_root, &real_ancestor) {
            return Err(BrokerError::SymlinkEscape);
        }

        // Rule 3 (final component): if the final target exists and is a symlink,
        // refuse (the fd path will use O_NOFOLLOW; belt-and-suspenders here).
        if let Ok(meta) = std::fs::symlink_metadata(&candidate) {
            if meta.file_type().is_symlink() {
                return Err(BrokerError::SymlinkEscape);
            }
            // Rule 7: on write, refuse hardlinked files (nlink > 1) — a hardlink
            // can point an in-scope name at an out-of-scope inode.
            if mode.is_write() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if meta.nlink() > 1 {
                        return Err(BrokerError::HardlinkRefused);
                    }
                }
                // Windows: same Rule 7 via BY_HANDLE nNumberOfLinks.
                // windows-sys is a Windows-only dep (see Cargo.toml); unix builds
                // never compile this.
                #[cfg(windows)]
                {
                    use std::os::windows::io::AsRawHandle;
                    use windows_sys::Win32::Storage::FileSystem::{
                        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
                    };
                    let f = std::fs::File::open(&candidate)
                        .map_err(|_| BrokerError::NotFound)?;
                    let mut info: BY_HANDLE_FILE_INFORMATION =
                        unsafe { std::mem::zeroed() };
                    let ok = unsafe {
                        GetFileInformationByHandle(f.as_raw_handle(), &mut info)
                    };
                    if ok == 0 {
                        return Err(BrokerError::Io("link count query failed".into()));
                    }
                    if info.nNumberOfLinks > 1 {
                        return Err(BrokerError::HardlinkRefused);
                    }
                }
            }
        }

        // Rebuild the admitted path rooted at the REAL root (canonical), joining
        // the portion of the request beyond the existing ancestor.
        // BUGFIX (errno 20 ENOTDIR on read-back): when the file already EXISTS,
        // existing_ancestor == candidate, so tail is empty and `join("")` would
        // append a trailing separator -> kernel reads `file/` -> ENOTDIR. In
        // that case return the canonical ancestor directly.
        let tail = candidate.strip_prefix(&existing_ancestor).unwrap_or(Path::new(""));
        if tail.as_os_str().is_empty() {
            Ok(real_ancestor)
        } else {
            Ok(real_ancestor.join(tail))
        }
    }

    fn scope_for(&self, agent_id: &str) -> Result<AgentScope, BrokerError> {
        self.scopes
            .lock()
            .unwrap()
            .get(agent_id)
            .cloned()
            .ok_or(BrokerError::NoScope)
    }

    /// The canonical scoped root for an agent, if set. Save Points run git
    /// against this root (never against a path the daemon supplies). Refuses if
    /// the bookmark is stale — same fail-closed rule as resolve().
    pub fn root_for(&self, agent_id: &str) -> Result<PathBuf, BrokerError> {
        let scope = self.scope_for(agent_id)?;
        if scope.bookmark_stale {
            return Err(BrokerError::StaleBookmark);
        }
        Ok(scope.root)
    }

    /// M0.2(c): resolve + open ATOMICALLY. This closes the TOCTOU gap: the
    /// admitted path is opened with O_NOFOLLOW on the final component in the
    /// SAME step as the check, so an attacker can't swap a component for a
    /// symlink between resolve() and open(). Returns a File (owns the fd).
    /// The daemon never receives this path — broker_ws reads/writes via the fd
    /// and returns CONTENT.
    #[cfg(unix)]
    pub fn resolve_and_open(
        &self,
        agent_id: &str,
        requested: &str,
        mode: Mode,
    ) -> Result<std::fs::File, BrokerError> {
        let admitted = self.resolve(agent_id, requested, mode)?;
        open_nofollow(&admitted, mode)
    }

    /// Windows counterpart to resolve_and_open. No O_NOFOLLOW exists here, so
    /// this is resolve() (which already refuses final-component symlinks per
    /// Rule 3) + a plain open — a documented TOCTOU residual vs unix, accepted
    /// because the jail owner (Mason/local user) is also the process owner.
    /// Same signature/return so all call sites compile unchanged.
    #[cfg(windows)]
    pub fn resolve_and_open(
        &self,
        agent_id: &str,
        requested: &str,
        mode: Mode,
    ) -> Result<std::fs::File, BrokerError> {
        use std::fs::OpenOptions;
        let admitted = self.resolve(agent_id, requested, mode)?;
        let file = match mode {
            Mode::Read => OpenOptions::new().read(true).open(&admitted),
            Mode::Write => OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&admitted),
            Mode::ReadWrite => OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&admitted),
        };
        file.map_err(|e| BrokerError::Io(format!("open failed: {e}")))
    }
} // end impl Broker

/// Open a path with O_NOFOLLOW on the final component (Atlas C2 rule 2/3).
/// If the final component is a symlink, the OS itself refuses with ELOOP — the
/// atomic guarantee that a check-then-open race cannot bypass.
#[cfg(unix)]
fn open_nofollow(path: &Path, mode: Mode) -> Result<std::fs::File, BrokerError> {
    use std::os::unix::ffi::OsStrExt;
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| BrokerError::NotFound)?;

    let mut flags = libc::O_NOFOLLOW | libc::O_CLOEXEC;
    match mode {
        Mode::Read => flags |= libc::O_RDONLY,
        Mode::Write => flags |= libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
        Mode::ReadWrite => flags |= libc::O_RDWR | libc::O_CREAT,
    }

    // 0o600 for newly-created files (owner-only).
    let fd = unsafe { libc::open(cpath.as_ptr(), flags, 0o600 as libc::c_uint) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        // ELOOP = final component was a symlink (O_NOFOLLOW refused it).
        if err.raw_os_error() == Some(libc::ELOOP) {
            return Err(BrokerError::SymlinkEscape);
        }
        // Surface the real errno instead of masking everything as NotFound
        // (that masked the read-back bug). e.g. ENOENT vs EACCES vs EISDIR.
        return Err(BrokerError::Io(format!(
            "open failed (errno {:?}): {}",
            err.raw_os_error(),
            err
        )));
    }
    use std::os::unix::io::FromRawFd;
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

/// Component-boundary containment check (Atlas C2 rule 4: NOT string prefix).
/// `/Users/m/vault` does NOT contain `/Users/m/vaultEVIL`.
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    let root_c: Vec<Component> = root.components().collect();
    let cand_c: Vec<Component> = candidate.components().collect();
    if cand_c.len() < root_c.len() {
        return false;
    }
    root_c.iter().zip(cand_c.iter()).all(|(a, b)| a == b)
}

/// Sanitize a mount label into a single safe path segment for the @shared
/// namespace: keep alphanumerics/dash/underscore/dot, replace the rest with
/// '-', collapse repeats, trim, and never allow "."/".."/empty. Deterministic
/// so the label an agent lists is the label it can address.
fn sanitize_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;
    for ch in raw.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.';
        if ok {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." { "shared".to_string() } else { trimmed }
}

/// Walk up from `p` to the deepest ancestor that actually exists on disk.
/// Used so we can canonicalize (resolve symlinks/firmlinks/case) even when the
/// requested file itself is new (being created).
fn deepest_existing(p: &Path) -> PathBuf {
    let mut cur = p;
    loop {
        if cur.exists() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return PathBuf::from("/"),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum BrokerError {
    NoScope,
    StaleBookmark,
    Traversal,
    Forbidden,
    OutsideRoot,
    SymlinkEscape,
    HardlinkRefused,
    NotFound,
    Io(String),
    NotYetImplemented,
}

// ---------------------------------------------------------------------------
// ESCAPE SUITE (Rust-level) — runnable with `cargo test`. These are the pure
// path-resolution tests (Part F #4,5,6,7,10). The fs-level TOCTOU/hardlink
// tests that need the real daemon-under-Seatbelt land with the RPC bridge.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_root() -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("aygent_test_{}", rand_suffix()));
        fs::create_dir_all(&d).unwrap();
        // canonicalize so comparisons match resolve_within's canonical root
        fs::canonicalize(&d).unwrap()
    }
    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    #[test]
    fn t4_parent_dir_traversal_refused() {
        let root = tmp_root();
        let r = Broker::resolve_within(&root, "../../etc/passwd", Mode::Read);
        assert_eq!(r, Err(BrokerError::Traversal));
    }

    #[test]
    fn t5_absolute_outside_refused() {
        let root = tmp_root();
        // /etc is forbidden AND outside root
        let r = Broker::resolve_within(&root, "/etc/passwd", Mode::Read);
        assert!(matches!(r, Err(BrokerError::Forbidden) | Err(BrokerError::SymlinkEscape) | Err(BrokerError::NotFound)));
    }

    #[test]
    fn t6_sibling_prefix_not_admitted() {
        // component-boundary compare: /root vs /rootEVIL
        let root = PathBuf::from("/Users/m/vault");
        let evil = PathBuf::from("/Users/m/vaultEVIL/secret");
        assert!(!is_within(&root, &evil));
        let child = PathBuf::from("/Users/m/vault/notes/a.md");
        assert!(is_within(&root, &child));
    }

    #[test]
    fn t7_in_scope_file_admitted() {
        let root = tmp_root();
        fs::write(root.join("note.md"), b"hi").unwrap();
        let r = Broker::resolve_within(&root, "note.md", Mode::Read).unwrap();
        assert!(is_within(&root, &r));
    }

    #[test]
    fn t8_new_file_in_scope_admitted() {
        let root = tmp_root();
        // file doesn't exist yet (write path) — must still admit within root
        let r = Broker::resolve_within(&root, "subdir/new.md", Mode::Write).unwrap();
        assert!(is_within(&root, &r));
    }

    #[test]
    fn t10_tmp_forbidden() {
        let root = tmp_root();
        let r = Broker::resolve_within(&root, "/tmp/evil", Mode::Read);
        assert_eq!(r, Err(BrokerError::Forbidden));
    }

    #[test]
    fn t_symlink_escape_refused() {
        let root = tmp_root();
        // create a symlink inside root pointing OUT to /etc
        let link = root.join("escape");
        std::os::unix::fs::symlink("/etc", &link).unwrap();
        let r = Broker::resolve_within(&root, "escape/passwd", Mode::Read);
        assert_eq!(r, Err(BrokerError::SymlinkEscape));
    }

    #[test]
    fn t_final_symlink_refused() {
        let root = tmp_root();
        let target = root.join("real.md");
        fs::write(&target, b"x").unwrap();
        let link = root.join("link.md");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let r = Broker::resolve_within(&root, "link.md", Mode::Read);
        assert_eq!(r, Err(BrokerError::SymlinkEscape));
    }

    // -----------------------------------------------------------------------
    // GATE TESTS — adversarial, against the REAL atomic open (resolve_and_open).
    // These prove the atomic O_NOFOLLOW layer survives an active attacker, not
    // just correct resolution. (Part F tests 8 + 9.)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // SHARED CONTEXT (read-only mounts). These are the security tests for the
    // "two agents, one project" feature: a mount must grant READS and NEVER a
    // write, and must never shadow the agent's own root.
    // -----------------------------------------------------------------------

    fn broker_with_mount(root: &Path, mount: &Path) -> Arc<Broker> {
        let b = Broker::new();
        b.set_scope("a1", root.to_path_buf(), false);
        b.set_mounts("a1", vec![Mount { root: mount.to_path_buf(), label: "shared".into() }]);
        b
    }

    #[test]
    fn mount_read_is_admitted() {
        // The whole point: agent A reads a file that only exists in agent B's
        // folder, without B's folder being A's root.
        let root = tmp_root();
        let shared = tmp_root();
        fs::write(shared.join("notes.md"), b"shared knowledge").unwrap();

        let b = broker_with_mount(&root, &shared);
        let r = b.resolve("a1", "notes.md", Mode::Read);
        assert!(r.is_ok(), "expected mount read to be admitted, got {r:?}");
        assert_eq!(r.unwrap(), shared.join("notes.md"));
    }

    #[test]
    fn mount_write_never_lands_in_the_mount() {
        // THE critical guarantee. A mounted folder is READ-ONLY. Writing a name
        // that exists ONLY in the mount must resolve into the agent's OWN root
        // (creating its own copy) — never into the shared folder.
        let root = tmp_root();
        let shared = tmp_root();
        fs::write(shared.join("notes.md"), b"shared knowledge").unwrap();

        let b = broker_with_mount(&root, &shared);
        let got = b.resolve("a1", "notes.md", Mode::Write).unwrap();
        assert!(got.starts_with(&root), "write escaped into a mount: {got:?}");
        assert!(!got.starts_with(&shared), "write escaped into a mount: {got:?}");

        // The shared file is untouched by a write through the mount.
        assert_eq!(fs::read_to_string(shared.join("notes.md")).unwrap(), "shared knowledge");
    }

    #[test]
    fn mount_read_of_missing_file_does_not_leak_mount_path() {
        // A file nobody has: the answer must come from the agent's own root, so
        // the error/path never reveals a mounted folder's location.
        let root = tmp_root();
        let shared = tmp_root();
        let b = broker_with_mount(&root, &shared);
        let got = b.resolve("a1", "nope.md", Mode::Read).unwrap();
        assert!(got.starts_with(&root), "leaked a mount path: {got:?}");
    }

    #[test]
    fn mount_never_shadows_own_root() {
        // Same relative name in both places: the agent's OWN file must win, so
        // a mount can never intercept a path the agent legitimately owns.
        let root = tmp_root();
        let shared = tmp_root();
        fs::write(root.join("notes.md"), b"mine").unwrap();
        fs::write(shared.join("notes.md"), b"theirs").unwrap();

        let b = broker_with_mount(&root, &shared);
        let got = b.resolve("a1", "notes.md", Mode::Read).unwrap();
        assert_eq!(got, root.join("notes.md"));
        assert_eq!(fs::read_to_string(got).unwrap(), "mine");
    }

    #[test]
    fn mount_still_refuses_traversal() {
        // A mount is resolved by the SAME kernel: `..` escapes stay refused.
        let root = tmp_root();
        let shared = tmp_root();
        let b = broker_with_mount(&root, &shared);
        let r = b.resolve("a1", "../outside.md", Mode::Read);
        assert!(matches!(r, Err(BrokerError::Traversal)), "got {r:?}");
    }

    #[test]
    fn set_scope_preserves_mounts() {
        // Re-registering a scope (boot, folder change, agent save) must not
        // silently drop shared context.
        let root = tmp_root();
        let shared = tmp_root();
        fs::write(shared.join("notes.md"), b"x").unwrap();
        let b = broker_with_mount(&root, &shared);

        b.set_scope("a1", root.to_path_buf(), false); // re-register
        assert_eq!(b.mounts_for("a1").len(), 1, "mounts dropped on re-register");
        assert!(b.resolve("a1", "notes.md", Mode::Read).is_ok());
    }

    // -- @shared NAMESPACE (discoverable, unambiguous read-only mounts) --------

    #[test]
    fn shared_labels_are_listed() {
        // The discovery surface: an agent can enumerate its mounts by label.
        let root = tmp_root();
        let shared = tmp_root();
        let b = broker_with_mount(&root, &shared); // label "shared"
        assert_eq!(b.shared_labels_for("a1"), vec!["shared".to_string()]);
    }

    #[test]
    fn shared_path_reads_the_mount() {
        // @shared/<label>/file resolves into the mount root, unambiguously.
        let root = tmp_root();
        let shared = tmp_root();
        fs::write(shared.join("memory.md"), b"cleo brain").unwrap();
        let b = broker_with_mount(&root, &shared);
        let got = b.resolve("a1", "@shared/shared/memory.md", Mode::Read).unwrap();
        assert_eq!(got, shared.join("memory.md"));
        assert_eq!(fs::read_to_string(got).unwrap(), "cleo brain");
    }

    #[test]
    fn shared_path_reaches_a_colliding_name() {
        // THE bug: a name that exists in BOTH folders. Own-root-wins makes the
        // plain name hit the agent's own file; @shared explicitly reaches the
        // mounted one. Both must be addressable.
        let root = tmp_root();
        let shared = tmp_root();
        fs::write(root.join("memory.md"), b"mine").unwrap();
        fs::write(shared.join("memory.md"), b"theirs").unwrap();
        let b = broker_with_mount(&root, &shared);
        // plain -> own
        let own = b.resolve("a1", "memory.md", Mode::Read).unwrap();
        assert_eq!(fs::read_to_string(own).unwrap(), "mine");
        // @shared -> mount
        let their = b.resolve("a1", "@shared/shared/memory.md", Mode::Read).unwrap();
        assert_eq!(fs::read_to_string(their).unwrap(), "theirs");
    }

    #[test]
    fn shared_write_is_refused() {
        // Mounts are strictly read-only; a write through @shared is refused and
        // NEVER lands in the mount.
        let root = tmp_root();
        let shared = tmp_root();
        fs::write(shared.join("memory.md"), b"theirs").unwrap();
        let b = broker_with_mount(&root, &shared);
        let r = b.resolve("a1", "@shared/shared/memory.md", Mode::Write);
        assert_eq!(r, Err(BrokerError::Forbidden));
        assert_eq!(fs::read_to_string(shared.join("memory.md")).unwrap(), "theirs");
    }

    #[test]
    fn shared_unknown_label_is_not_found() {
        // An unregistered label maps to NotFound — no leak of which mounts exist.
        let root = tmp_root();
        let shared = tmp_root();
        let b = broker_with_mount(&root, &shared);
        let r = b.resolve("a1", "@shared/nope/x.md", Mode::Read);
        assert_eq!(r, Err(BrokerError::NotFound));
    }

    #[test]
    fn shared_still_refuses_traversal() {
        // The mount kernel still applies inside @shared: `..` can't escape.
        let root = tmp_root();
        let shared = tmp_root();
        let b = broker_with_mount(&root, &shared);
        let r = b.resolve("a1", "@shared/shared/../escape.md", Mode::Read);
        assert!(matches!(r, Err(BrokerError::Traversal)), "got {r:?}");
    }

    #[test]
    fn shared_labels_dedupe() {
        // Two mounts with the same label get distinct, addressable names.
        let root = tmp_root();
        let m1 = tmp_root();
        let m2 = tmp_root();
        fs::write(m2.join("x.md"), b"from second").unwrap();
        let b = Broker::new();
        b.set_scope("a1", root.clone(), false);
        b.set_mounts("a1", vec![
            Mount { root: m1.clone(), label: "Cleo".into() },
            Mount { root: m2.clone(), label: "Cleo".into() },
        ]);
        assert_eq!(b.shared_labels_for("a1"), vec!["Cleo".to_string(), "Cleo-2".to_string()]);
        let got = b.resolve("a1", "@shared/Cleo-2/x.md", Mode::Read).unwrap();
        assert_eq!(fs::read_to_string(got).unwrap(), "from second");
    }

    #[test]
    fn self_mount_is_dropped() {
        // Mounting your own root is a no-op, not a duplicate scope.
        let root = tmp_root();
        let b = Broker::new();
        b.set_scope("a1", root.to_path_buf(), false);
        b.set_mounts("a1", vec![Mount { root: root.clone(), label: "self".into() }]);
        assert!(b.mounts_for("a1").is_empty());
    }

    /// A broker scoped to a root, for exercising resolve_and_open directly.
    fn broker_with_scope(root: &Path) -> Arc<Broker> {
        let b = Broker::new();
        b.set_scope("default", root.to_path_buf(), false);
        b
    }

    #[test]
    fn gate_hardlink_write_refused() {
        // Rule 7: a hardlink inside root pointing at an OUTSIDE inode must be
        // refused on write (nlink > 1). Create an outside file, hardlink it in.
        let root = tmp_root();
        let outside_dir = tmp_root(); // separate root => "outside"
        let outside = outside_dir.join("secret");
        fs::write(&outside, b"top secret").unwrap();
        let hl = root.join("innocent.md");
        // hardlink (not symlink): same inode, nlink becomes 2.
        std::fs::hard_link(&outside, &hl).unwrap();

        let b = broker_with_scope(&root);
        let r = b.resolve_and_open("default", "innocent.md", Mode::Write);
        assert!(
            matches!(r, Err(BrokerError::HardlinkRefused)),
            "expected HardlinkRefused, got {r:?}"
        );
    }

    #[test]
    fn gate_toctou_symlink_swap() {
        // Rule 2/3 atomic: race resolve vs an attacker swapping a component for
        // a symlink to /etc. With O_NOFOLLOW at the final component + the
        // canonicalized-ancestor check, the broker must NEVER open outside root,
        // no matter the interleaving. We hammer it in a loop while a thread
        // flips a name between a real file and a symlink-to-/etc/passwd.
        use std::sync::atomic::{AtomicBool, Ordering};
        let root = tmp_root();
        let name = "racy";
        let real = root.join("racy_real");
        fs::write(&real, b"in-scope").unwrap();
        let target = root.join(name);
        fs::write(&target, b"in-scope").unwrap();

        let b = broker_with_scope(&root);
        let stop = Arc::new(AtomicBool::new(false));

        // Attacker thread: repeatedly swap `racy` between a real file and a
        // symlink pointing OUT to /etc/passwd.
        let root2 = root.clone();
        let stop2 = stop.clone();
        let attacker = std::thread::spawn(move || {
            let p = root2.join(name);
            while !stop2.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&p);
                let _ = std::os::unix::fs::symlink("/etc/passwd", &p);
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::write(&p, b"in-scope");
            }
        });

        // Victim loop: open many times; assert we NEVER read /etc/passwd content.
        for _ in 0..5000 {
            if let Ok(mut f) = b.resolve_and_open("default", name, Mode::Read) {
                use std::io::Read;
                let mut s = String::new();
                let _ = f.read_to_string(&mut s);
                assert!(
                    !s.contains("root:") && !s.contains("/bin/"),
                    "LEAK: opened /etc/passwd content through TOCTOU race"
                );
            }
        }
        stop.store(true, Ordering::Relaxed);
        attacker.join().unwrap();
    }
}
