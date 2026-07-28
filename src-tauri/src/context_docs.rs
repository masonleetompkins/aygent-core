// AYGENT — Per-agent context documents (M1.4, Atlas C).
//
// A user can upload documents that become extra context for ONE specific agent
// (alongside its Custom Instructions / system_prompt). Storage decisions, per
// Atlas's review:
//
//   - Docs live in APP-DATA, RUST-SIDE, OUT OF THE JAIL:
//       <app_data>/agents/<agentId>/context/<filename>
//     NOT in the agent's folder — putting them there would sweep them into the
//     folder's git save point stream (polluting the user's real vault history
//     and bloating snapshots). Wrong layer.
//
//   - Extracted text is chunked into the `mem_chunk` table (owner_kind='agent',
//     owner_id=agentId) — the table M1.1 froze for exactly this. M1.7 retrieval
//     becomes a query swap, not a re-import.
//
//   - DELIVERY for M1.4 (bridge): a small pinned prepend of the docs' text,
//     hard-capped, injected into the agent's system prompt. Full embedding
//     retrieval is the M1.7 fast-follow (the chunks are already written).
//
// Text extraction: plain-text/markdown pass through; PDFs are extracted via the
// pulldown-cmark-free path (we already bundle pdf tooling for WRITING; reading a
// PDF's text is a separate concern — for the first cut we accept .md/.txt/.json/
// .csv/code and extract PDFs best-effort if a text layer is trivially present,
// else store the file + note it needs OCR later). Keeping the first cut honest:
// we only claim to read what we can actually read.

use crate::writer::Db;
use rusqlite::params;
use std::path::{Path, PathBuf};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Per-agent context dir under app-data (created if missing). Out of the jail.
fn context_dir(app_data: &Path, agent_id: &str) -> Result<PathBuf, String> {
    let safe: String = agent_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    if safe.is_empty() { return Err("invalid agent id".into()); }
    let dir = app_data.join("agents").join(safe).join("context");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir context: {e}"))?;
    Ok(dir)
}

/// The hard cap (chars) of context-doc text we prepend to the system prompt in
/// the M1.4 bridge. Beyond this, retrieval (M1.7) is required; we truncate with
/// a note so we never blow the model's window with 3 uploaded PDFs.
pub const PREPEND_CHAR_CAP: usize = 12_000;

/// Chunk size (chars) for mem_chunk ingestion. Rough paragraph-ish windows;
/// M1.7 will re-tune with real embeddings. Kept simple + deterministic.
const CHUNK_CHARS: usize = 1_500;

/// One stored context doc (the human-facing index row shape).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextDoc {
    pub id: i64,
    pub filename: String,
    pub bytes: i64,
    pub char_count: i64,
    pub added_at: i64,
}

/// Best-effort text extraction by extension. Returns None if we can't honestly
/// read it as text (the caller stores the file but records char_count=0 + the
/// UI can show "stored, not yet readable").
fn extract_text(filename: &str, bytes: &[u8]) -> Option<String> {
    let ext = Path::new(filename).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "txt" | "md" | "markdown" | "json" | "csv" | "yaml" | "yml" | "toml"
        | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "c" | "cpp"
        | "h" | "html" | "css" | "sh" | "sql" | "log" => {
            String::from_utf8(bytes.to_vec()).ok()
        }
        _ => {
            // Unknown/binary (e.g. PDF, docx): only accept if it's actually
            // valid UTF-8 text; otherwise be honest and return None.
            std::str::from_utf8(bytes).ok().map(|s| s.to_string())
        }
    }
}

/// Add a context doc for an agent: write the file to app-data, extract text,
/// chunk it into mem_chunk, and index it in agent_context. Returns the new row.
pub fn add(db: &Db, app_data: &Path, agent_id: &str, filename: &str, bytes: &[u8]) -> Result<ContextDoc, String> {
    // Guard the filename against traversal.
    if filename.is_empty() || filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("invalid filename".into());
    }
    let dir = context_dir(app_data, agent_id)?;
    let dest = dir.join(filename);
    std::fs::write(&dest, bytes).map_err(|e| format!("write context doc: {e}"))?;

    let text = extract_text(filename, bytes);
    let char_count = text.as_ref().map(|t| t.chars().count()).unwrap_or(0);
    let stored_rel = format!("agents/{agent_id}/context/{filename}");
    let byte_len = bytes.len() as i64;
    let fname = filename.to_string();
    let aid = agent_id.to_string();

    // Index row + chunks, all in one write (single-writer actor).
    let text_for_chunks = text.clone();
    let row_id = db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        tx.execute(
            "INSERT INTO agent_context (agent_id, filename, stored_path, bytes, char_count, added_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![aid, fname, stored_rel, byte_len, char_count as i64, now()],
        ).map_err(|e| format!("insert agent_context: {e}"))?;
        let ctx_id = tx.last_insert_rowid();

        // Chunk into mem_chunk (owner_kind='agent') so M1.7 retrieval is a query
        // swap. source_path is the stored doc; ordinal is the chunk index.
        if let Some(txt) = &text_for_chunks {
            let chars: Vec<char> = txt.chars().collect();
            let mut ord = 0i64;
            let mut i = 0usize;
            while i < chars.len() {
                let end = (i + CHUNK_CHARS).min(chars.len());
                let chunk: String = chars[i..end].iter().collect();
                tx.execute(
                    "INSERT OR REPLACE INTO mem_chunk (owner_kind, owner_id, source_path, chunk_ordinal, chunk_text, updated_at)
                     VALUES ('agent', ?1, ?2, ?3, ?4, ?5)",
                    params![aid, stored_rel, ord, chunk, now()],
                ).map_err(|e| format!("insert mem_chunk: {e}"))?;
                ord += 1;
                i = end;
            }
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(ctx_id)
    })?;

    Ok(ContextDoc { id: row_id, filename: filename.to_string(), bytes: byte_len, char_count: char_count as i64, added_at: now() })
}

/// List an agent's context docs (index rows, newest first).
pub fn list(db: &Db, agent_id: &str) -> Result<Vec<ContextDoc>, String> {
    let conn = db.reader()?;
    let mut stmt = conn.prepare(
        "SELECT id, filename, bytes, char_count, added_at FROM agent_context WHERE agent_id = ?1 ORDER BY added_at DESC",
    ).map_err(|e| format!("prepare list ctx: {e}"))?;
    let rows = stmt.query_map(params![agent_id], |r| {
        Ok(ContextDoc { id: r.get(0)?, filename: r.get(1)?, bytes: r.get(2)?, char_count: r.get(3)?, added_at: r.get(4)? })
    }).map_err(|e| format!("query list ctx: {e}"))?;
    let mut out = Vec::new();
    for d in rows { out.push(d.map_err(|e| format!("row: {e}"))?); }
    Ok(out)
}

/// Remove a context doc: delete the file, its index row, and its mem_chunks.
pub fn remove(db: &Db, app_data: &Path, agent_id: &str, id: i64) -> Result<(), String> {
    // Look up the stored path + filename first (reader).
    let conn = db.reader()?;
    let row: Option<(String, String)> = conn.query_row(
        "SELECT filename, stored_path FROM agent_context WHERE id = ?1 AND agent_id = ?2",
        params![id, agent_id], |r| Ok((r.get(0)?, r.get(1)?)),
    ).ok();
    let Some((filename, stored_rel)) = row else { return Ok(()) };

    // Delete file (best-effort).
    if let Ok(dir) = context_dir(app_data, agent_id) {
        let _ = std::fs::remove_file(dir.join(&filename));
    }
    let aid = agent_id.to_string();
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        tx.execute("DELETE FROM mem_chunk WHERE owner_kind='agent' AND owner_id=?1 AND source_path=?2",
            params![aid, stored_rel]).map_err(|e| format!("del chunks: {e}"))?;
        tx.execute("DELETE FROM agent_context WHERE id=?1", params![id]).map_err(|e| format!("del ctx: {e}"))?;
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })
}

/// Assemble the M1.4 prepend block: the agent's context-doc text, concatenated
/// and hard-capped at PREPEND_CHAR_CAP. Returns "" if the agent has no readable
/// docs. This is the bridge until M1.7 embedding retrieval replaces it (the
/// chunks are already in mem_chunk, so that swap is isolated).
pub fn prepend_block(db: &Db, agent_id: &str) -> Result<String, String> {
    let conn = db.reader()?;
    // Pull chunks in order for this agent's context docs. mem_chunk holds them.
    let mut stmt = conn.prepare(
        "SELECT source_path, chunk_ordinal, chunk_text FROM mem_chunk
         WHERE owner_kind='agent' AND owner_id=?1
         ORDER BY source_path ASC, chunk_ordinal ASC",
    ).map_err(|e| format!("prepare prepend: {e}"))?;
    let rows = stmt.query_map(params![agent_id], |r| {
        let sp: String = r.get(0)?; let txt: String = r.get(2)?; Ok((sp, txt))
    }).map_err(|e| format!("query prepend: {e}"))?;

    let mut buf = String::new();
    let mut last_src = String::new();
    let mut truncated = false;
    for row in rows {
        let (src, txt) = row.map_err(|e| format!("row: {e}"))?;
        if buf.chars().count() >= PREPEND_CHAR_CAP { truncated = true; break; }
        if src != last_src {
            // Header per doc so the model knows the boundaries.
            let name = Path::new(&src).file_name().and_then(|n| n.to_str()).unwrap_or(&src);
            buf.push_str(&format!("\n\n### Context document: {name}\n"));
            last_src = src;
        }
        buf.push_str(&txt);
    }
    if buf.trim().is_empty() { return Ok(String::new()); }

    // Enforce the hard cap by chars.
    let capped: String = buf.chars().take(PREPEND_CHAR_CAP).collect();
    let note = if truncated || capped.chars().count() < buf.chars().count() {
        "\n\n(Context documents truncated to fit; full retrieval lands in a later update.)"
    } else { "" };
    Ok(format!(
        "\n\nThe user has attached the following reference documents as context for you. Use them when relevant:\n{capped}{note}"
    ))
}
