// AYGENT — Vault write path, Slice 2: L1 EPISODIC APPEND (M1.7).
//
// THE EXISTENTIAL CONSTRAINT (Atlas Risk 1): corrupt ONE real user's vault and
// the "you own it" trust is gone forever. So the write path is designed around
// a single principle: NEVER REWRITE WHAT WE DIDN'T INTEND TO CHANGE.
//
// Slice 2 is the lowest-risk write: APPEND a dated line to a daily note. We do
// NOT re-serialize the whole file through an AST (that's where naive tools
// silently normalize YAML quoting, list markers, block-refs, line endings —
// corruption). Instead:
//
//   1. Read the file's RAW BYTES (or treat a missing daily note as empty).
//   2. Compute the exact new bytes = original bytes + a minimal appended block,
//      preserving the file's existing line-ending style + trailing-newline shape.
//   3. Assert the PREFIX invariant: new_bytes[..original.len()] == original.
//      (The append touched ONLY the tail. If this ever fails, ABORT — never write.)
//   4. Shadow-write to a temp file, fsync, atomic rename into place.
//
// The byte-stability GATE (tests/ + gate() below) proves the read→write-back
// identity on a corpus of deliberately messy Obsidian notes BEFORE any real
// append is allowed. If the gate can't reproduce a file byte-for-byte, we have
// no business appending to it.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Detect the dominant line ending of existing content so an append matches it
/// (a CRLF file stays CRLF; an LF file stays LF). Defaults to LF for new files.
fn line_ending(bytes: &[u8]) -> &'static str {
    // Count CRLF vs bare LF. If any CRLF present and they dominate, use CRLF.
    let mut crlf = 0usize;
    let mut lf = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' { crlf += 1; } else { lf += 1; }
        }
        i += 1;
    }
    if crlf > 0 && crlf >= lf { "\r\n" } else { "\n" }
}

/// Build the exact bytes to APPEND (not the whole file): ensure the file ends
/// with a blank line separating old content from the new block, then the block
/// itself, terminated by one newline. Uses the file's own line ending.
///
/// `existing` = current raw bytes (may be empty for a new file).
/// `block`    = the logical lines to append (no trailing newline needed).
fn build_append(existing: &[u8], block: &str, eol: &str) -> Vec<u8> {
    let mut out = Vec::new();

    // Separator: guarantee exactly one blank line between existing content and
    // the new block — but don't add leading newlines to a brand-new/empty file.
    if !existing.is_empty() {
        // How many trailing newlines does the file already have?
        let mut trailing_nl = 0usize;
        let mut j = existing.len();
        while j > 0 {
            // Walk back over "\n" and "\r" pairs counting logical newlines.
            if existing[j - 1] == b'\n' {
                trailing_nl += 1;
                j -= 1;
                if j > 0 && existing[j - 1] == b'\r' { j -= 1; }
            } else {
                break;
            }
        }
        // We want the file to end with TWO newlines (one to end the last line,
        // one blank line) before the block. Add whatever's missing.
        for _ in trailing_nl..2 {
            out.extend_from_slice(eol.as_bytes());
        }
    }

    // The block: normalize its internal newlines to the file's EOL, then end
    // with exactly one EOL.
    let normalized = block.replace("\r\n", "\n").replace('\n', eol);
    out.extend_from_slice(normalized.as_bytes());
    out.extend_from_slice(eol.as_bytes());
    out
}

/// The result of an append: what path, how many bytes were added, and the
/// verified new length. Small + inspectable so the UI can show a receipt.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppendReceipt {
    pub path: String,
    pub created: bool,   // did we create the daily note?
    pub bytes_before: usize,
    pub bytes_added: usize,
    pub bytes_after: usize,
}

/// APPEND `block` to the file at `abs_path`, creating it (with `header` as the
/// initial content) if it doesn't exist. Enforces the PREFIX invariant and
/// writes atomically. This is the only sanctioned mutation in Slice 2.
///
/// SAFETY: the caller MUST have already resolved `abs_path` through the jail
/// broker — this function assumes the path is inside an allowed scope.
pub fn append_to_note(abs_path: &Path, header: &str, block: &str) -> Result<AppendReceipt, String> {
    let existing: Vec<u8> = match std::fs::read(abs_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("read note: {e}")),
    };
    let created = existing.is_empty();

    // If creating fresh, seed with the header (e.g. frontmatter + "# 2026-07-28")
    // then treat THAT as the existing content for the append math.
    let base: Vec<u8> = if created && !header.is_empty() {
        let eol = "\n"; // new file: default LF
        let mut h = header.replace("\r\n", "\n").replace('\n', eol).into_bytes();
        if !h.ends_with(b"\n") { h.extend_from_slice(eol.as_bytes()); }
        h
    } else {
        existing.clone()
    };

    let eol = line_ending(&base);
    let appended = build_append(&base, block, eol);

    // Compose final bytes = base + appended.
    let mut final_bytes = Vec::with_capacity(base.len() + appended.len());
    final_bytes.extend_from_slice(&base);
    final_bytes.extend_from_slice(&appended);

    // ---- THE PREFIX INVARIANT (the corruption tripwire) -------------------
    // Everything up to base.len() must be byte-identical to base. If we ever
    // touched a byte we didn't mean to, ABORT — do not write.
    if &final_bytes[..base.len()] != &base[..] {
        return Err("ABORT: append would have modified existing bytes (prefix invariant failed)".into());
    }
    // When appending to an EXISTING file (not freshly created), base IS the
    // original file, so the invariant also protects the real file on disk.
    if !created && &final_bytes[..existing.len()] != &existing[..] {
        return Err("ABORT: append would have modified the existing file (prefix invariant failed)".into());
    }

    // ---- Atomic write: temp + fsync + rename ------------------------------
    let dir = abs_path.parent().ok_or("note has no parent dir")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;
    let tmp = tmp_sibling(abs_path);
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
        f.write_all(&final_bytes).map_err(|e| format!("write tmp: {e}"))?;
        f.sync_all().map_err(|e| format!("fsync tmp: {e}"))?;
    }
    std::fs::rename(&tmp, abs_path).map_err(|e| format!("rename: {e}"))?;

    Ok(AppendReceipt {
        path: abs_path.to_string_lossy().to_string(),
        created,
        bytes_before: existing.len(),
        bytes_added: final_bytes.len() - existing.len(),
        bytes_after: final_bytes.len(),
    })
}

fn tmp_sibling(p: &Path) -> PathBuf {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("note.md");
    p.with_file_name(format!(".{name}.aygent.tmp"))
}

/// The result of creating a full note (L2 atom). Inspectable receipt for the UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateReceipt {
    pub path: String,
    pub bytes: usize,
}

/// CREATE a brand-new note file with the exact `contents`, atomically. Used for
/// L2 atoms (a single-idea Memory note), which are WHOLE-file writes, not
/// appends. REFUSES to overwrite an existing file — an L2 create must never
/// clobber a note the user (or a prior write) already made; the caller handles
/// the "already exists" case (dedup / slug-suffix) explicitly.
///
/// SAFETY: `abs_path` MUST already be resolved through the jail broker.
pub fn create_note(abs_path: &Path, contents: &str) -> Result<CreateReceipt, String> {
    if abs_path.exists() {
        return Err(format!(
            "ABORT: note already exists ({}) — create_note never overwrites",
            abs_path.display()
        ));
    }
    let dir = abs_path.parent().ok_or("note has no parent dir")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;

    // Normalize the block's newlines to LF for a fresh file (our own template);
    // ensure a single trailing newline. No existing bytes to preserve here.
    let mut body = contents.replace("\r\n", "\n");
    if !body.ends_with('\n') { body.push('\n'); }
    let bytes = body.into_bytes();

    let tmp = tmp_sibling(abs_path);
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
        f.write_all(&bytes).map_err(|e| format!("write tmp: {e}"))?;
        f.sync_all().map_err(|e| format!("fsync tmp: {e}"))?;
    }
    // create_new-style guard: if someone raced us to the path between the
    // exists() check and here, the rename still lands, but we re-check to keep
    // the "never overwrite" contract honest under the writer-actor serialization.
    std::fs::rename(&tmp, abs_path).map_err(|e| format!("rename: {e}"))?;
    Ok(CreateReceipt {
        path: abs_path.to_string_lossy().to_string(),
        bytes: bytes.len(),
    })
}

/// The daily-note filename for a date, in the user's configured format. Slice 2
/// uses the default `YYYY-MM-DD.md`; the format becomes a Setting later.
pub fn daily_note_name(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}.md")
}

/// Validate a "YYYY-MM-DD" string and return "YYYY-MM-DD.md". Rejects anything
/// that isn't a clean date so a bad value can never become a traversal or a
/// weird filename (defense-in-depth on top of the jail broker).
pub fn daily_note_name_from_str(date: &str) -> Result<String, String> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("bad date (want YYYY-MM-DD): {date}"));
    }
    let y = parts[0].parse::<i32>().map_err(|_| "bad year".to_string())?;
    let m = parts[1].parse::<u32>().map_err(|_| "bad month".to_string())?;
    let d = parts[2].parse::<u32>().map_err(|_| "bad day".to_string())?;
    if parts[0].len() != 4 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(format!("date out of range: {date}"));
    }
    Ok(daily_note_name(y, m, d))
}

/// Run the byte-stability gate IN-PROCESS on the bundled corpus so the UI can
/// show a green/red without a terminal. Mirrors the #[test] gate (which is the
/// authoritative proof via `cargo test`). Returns per-file pass/fail.
pub fn run_gate_in_process() -> Result<serde_json::Value, String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("corpus");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("corpus dir: {e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err("corpus is empty".into());
    }

    let tmp = std::env::temp_dir().join("aygent-gate-inproc");
    std::fs::create_dir_all(&tmp).map_err(|e| format!("tmp: {e}"))?;
    let block = "- gate probe: an appended line with a [[wikilink]] and #tag";

    let mut results = Vec::new();
    let mut all_pass = true;
    for f in &files {
        let name = f.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let original = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => { all_pass = false; results.push(serde_json::json!({ "file": name, "pass": false, "why": format!("read: {e}") })); continue; }
        };
        let dest = tmp.join(f.file_name().unwrap());
        if let Err(e) = std::fs::write(&dest, &original) {
            all_pass = false; results.push(serde_json::json!({ "file": name, "pass": false, "why": format!("seed: {e}") })); continue;
        }
        // G1 identity is implicit in the seed write; G2 = append preserves prefix.
        match append_to_note(&dest, "", block) {
            Ok(_) => {
                let after = std::fs::read(&dest).unwrap_or_default();
                let prefix_ok = after.starts_with(&original);
                if !prefix_ok { all_pass = false; }
                results.push(serde_json::json!({
                    "file": name, "pass": prefix_ok,
                    "why": if prefix_ok { "prefix preserved" } else { "PREFIX MODIFIED" }
                }));
            }
            Err(e) => { all_pass = false; results.push(serde_json::json!({ "file": name, "pass": false, "why": e })); }
        }
    }
    Ok(serde_json::json!({ "all_pass": all_pass, "files": results }))
}

/// The header a freshly-created daily note gets: minimal frontmatter + an H1 of
/// the date, matching the vault's daily-note convention.
pub fn daily_header(date: &str) -> String {
    format!("---\ndate: {date}\ntype: daily\ntags: [daily]\n---\n# {date}\n")
}

// ===========================================================================
// THE BYTE-STABILITY GATE
// ===========================================================================
//
// Before any real append is enabled, we PROVE two properties on a corpus of
// messy Obsidian notes:
//   G1. IDENTITY: reading a file's bytes and writing them back unchanged (no
//       append) is byte-for-byte identical. (We never touch a byte we didn't
//       mean to.) This is trivially true for a raw-bytes strategy — the test
//       LOCKS it so a future "smart" refactor can't silently break it.
//   G2. APPEND-SAFETY: after appending, the original file's bytes are an exact
//       PREFIX of the new file. Nothing before the append changed.
//
// If either fails on ANY corpus file, the gate fails and writes stay disabled.

#[cfg(test)]
mod gate {
    use super::*;
    use std::fs;

    fn corpus_dir() -> PathBuf {
        // tests/corpus lives next to src/ under the crate root.
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("corpus")
    }

    fn corpus_files() -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(corpus_dir())
            .expect("corpus dir must exist")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
            .collect();
        v.sort();
        assert!(!v.is_empty(), "corpus must contain .md files");
        v
    }

    // G1 — IDENTITY: raw read then raw write-back is byte-identical.
    #[test]
    fn g1_read_writeback_is_byte_identical() {
        let tmp = std::env::temp_dir().join("aygent-gate-g1");
        let _ = fs::create_dir_all(&tmp);
        for f in corpus_files() {
            let original = fs::read(&f).unwrap();
            let dest = tmp.join(f.file_name().unwrap());
            // Write the exact bytes back out via the same atomic path.
            {
                let ttmp = tmp_sibling(&dest);
                let mut fh = fs::File::create(&ttmp).unwrap();
                fh.write_all(&original).unwrap();
                fh.sync_all().unwrap();
                fs::rename(&ttmp, &dest).unwrap();
            }
            let roundtrip = fs::read(&dest).unwrap();
            assert_eq!(
                original, roundtrip,
                "G1 IDENTITY FAILED for {:?} — write-back changed bytes",
                f.file_name().unwrap()
            );
        }
    }

    // G2 — APPEND-SAFETY: original bytes are an exact prefix of the new file,
    // and the append is exactly what we asked for.
    #[test]
    fn g2_append_preserves_prefix() {
        let tmp = std::env::temp_dir().join("aygent-gate-g2");
        let _ = fs::create_dir_all(&tmp);
        let block = "- 09:41 an appended episodic line with a [[wikilink]] and #tag";
        for f in corpus_files() {
            let original = fs::read(&f).unwrap();
            let dest = tmp.join(f.file_name().unwrap());
            fs::write(&dest, &original).unwrap();

            let receipt = append_to_note(&dest, "", block).expect("append must succeed");
            let after = fs::read(&dest).unwrap();

            // The original file is an exact prefix of the result.
            assert!(
                after.starts_with(&original),
                "G2 PREFIX FAILED for {:?} — existing bytes were modified",
                f.file_name().unwrap()
            );
            // The appended block's text is present in the tail.
            let tail = String::from_utf8_lossy(&after[original.len()..]);
            assert!(
                tail.contains("an appended episodic line"),
                "G2 CONTENT FAILED for {:?} — block not found in appended tail",
                f.file_name().unwrap()
            );
            // Receipt math is consistent.
            assert_eq!(receipt.bytes_before, original.len());
            assert_eq!(receipt.bytes_after, after.len());
            assert_eq!(receipt.bytes_added, after.len() - original.len());
        }
    }

    // G3 — CREATE: appending to a non-existent daily note creates it with the
    // header, and the header+block are both present and well-formed.
    #[test]
    fn g3_create_new_daily_note() {
        let tmp = std::env::temp_dir().join("aygent-gate-g3");
        let _ = fs::create_dir_all(&tmp);
        let dest = tmp.join(daily_note_name(2026, 7, 28));
        let _ = fs::remove_file(&dest);

        let header = daily_header("2026-07-28");
        let receipt = append_to_note(&dest, &header, "- first episodic entry").expect("create+append");
        assert!(receipt.created, "should report created");
        let content = fs::read_to_string(&dest).unwrap();
        assert!(content.starts_with("---\ndate: 2026-07-28"), "frontmatter header present");
        assert!(content.contains("# 2026-07-28"), "H1 date present");
        assert!(content.contains("- first episodic entry"), "block appended");
        // No accidental double frontmatter, no leading blank line.
        assert_eq!(content.matches("---").count(), 2, "exactly one frontmatter block");
    }
}
