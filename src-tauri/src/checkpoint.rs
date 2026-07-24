// AYGENT — Checkpoints (Phase 1, Contract C4).
//
// "The AI agent you actually own" only means something if you can UNDO what it
// did to your files. Checkpoints give AYGENT a rewind button: before an agent
// turn writes anything, we snapshot the folder; if the result is wrong, one
// click restores the exact prior tree.
//
// ZERO USER SETUP (the whole product promise): we use **git2 / libgit2**, which
// is compiled INTO our binary. There is NO dependency on a system `git` install
// and nothing to bundle separately — the git object model just lives inside the
// app. A user double-clicks AYGENT and checkpoints work, full stop.
//
// DESIGN (per docs/CONTRACTS.md §4 — the frozen Folder-lock protocol picked git):
//   - Each agent folder gets a SHADOW git repo whose GIT_DIR lives at
//     `<root>/.aygent/checkpoints.git`, with the work-tree set to `<root>`.
//     A separate GIT_DIR (not `<root>/.git`) means we NEVER touch or conflict
//     with a user's real git repo if the folder already is one.
//   - A checkpoint = stage-all + commit of the whole work-tree. The commit
//     message carries the user prompt that caused the turn.
//   - Rewind = reset the work-tree to that commit's tree (checkout + remove
//     files the target didn't have), then commit the restore so HEAD tracks it.
//   - We SNAPSHOT-BEFORE the rewind too, so "undo the undo" is always possible.
//
// SECURITY: the repo is opened at the broker-resolved root (never a
// daemon-supplied path). `.aygent/` is git-ignored so checkpoints never recurse
// into the shadow repo itself.

use git2::{IndexAddOption, Repository, ResetType, Signature};
use std::path::{Path, PathBuf};

/// One checkpoint entry surfaced to the UI timeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Checkpoint {
    pub id: String,        // commit sha (short)
    pub message: String,   // the prompt/label that caused this snapshot
    pub timestamp: i64,    // unix seconds (commit time)
    pub files: usize,      // files changed vs the previous checkpoint
    pub is_current: bool,  // is this the checkpoint the tree currently matches?
}

fn git_dir(root: &Path) -> PathBuf {
    root.join(".aygent").join("checkpoints.git")
}

fn sig() -> Result<Signature<'static>, String> {
    Signature::now("AYGENT Checkpoints", "checkpoints@aygent.local")
        .map_err(|e| format!("signature: {e}"))
}

/// Open the shadow repo, creating + configuring it if absent. Idempotent — safe
/// to call before every snapshot. The work-tree is set to `root` so a bare-ish
/// separate GIT_DIR still commits/checks-out the user's folder.
fn open_or_init(root: &Path) -> Result<Repository, String> {
    let gd = git_dir(root);
    if gd.exists() {
        return Repository::open(&gd).map_err(|e| format!("open repo: {e}"));
    }
    std::fs::create_dir_all(&gd).map_err(|e| format!("mkdir .aygent: {e}"))?;

    // Init a repo whose GIT_DIR is our shadow dir but whose work-tree is `root`.
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(false);
    opts.no_reinit(true);
    opts.workdir_path(root);
    let repo = Repository::init_opts(&gd, &opts).map_err(|e| format!("init repo: {e}"))?;

    // Never snapshot our own shadow repo (avoid recursion/bloat).
    let exclude = gd.join("info").join("exclude");
    if let Some(parent) = exclude.parent() { let _ = std::fs::create_dir_all(parent); }
    let _ = std::fs::write(&exclude, ".aygent/\n");

    Ok(repo)
}

/// Stage the entire work-tree into the index and write the tree object. Returns
/// the tree oid. Honors `.aygent/info/exclude` so the shadow repo is skipped.
fn stage_all(repo: &Repository) -> Result<git2::Oid, String> {
    let mut index = repo.index().map_err(|e| format!("index: {e}"))?;
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .map_err(|e| format!("add_all: {e}"))?;
    // Capture deletions too (add_all handles new/modified; update_all handles rm).
    index
        .update_all(["*"].iter(), None)
        .map_err(|e| format!("update_all: {e}"))?;
    index.write().map_err(|e| format!("index write: {e}"))?;
    index.write_tree().map_err(|e| format!("write_tree: {e}"))
}

/// Take a checkpoint of the whole work-tree. `label` becomes the commit message
/// (typically the user prompt). Returns the new checkpoint's short sha, or None
/// if nothing changed since the last checkpoint (no empty commits).
pub fn snapshot(root: &Path, label: &str) -> Result<Option<String>, String> {
    let repo = open_or_init(root)?;
    let tree_oid = stage_all(&repo)?;
    let tree = repo.find_tree(tree_oid).map_err(|e| format!("find_tree: {e}"))?;

    // Current HEAD (if any) becomes the parent.
    let parent_commit = match repo.head() {
        Ok(h) => h.peel_to_commit().ok(),
        Err(_) => None,
    };

    // Skip empty checkpoints (tree identical to parent), but always allow the
    // very first commit so there's an anchor to rewind to.
    if let Some(ref parent) = parent_commit {
        if parent.tree_id() == tree_oid {
            return Ok(None);
        }
    }

    let signature = sig()?;
    let msg = if label.trim().is_empty() { "checkpoint" } else { label.trim() };
    let parents: Vec<&git2::Commit> = parent_commit.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &signature, &signature, msg, &tree, &parents)
        .map_err(|e| format!("commit: {e}"))?;

    Ok(Some(short(&oid)))
}

/// List checkpoints newest-first. `is_current` marks the commit the work-tree
/// presently matches (HEAD — after a rewind we commit the restore so HEAD is
/// always the "current" pointer).
pub fn list(root: &Path) -> Result<Vec<Checkpoint>, String> {
    if !git_dir(root).exists() {
        return Ok(vec![]);
    }
    let repo = open_or_init(root)?;
    let head_oid = match repo.head().ok().and_then(|h| h.target()) {
        Some(o) => o,
        None => return Ok(vec![]), // no commits yet
    };

    let mut walk = repo.revwalk().map_err(|e| format!("revwalk: {e}"))?;
    walk.push_head().map_err(|e| format!("push_head: {e}"))?;
    walk.set_sorting(git2::Sort::TIME).map_err(|e| format!("sort: {e}"))?;

    let mut out = Vec::new();
    for oid in walk {
        let oid = oid.map_err(|e| format!("walk: {e}"))?;
        let commit = repo.find_commit(oid).map_err(|e| format!("find_commit: {e}"))?;
        let message = commit.summary().unwrap_or("checkpoint").to_string();
        let timestamp = commit.time().seconds();

        // files changed vs the (first) parent — 0 for the root commit.
        let files = if commit.parent_count() > 0 {
            let parent = commit.parent(0).map_err(|e| format!("parent: {e}"))?;
            let a = parent.tree().map_err(|e| format!("ptree: {e}"))?;
            let b = commit.tree().map_err(|e| format!("ctree: {e}"))?;
            repo.diff_tree_to_tree(Some(&a), Some(&b), None)
                .map(|d| d.deltas().count())
                .unwrap_or(0)
        } else {
            0
        };

        out.push(Checkpoint {
            id: short(&oid),
            message,
            timestamp,
            files,
            is_current: oid == head_oid,
        });
    }
    Ok(out)
}

/// Rewind the folder to a checkpoint. We FIRST snapshot the current state (so
/// the rewind itself is undoable — "undo the undo"), then hard-reset the
/// work-tree to the target tree, then commit the restore so HEAD tracks it.
pub fn rewind(root: &Path, target: &str) -> Result<(), String> {
    let repo = open_or_init(root)?;

    // Resolve the target to a real commit in OUR repo (accepts short shas).
    let obj = repo
        .revparse_single(target)
        .map_err(|_| format!("unknown checkpoint: {target}"))?;
    let target_commit = obj
        .peel_to_commit()
        .map_err(|_| format!("not a commit: {target}"))?;

    // 1) Safety snapshot of the present state (best-effort; ignore "no changes").
    let _ = snapshot(root, "before rewind");

    // 2) Hard reset the work-tree + index to the target commit's tree. This both
    //    restores modified files AND removes files the target didn't have — a
    //    true restore, not a merge. (reset moves HEAD too, which we then advance
    //    with the restore commit below to keep the timeline linear.)
    let target_obj = target_commit.as_object();
    repo.reset(target_obj, ResetType::Hard, None)
        .map_err(|e| format!("reset: {e}"))?;

    // 3) Commit the restore so HEAD == where we are now (makes `is_current`
    //    meaningful and keeps history append-only/inspectable).
    let tree_oid = stage_all(&repo)?;
    let tree = repo.find_tree(tree_oid).map_err(|e| format!("find_tree: {e}"))?;
    let signature = sig()?;
    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| format!("head: {e}"))?;
    // Only add a restore commit if the reset actually changed the tree vs HEAD.
    if head_commit.tree_id() != tree_oid {
        let parents = [&head_commit];
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &format!("rewind to {}", short(&target_commit.id())),
            &tree,
            &parents,
        )
        .map_err(|e| format!("restore commit: {e}"))?;
    }
    Ok(())
}

/// Short (7-char) sha, matching what users see in the timeline.
fn short(oid: &git2::Oid) -> String {
    let s = oid.to_string();
    s.chars().take(7).collect()
}
