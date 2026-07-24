// AYGENT — Checkpoints (Phase 1, Contract C4).
//
// "The AI agent you actually own" only means something if you can UNDO what it
// did to your files. Checkpoints give AYGENT a rewind button: before an agent
// turn writes anything, we snapshot the folder; if the result is wrong, one
// click restores the exact prior tree.
//
// DESIGN (per docs/CONTRACTS.md §4 — the frozen Folder-lock protocol picked git):
//   - Each agent folder gets a SHADOW git repo whose GIT_DIR lives at
//     `<root>/.aygent/checkpoints.git`, with the work-tree set to `<root>`.
//     Using a separate GIT_DIR (not `<root>/.git`) means we NEVER touch or
//     conflict with a user's real git repo if the folder already is one.
//   - A checkpoint = `git add -A && git commit` of the whole work-tree. The
//     commit message carries the user prompt that caused the turn.
//   - Rewind = `git checkout <commit> -- .` + clean untracked → the folder is
//     restored to that snapshot's exact state.
//   - We SNAPSHOT-BEFORE the rewind too, so "undo the undo" is always possible.
//
// SECURITY: every git invocation runs with cwd forced to the broker-resolved
// root (never a daemon-supplied path). The shadow repo is excluded from its own
// snapshots (`.aygent/` is git-ignored) so checkpoints never recurse.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One checkpoint entry surfaced to the UI timeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Checkpoint {
    pub id: String,        // commit sha (short)
    pub message: String,   // the prompt/label that caused this snapshot
    pub timestamp: i64,    // unix seconds (author date)
    pub files: usize,      // files changed vs the previous checkpoint
    pub is_current: bool,  // is this the checkpoint the tree currently matches?
}

fn git_dir(root: &Path) -> PathBuf {
    root.join(".aygent").join("checkpoints.git")
}

/// Build a git Command already pointed at the shadow repo + work-tree, with cwd
/// pinned to the root. All git operations go through this so no call can drift
/// off the resolved root.
fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(root)
        .arg("--git-dir")
        .arg(git_dir(root))
        .arg("--work-tree")
        .arg(root);
    c
}

fn run(mut cmd: Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("git spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Ensure the shadow repo exists + is configured. Idempotent — safe to call
/// before every snapshot. Sets a local identity so commits work even if the
/// user has no global git config, and ignores our own `.aygent/` dir.
pub fn ensure_repo(root: &Path) -> Result<(), String> {
    let gd = git_dir(root);
    if !gd.exists() {
        std::fs::create_dir_all(&gd).map_err(|e| format!("mkdir .aygent failed: {e}"))?;
        run({
            let mut c = git(root);
            c.args(["init", "--quiet"]);
            c
        })?;
        // Local identity (never touches the user's global git config).
        run({ let mut c = git(root); c.args(["config", "user.email", "checkpoints@aygent.local"]); c })?;
        run({ let mut c = git(root); c.args(["config", "user.name", "AYGENT Checkpoints"]); c })?;
        // Never snapshot our own shadow repo (avoid recursion/bloat).
        let exclude = gd.join("info").join("exclude");
        if let Some(parent) = exclude.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(&exclude, ".aygent/\n");
    }
    Ok(())
}

/// Take a checkpoint of the whole work-tree. `label` becomes the commit message
/// (typically the user prompt). Returns the new checkpoint's short sha, or None
/// if nothing changed since the last checkpoint (no empty commits).
pub fn snapshot(root: &Path, label: &str) -> Result<Option<String>, String> {
    ensure_repo(root)?;
    run({ let mut c = git(root); c.args(["add", "-A"]); c })?;

    // Anything staged? `git diff --cached --quiet` exits 1 when there ARE staged
    // changes. If clean (exit 0), skip — no empty checkpoints cluttering the timeline.
    let status = {
        let mut c = git(root);
        c.args(["diff", "--cached", "--quiet"]);
        c.status().map_err(|e| format!("git diff failed: {e}"))?
    };
    let has_changes = !status.success();
    // Special case: the very first commit (no HEAD yet) should always snapshot,
    // even of an empty tree, so there's an anchor to rewind to.
    let has_head = run({ let mut c = git(root); c.args(["rev-parse", "--verify", "HEAD"]); c }).is_ok();
    if !has_changes && has_head {
        return Ok(None);
    }

    let msg = if label.trim().is_empty() { "checkpoint" } else { label.trim() };
    run({
        let mut c = git(root);
        c.args(["commit", "--allow-empty", "--quiet", "-m", msg]);
        c
    })?;
    let sha = run({ let mut c = git(root); c.args(["rev-parse", "--short", "HEAD"]); c })?;
    Ok(Some(sha))
}

/// List checkpoints newest-first. `is_current` marks the commit the work-tree
/// presently matches (i.e. where a rewind last landed, or the latest snapshot).
pub fn list(root: &Path) -> Result<Vec<Checkpoint>, String> {
    if !git_dir(root).exists() { return Ok(vec![]); }
    if run({ let mut c = git(root); c.args(["rev-parse", "--verify", "HEAD"]); c }).is_err() {
        return Ok(vec![]);
    }

    // The commit the tree currently matches. After a rewind we tag the tree by
    // committing the restore, so HEAD is always the "current" pointer.
    let head = run({ let mut c = git(root); c.args(["rev-parse", "--short", "HEAD"]); c }).unwrap_or_default();

    // sha \x1f message \x1f unix-date, one record per line.
    let log = run({
        let mut c = git(root);
        c.args(["log", "--pretty=format:%h\x1f%s\x1f%at"]);
        c
    })?;

    let mut out = Vec::new();
    for line in log.lines() {
        let mut parts = line.splitn(3, '\x1f');
        let id = parts.next().unwrap_or("").to_string();
        let message = parts.next().unwrap_or("").to_string();
        let timestamp = parts.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        // files changed vs the parent commit (0 for the root commit).
        let files = run({
            let mut c = git(root);
            c.args(["diff", "--name-only", &format!("{id}~1"), &id]);
            c
        })
        .map(|s| s.lines().filter(|l| !l.is_empty()).count())
        .unwrap_or(0);
        let is_current = id == head;
        out.push(Checkpoint { id, message, timestamp, files, is_current });
    }
    Ok(out)
}

/// Rewind the folder to a checkpoint. We FIRST snapshot the current state (so
/// the rewind itself is undoable — "undo the undo"), then restore the target
/// tree, then commit the restore so HEAD tracks where we are.
pub fn rewind(root: &Path, target: &str) -> Result<(), String> {
    ensure_repo(root)?;
    // Guard: refuse a target that isn't a real commit in OUR repo.
    run({ let mut c = git(root); c.args(["cat-file", "-e", &format!("{target}^{{commit}}")]); c })
        .map_err(|_| format!("unknown checkpoint: {target}"))?;

    // 1) Safety snapshot of the present state (best-effort; ignore "nothing to commit").
    let _ = snapshot(root, "before rewind");

    // 2) Restore the target tree into the work-tree, then clean untracked files
    //    that the target didn't have (so a rewind is a true restore, not a merge).
    run({ let mut c = git(root); c.args(["checkout", target, "--", "."]); c })?;
    run({ let mut c = git(root); c.args(["clean", "-fd", "--quiet"]); c })?;

    // 3) Commit the restore so HEAD == where we are now (keeps the timeline linear
    //    and makes `is_current` meaningful).
    run({ let mut c = git(root); c.args(["add", "-A"]); c })?;
    let _ = run({
        let mut c = git(root);
        c.args(["commit", "--allow-empty", "--quiet", "-m", &format!("rewind to {target}")]);
        c
    });
    Ok(())
}
