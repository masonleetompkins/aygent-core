// AYGENT — Vault-native memory, Slice 1: THE READ PATH (M1.7).
//
// Atlas's north star (projects/aygent/vault-memory-design.md): "keep every
// Obsidian property that makes memory compound; delete every property that
// needs a human gardener by making the AGENT the gardener." Slice 1 proves the
// half that carries zero risk: INGEST + RETRIEVAL, no writes to the vault.
//
// WHAT THIS MODULE DOES (and only this, for Slice 1):
//   1. Walk a vault folder for .md files (through the Rust side, which legitimately
//      has fs — the DAEMON is jailed, the shell/broker is not).
//   2. Parse each note: YAML frontmatter (typed columns), [[wikilinks]] (incl.
//      |alias, #heading, ^blockid, ![[embeds]]), and #tags. AST-lite but
//      READ-ONLY, so we never risk the byte-stability gate here — that gate
//      guards WRITES (Slice 2+), and there are none yet.
//   3. Populate the DERIVED index: note + link + vec (embedding IN-PROCESS via
//      the compiled-in llama.cpp engine — NO Ollama, install nothing). sha lets
//      us skip re-embedding unchanged files.
//   4. Retrieval = cosine similarity + GRAPH EXPANSION: top-K semantic hits, then
//      pull their linked neighbors in. The "smarter, faster" multiplier and the
//      seed of the Self-Gardening killer feature (surface the link you never made).
//
// OWNERSHIP: (owner_kind, owner_id) on every row — the C5 privacy boundary, same
// as mem_chunk. An isolated agent's graph can never bleed into a pool.
//
// SOURCE OF TRUTH = the markdown files. Everything here is a rebuildable cache:
// nuke note/link/vec, re-ingest, lose nothing. That IS the no-lock-in ethos.

use crate::writer::Db;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// PARSING (read-only; no serialization back to disk in Slice 1)
// ---------------------------------------------------------------------------

/// A parsed note: frontmatter columns + body + outbound links + tags.
#[derive(Debug, Clone)]
pub struct ParsedNote {
    pub title: String,
    pub ntype: String,
    pub agent: String,
    pub pool: String,
    pub confidence: f64,
    pub status: String,
    pub created: String,
    pub updated: String,
    pub body: String,
    /// (dst target, kind) — kind = wikilink | embed. dst is the raw link target
    /// (note name, alias/heading/blockid stripped) — resolution to a path happens
    /// at ingest against the known note set.
    pub links: Vec<(String, String)>,
    pub tags: Vec<String>,
}

/// Split a file into (frontmatter_yaml, body). Frontmatter is a leading block
/// delimited by `---` lines. If absent, frontmatter is empty and body is all.
fn split_frontmatter(raw: &str) -> (String, String) {
    // Normalize only for detection; we never write this back in Slice 1.
    let trimmed = raw.strip_prefix('\u{feff}').unwrap_or(raw); // tolerate BOM
    let mut lines = trimmed.lines();
    if lines.next() == Some("---") {
        let mut fm = String::new();
        let mut rest = String::new();
        let mut in_fm = true;
        for line in lines {
            if in_fm && line.trim_end() == "---" {
                in_fm = false;
                continue;
            }
            if in_fm {
                fm.push_str(line);
                fm.push('\n');
            } else {
                rest.push_str(line);
                rest.push('\n');
            }
        }
        if !in_fm {
            return (fm, rest);
        }
        // No closing --- : treat whole thing as body (malformed frontmatter).
    }
    (String::new(), trimmed.to_string())
}

/// Minimal YAML reader for the flat frontmatter shape our atoms use. Handles
/// `key: value`, quoted values, and `[a, b]` inline lists. NOT a general YAML
/// parser — Slice 1 only READS our own known keys; anything exotic is ignored
/// gracefully (never panics, never corrupts — it's read-only).
fn parse_frontmatter(fm: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in fm.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_string();
            let mut val = line[colon + 1..].trim().to_string();
            // Strip surrounding quotes.
            if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            {
                val = val[1..val.len() - 1].to_string();
            }
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

/// Parse an inline YAML list `[a, b, c]` into items (used for `tags`).
fn parse_inline_list(val: &str) -> Vec<String> {
    let v = val.trim();
    if v.starts_with('[') && v.ends_with(']') {
        v[1..v.len() - 1]
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if v.is_empty() {
        Vec::new()
    } else {
        vec![v.to_string()]
    }
}

/// Extract [[wikilinks]] and ![[embeds]] from body text. Strips |alias,
/// #heading, ^blockid to the base note target. Returns (target, kind).
fn extract_links(body: &str) -> Vec<(String, String)> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Detect an embed `![[` or a wikilink `[[`.
        let is_embed = i + 2 < bytes.len() && bytes[i] == b'!' && bytes[i + 1] == b'[' && bytes[i + 2] == b'[';
        let is_link = i + 1 < bytes.len() && bytes[i] == b'[' && bytes[i + 1] == b'[';
        if is_embed || is_link {
            let start = if is_embed { i + 3 } else { i + 2 };
            // Find closing ]]
            if let Some(rel_end) = body[start..].find("]]") {
                let inner = &body[start..start + rel_end];
                // Strip alias (|), heading (#), blockid (^): base target is the
                // text before the FIRST of these.
                let base = inner
                    .split(['|', '#', '^'])
                    .next()
                    .unwrap_or(inner)
                    .trim()
                    .to_string();
                if !base.is_empty() {
                    let kind = if is_embed { "embed" } else { "wikilink" };
                    out.push((base, kind.to_string()));
                }
                i = start + rel_end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Extract #tags from body (word-boundary, ignores code fences crudely — a #tag
/// inside `code` is over-collected, acceptable for Slice 1 read-only).
fn extract_tags(body: &str) -> Vec<String> {
    let mut out = HashSet::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            // A tag needs a preceding boundary (start, whitespace) and a letter next.
            let boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
            let has_alpha = i + 1 < bytes.len() && (bytes[i + 1].is_ascii_alphabetic());
            if boundary && has_alpha {
                let mut j = i + 1;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_' || bytes[j] == b'/')
                {
                    j += 1;
                }
                out.insert(body[i + 1..j].to_string());
                i = j;
                continue;
            }
        }
        i += 1;
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

/// Derive a display title: frontmatter has none, so use first `# heading` or the
/// filename stem.
fn derive_title(body: &str, path: &Path) -> String {
    for line in body.lines() {
        let l = line.trim();
        if let Some(h) = l.strip_prefix("# ") {
            return h.trim().to_string();
        }
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// Parse a raw markdown file's contents into a ParsedNote.
pub fn parse_note(raw: &str, path: &Path) -> ParsedNote {
    let (fm_raw, body) = split_frontmatter(raw);
    let fm = parse_frontmatter(&fm_raw);

    let ntype = fm.get("type").cloned().unwrap_or_else(|| "note".into());
    let agent = fm.get("agent").cloned().unwrap_or_default();
    let pool = fm
        .get("pool")
        .map(|p| if p == "null" { String::new() } else { p.clone() })
        .unwrap_or_default();
    let confidence = fm
        .get("confidence")
        .and_then(|c| c.parse::<f64>().ok())
        .unwrap_or(0.0);
    let status = fm.get("status").cloned().unwrap_or_else(|| "active".into());
    let created = fm.get("created").cloned().unwrap_or_default();
    let updated = fm.get("updated").cloned().unwrap_or_default();

    // Tags come from frontmatter `tags:` AND inline #tags in the body.
    let mut tags: HashSet<String> = fm
        .get("tags")
        .map(|t| parse_inline_list(t))
        .unwrap_or_default()
        .into_iter()
        .collect();
    for t in extract_tags(&body) {
        tags.insert(t);
    }
    let mut tags: Vec<String> = tags.into_iter().collect();
    tags.sort();

    ParsedNote {
        title: derive_title(&body, path),
        ntype,
        agent,
        pool,
        confidence,
        status,
        created,
        updated,
        body: body.clone(),
        links: extract_links(&body),
        tags,
    }
}

// ---------------------------------------------------------------------------
// EMBEDDINGS — IN-PROCESS via the compiled-in llama.cpp engine. NO OLLAMA.
//
// Mason's hard stipulation: AYGENT installs NOTHING outside the app. So we embed
// through the SAME llama-cpp-2 backend that already runs chat (local_provider),
// using a small embedding GGUF that AYGENT auto-downloads to its own models dir
// on first use. `embed_model` here is the absolute PATH to that GGUF (resolved
// by the caller), not an HTTP model name. `_endpoint` is retained in the
// signature only so the call sites don't churn; it is unused.
// ---------------------------------------------------------------------------

/// Embed one text in-process. `embed_model` is the absolute path to the local
/// embedding GGUF; `_endpoint` is ignored (kept for signature stability).
pub async fn embed(text: &str, embed_model: &str, _endpoint: &str) -> Result<Vec<f32>, String> {
    crate::local_provider::embed_local(embed_model.to_string(), text.to_string()).await
}

fn f32_to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for f in v {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b
}
fn blob_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn sha_hex(raw: &str) -> String {
    // Cheap non-crypto content hash (FNV-1a 64) — we only need change-detection,
    // not security. Avoids pulling a sha crate for Slice 1.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in raw.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

// ---------------------------------------------------------------------------
// INGEST
// ---------------------------------------------------------------------------

/// Recursively collect .md files under `root`. Skips dotfolders (.git, .aygent,
/// .obsidian) so we never index our own cache or Obsidian's config.
fn collect_md_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if p.is_dir() {
                // Skip dotfolders (.git/.aygent/.obsidian/.trash) AND build/dep
                // junk so pointing at a repo root doesn't ingest node_modules
                // READMEs, Rust target/, etc. A real vault has none of these.
                if name.starts_with('.')
                    || matches!(name, "node_modules" | "target" | "dist" | "build" | ".git")
                {
                    continue;
                }
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("md") {
                out.push(p);
            }
        }
    }
    out
}

/// The vault-relative path used as a note's identity (forward slashes, no ext-
/// stripping — the path IS the identity). e.g. "Memory/launch-stress.md".
fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Resolve a wikilink target (e.g. "launch-stress" or "2026-07-08") to a known
/// note's vault-relative path. Obsidian resolves by basename (stem) across the
/// whole vault; we mirror that. Returns the raw target if unresolved (dangling).
fn resolve_link(target: &str, stem_index: &HashMap<String, String>) -> String {
    // target may already include a subpath; try full, then basename stem.
    let stem = Path::new(target)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(target);
    stem_index
        .get(&stem.to_lowercase())
        .cloned()
        .unwrap_or_else(|| target.to_string())
}

#[derive(Debug, Serialize, Clone)]
pub struct IngestReport {
    pub scanned: usize,
    pub parsed: usize,
    pub embedded: usize,
    pub skipped_unchanged: usize,
    pub links: usize,
    pub errors: Vec<String>,
}

/// Ingest an entire vault folder into the derived index for (owner_kind,
/// owner_id). Idempotent: unchanged files (same sha) skip re-embedding. This is
/// the whole read-path build — no vault writes anywhere.
pub async fn ingest_vault(
    db: &Db,
    owner_kind: &str,
    owner_id: &str,
    root: &Path,
    embed_model: &str,
    embed_endpoint: &str,
) -> Result<IngestReport, String> {
    let mut report = IngestReport {
        scanned: 0,
        parsed: 0,
        embedded: 0,
        skipped_unchanged: 0,
        links: 0,
        errors: Vec::new(),
    };

    let files = collect_md_files(root);
    report.scanned = files.len();

    // FAIL FAST + CLEAR if the in-process embedder can't load its GGUF — one
    // probe, one actionable message, instead of the same error per file. The
    // embedder runs INSIDE AYGENT (llama.cpp), so a failure here means the GGUF
    // is missing/corrupt, not that some external service is down.
    if !files.is_empty() {
        if let Err(e) = embed("probe", embed_model, embed_endpoint).await {
            return Err(format!(
                "in-process embedder failed to load ({e}). The embedding model GGUF \
                 at '{embed_model}' may be missing or still downloading. \
                 (Scanned {} .md files but embedded none.)",
                files.len()
            ));
        }
    }

    // Pass 1: read + parse all files, build a stem index for link resolution.
    struct Pending {
        path: String,
        raw: String,
        parsed: ParsedNote,
    }
    let mut pending: Vec<Pending> = Vec::new();
    let mut stem_index: HashMap<String, String> = HashMap::new(); // stem(lower) -> rel path
    for f in &files {
        let raw = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                report.errors.push(format!("read {}: {e}", f.display()));
                continue;
            }
        };
        let rel = rel_path(root, f);
        let parsed = parse_note(&raw, f);
        if let Some(stem) = Path::new(&rel).file_stem().and_then(|s| s.to_str()) {
            stem_index.insert(stem.to_lowercase(), rel.clone());
        }
        pending.push(Pending { path: rel, raw, parsed });
    }
    report.parsed = pending.len();

    // Fetch existing shas so we skip re-embedding unchanged files.
    let existing_sha: HashMap<String, String> = {
        let conn = db.reader()?;
        let mut stmt = conn
            .prepare("SELECT path, sha FROM note WHERE owner_kind=?1 AND owner_id=?2")
            .map_err(|e| format!("prep sha: {e}"))?;
        let rows = stmt
            .query_map(params![owner_kind, owner_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| format!("query sha: {e}"))?;
        let mut m = HashMap::new();
        for row in rows.flatten() {
            m.insert(row.0, row.1);
        }
        m
    };

    // Which notes ALREADY have an embedding? A file is only truly "unchanged"
    // (safe to skip) if its sha matches AND a vec row exists. Bug (Mason 07-28):
    // an earlier failed run wrote note rows + shas but NO vectors (embedder was
    // down); the next run saw matching shas and skipped everything -> vec table
    // stayed empty -> retrieve had nothing to rank -> "No hits". Requiring an
    // existing vector makes the cache self-heal.
    let has_vec: HashSet<String> = {
        let conn = db.reader()?;
        let mut stmt = conn
            .prepare("SELECT path FROM vec WHERE owner_kind=?1 AND owner_id=?2")
            .map_err(|e| format!("prep hasvec: {e}"))?;
        let rows = stmt
            .query_map(params![owner_kind, owner_id], |r| r.get::<_, String>(0))
            .map_err(|e| format!("query hasvec: {e}"))?;
        rows.flatten().collect()
    };

    // Pass 2: embed (async, only changed files), then persist everything.
    for p in &pending {
        let sha = sha_hex(&p.raw);
        // Skip ONLY if the content is unchanged AND we already have its vector.
        let unchanged = existing_sha.get(&p.path).map(|s| s == &sha).unwrap_or(false)
            && has_vec.contains(&p.path);

        // Embedding target = title + body (frontmatter stripped). Keep it lean.
        let embed_text = format!("{}\n\n{}", p.parsed.title, p.parsed.body);

        let embedding: Option<Vec<f32>> = if unchanged {
            report.skipped_unchanged += 1;
            None
        } else {
            match embed(&embed_text, embed_model, embed_endpoint).await {
                Ok(v) => {
                    report.embedded += 1;
                    Some(v)
                }
                Err(e) => {
                    report.errors.push(format!("embed {}: {e}", p.path));
                    None
                }
            }
        };

        // Resolve links to paths now that the full stem index exists.
        let resolved_links: Vec<(String, String)> = p
            .parsed
            .links
            .iter()
            .map(|(t, k)| (resolve_link(t, &stem_index), k.clone()))
            .collect();
        report.links += resolved_links.len();

        // Persist through the single writer actor (Atlas C6).
        let (owner_kind, owner_id) = (owner_kind.to_string(), owner_id.to_string());
        let path = p.path.clone();
        let pn = p.parsed.clone();
        let tags = p.parsed.tags.clone();
        let emb_blob = embedding.as_ref().map(|v| (v.len() as i64, f32_to_blob(v)));

        db.write(move |c| {
            let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
            tx.execute(
                "INSERT INTO note (owner_kind,owner_id,path,sha,title,ntype,agent,pool,confidence,status,body,created,updated,indexed_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                 ON CONFLICT(owner_kind,owner_id,path) DO UPDATE SET
                   sha=excluded.sha, title=excluded.title, ntype=excluded.ntype, agent=excluded.agent,
                   pool=excluded.pool, confidence=excluded.confidence, status=excluded.status,
                   body=excluded.body, created=excluded.created, updated=excluded.updated,
                   indexed_at=excluded.indexed_at",
                params![owner_kind, owner_id, path, sha, pn.title, pn.ntype, pn.agent, pn.pool,
                        pn.confidence, pn.status, pn.body, pn.created, pn.updated, now()],
            ).map_err(|e| format!("upsert note: {e}"))?;

            // Rewrite this note's outbound links + tag-edges (clear then insert).
            tx.execute("DELETE FROM link WHERE owner_kind=?1 AND owner_id=?2 AND src_path=?3",
                params![owner_kind, owner_id, path]).map_err(|e| format!("clear links: {e}"))?;
            for (dst, kind) in &resolved_links {
                tx.execute(
                    "INSERT OR IGNORE INTO link (owner_kind,owner_id,src_path,dst_path,kind) VALUES (?1,?2,?3,?4,?5)",
                    params![owner_kind, owner_id, path, dst, kind],
                ).map_err(|e| format!("insert link: {e}"))?;
            }
            for t in &tags {
                tx.execute(
                    "INSERT OR IGNORE INTO link (owner_kind,owner_id,src_path,dst_path,kind) VALUES (?1,?2,?3,?4,'tag')",
                    params![owner_kind, owner_id, path, format!("#{t}")],
                ).map_err(|e| format!("insert tag edge: {e}"))?;
            }

            if let Some((dim, blob)) = &emb_blob {
                tx.execute(
                    "INSERT INTO vec (owner_kind,owner_id,path,dim,embedding) VALUES (?1,?2,?3,?4,?5)
                     ON CONFLICT(owner_kind,owner_id,path) DO UPDATE SET dim=excluded.dim, embedding=excluded.embedding",
                    params![owner_kind, owner_id, path, dim, blob],
                ).map_err(|e| format!("upsert vec: {e}"))?;
            }
            tx.commit().map_err(|e| format!("commit: {e}"))?;
            Ok(())
        })?;
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// RETRIEVAL = cosine similarity + GRAPH EXPANSION
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Clone)]
pub struct RetrievedNote {
    pub path: String,
    pub title: String,
    pub ntype: String,
    pub score: f32,
    /// How this note entered the result set: "semantic" (direct hit) or
    /// "graph:<neighbor-of-path>" (pulled in via a link from a top hit).
    pub via: String,
    pub snippet: String,
}

/// Retrieve for a query: embed it, cosine-rank all notes in this owner's scope,
/// take top-K, then EXPAND along the link graph (linked neighbors of the top
/// hits become context). This is the multiplier — it surfaces related facts the
/// query didn't literally match. The seed of Self-Gardening Memory.
pub async fn retrieve(
    db: &Db,
    owner_kind: &str,
    owner_id: &str,
    query: &str,
    top_k: usize,
    expand_hops: usize,
    embed_model: &str,
    embed_endpoint: &str,
) -> Result<Vec<RetrievedNote>, String> {
    let qvec = embed(query, embed_model, embed_endpoint).await?;

    // Load all (path, title, ntype, body, embedding) for this scope.
    struct Row {
        path: String,
        title: String,
        ntype: String,
        body: String,
        emb: Vec<f32>,
    }
    let rows: Vec<Row> = {
        let conn = db.reader()?;
        let mut stmt = conn
            .prepare(
                "SELECT n.path, n.title, n.ntype, n.body, v.embedding
                 FROM note n JOIN vec v
                   ON v.owner_kind=n.owner_kind AND v.owner_id=n.owner_id AND v.path=n.path
                 WHERE n.owner_kind=?1 AND n.owner_id=?2 AND n.status='active'",
            )
            .map_err(|e| format!("prep retrieve: {e}"))?;
        let mapped = stmt
            .query_map(params![owner_kind, owner_id], |r| {
                Ok(Row {
                    path: r.get(0)?,
                    title: r.get(1)?,
                    ntype: r.get(2)?,
                    body: r.get(3)?,
                    emb: blob_to_f32(&r.get::<_, Vec<u8>>(4)?),
                })
            })
            .map_err(|e| format!("query retrieve: {e}"))?;
        mapped.filter_map(|x| x.ok()).collect()
    };

    // Rank by cosine.
    let mut scored: Vec<(f32, &Row)> = rows
        .iter()
        .map(|r| (cosine(&qvec, &r.emb), r))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut results: Vec<RetrievedNote> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // Top-K direct semantic hits.
    let top: Vec<&Row> = scored.iter().take(top_k).map(|(_, r)| *r).collect();
    for (score, r) in scored.iter().take(top_k) {
        seen.insert(r.path.clone());
        results.push(RetrievedNote {
            path: r.path.clone(),
            title: r.title.clone(),
            ntype: r.ntype.clone(),
            score: *score,
            via: "semantic".into(),
            snippet: snippet(&r.body),
        });
    }

    // GRAPH EXPANSION: pull linked neighbors (both directions) of the top hits.
    if expand_hops > 0 && !top.is_empty() {
        let by_path: HashMap<String, &Row> =
            rows.iter().map(|r| (r.path.clone(), r)).collect();
        let mut frontier: Vec<String> = top.iter().map(|r| r.path.clone()).collect();
        let conn = db.reader()?;
        for _ in 0..expand_hops {
            let mut next: Vec<String> = Vec::new();
            for src in &frontier {
                // neighbors: outbound (src->?) and inbound (?->src), skip tag edges.
                let mut stmt = conn
                    .prepare(
                        "SELECT dst_path FROM link WHERE owner_kind=?1 AND owner_id=?2 AND src_path=?3 AND kind IN ('wikilink','embed')
                         UNION
                         SELECT src_path FROM link WHERE owner_kind=?1 AND owner_id=?2 AND dst_path=?3 AND kind IN ('wikilink','embed')",
                    )
                    .map_err(|e| format!("prep expand: {e}"))?;
                let neighbors = stmt
                    .query_map(params![owner_kind, owner_id, src], |r| r.get::<_, String>(0))
                    .map_err(|e| format!("query expand: {e}"))?;
                for nb in neighbors.flatten() {
                    if seen.contains(&nb) {
                        continue;
                    }
                    if let Some(r) = by_path.get(&nb) {
                        seen.insert(nb.clone());
                        results.push(RetrievedNote {
                            path: r.path.clone(),
                            title: r.title.clone(),
                            ntype: r.ntype.clone(),
                            score: cosine(&qvec, &r.emb),
                            via: format!("graph:{src}"),
                            snippet: snippet(&r.body),
                        });
                        next.push(nb);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
    }

    Ok(results)
}

/// A short context snippet: first non-empty, non-heading line of the body.
fn snippet(body: &str) -> String {
    for line in body.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let s: String = l.chars().take(200).collect();
        return s;
    }
    String::new()
}

/// Count rows for a quick sanity/status read (used by the ingest command).
pub fn stats(db: &Db, owner_kind: &str, owner_id: &str) -> Result<(i64, i64, i64), String> {
    let conn = db.reader()?;
    let notes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM note WHERE owner_kind=?1 AND owner_id=?2",
            params![owner_kind, owner_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("count notes: {e}"))?
        .unwrap_or(0);
    let links: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM link WHERE owner_kind=?1 AND owner_id=?2",
            params![owner_kind, owner_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("count links: {e}"))?
        .unwrap_or(0);
    let vecs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vec WHERE owner_kind=?1 AND owner_id=?2",
            params![owner_kind, owner_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("count vecs: {e}"))?
        .unwrap_or(0);
    Ok((notes, links, vecs))
}

// ===========================================================================
// SLICE 3 — EXPLICIT L2 WRITE ("remember this") + NOVELTY DEDUP
//
// An L2 atom = one durable idea as a single Memory/<slug>.md note with typed
// frontmatter + provenance. The NOVELTY GATE is what keeps a vault from rotting
// into 400 near-duplicate notes: before creating a note, embed the candidate
// and compare to existing atoms in scope. If it's near-identical (cosine >=
// threshold) to one you already know, DON'T spawn a dupe — STRENGTHEN the
// existing note instead (bump confidence, add a second provenance ref, touch
// `updated`). "Said twice" reinforces one memory; it never forks it.
//
// Writes go through the SAME gate-guarded atomic writer (vault_write) + the DB
// index update, so a new atom is immediately retrievable + linked.
// ===========================================================================

/// Default novelty cutoff (cosine). >= this to an existing atom => it's the
/// "same fact" => reinforce, don't duplicate. 0.92 per Atlas's design.
pub const NOVELTY_CUTOFF: f32 = 0.92;

/// What happened on a remember(): a fresh atom, or a reinforced existing one.
#[derive(Debug, Serialize, Clone)]
pub struct RememberResult {
    pub action: String,      // "created" | "reinforced"
    pub path: String,        // vault-relative path of the atom
    pub title: String,
    pub similarity: f32,      // best cosine to prior memory (0 if none)
    pub matched_path: String, // the atom we reinforced (empty if created)
    pub confidence: f64,
}

/// Turn free text into a filesystem-safe slug for Memory/<slug>.md.
fn slugify(text: &str) -> String {
    let mut s = String::new();
    let mut last_dash = false;
    for ch in text.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            s.push(ch);
            last_dash = false;
        } else if !last_dash && !s.is_empty() {
            s.push('-');
            last_dash = true;
        }
    }
    let s = s.trim_matches('-').to_string();
    let s: String = s.chars().take(60).collect();
    if s.is_empty() { "memory".into() } else { s.trim_matches('-').to_string() }
}

/// Today's date as YYYY-MM-DD (local). Kept here so memory.rs has no chrono dep;
/// the caller can also pass an explicit date via the command layer.
fn today_ymd() -> String {
    // Derive from system time via a tiny civil-date calc (no chrono). UTC is
    // fine for a frontmatter stamp; the command layer passes local date for the
    // daily-note path where it matters.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Compose an L2 atom's markdown: typed frontmatter + body. `source` is an
/// optional provenance block-ref (e.g. "[[2026-07-28^s3]]").
fn compose_atom(
    ntype: &str,
    agent: &str,
    pool: &str,
    confidence: f64,
    source: &str,
    tags: &[String],
    body: &str,
) -> String {
    let today = today_ymd();
    let pool_val = if pool.is_empty() { "null".to_string() } else { pool.to_string() };
    let src_val = if source.is_empty() { "null".to_string() } else { source.to_string() };
    let tag_list = if tags.is_empty() {
        format!("[{ntype}]")
    } else {
        format!("[{}]", tags.join(", "))
    };
    format!(
        "---\n\
         type: {ntype}\n\
         agent: {agent}\n\
         pool: {pool_val}\n\
         created: {today}\n\
         updated: {today}\n\
         confidence: {confidence}\n\
         source: {src_val}\n\
         supersedes: null\n\
         status: active\n\
         tags: {tag_list}\n\
         ---\n\
         {body}\n"
    )
}

/// The novelty probe: embed `text`, return the best (cosine, path, confidence)
/// among ACTIVE atoms in this scope. (Only compares against Memory atoms, i.e.
/// ntype != 'daily' — we dedup durable facts, not episodic log lines.)
async fn best_match(
    db: &Db,
    owner_kind: &str,
    owner_id: &str,
    text: &str,
    embed_model: &str,
    embed_endpoint: &str,
) -> Result<(f32, String, f64, Vec<f32>), String> {
    let cand = embed(text, embed_model, embed_endpoint).await?;
    struct Row { path: String, conf: f64, emb: Vec<f32> }
    let rows: Vec<Row> = {
        let conn = db.reader()?;
        let mut stmt = conn.prepare(
            "SELECT n.path, n.confidence, v.embedding
             FROM note n JOIN vec v
               ON v.owner_kind=n.owner_kind AND v.owner_id=n.owner_id AND v.path=n.path
             WHERE n.owner_kind=?1 AND n.owner_id=?2 AND n.status='active' AND n.ntype!='daily'",
        ).map_err(|e| format!("prep novelty: {e}"))?;
        let mapped = stmt.query_map(params![owner_kind, owner_id], |r| Ok(Row {
            path: r.get(0)?, conf: r.get(1)?, emb: blob_to_f32(&r.get::<_, Vec<u8>>(2)?),
        })).map_err(|e| format!("query novelty: {e}"))?;
        mapped.filter_map(|x| x.ok()).collect()
    };
    let mut best = (0.0f32, String::new(), 0.0f64);
    for r in &rows {
        let c = cosine(&cand, &r.emb);
        if c > best.0 { best = (c, r.path.clone(), r.conf); }
    }
    Ok((best.0, best.1, best.2, cand))
}

/// EXPLICIT "remember this": create an L2 atom OR reinforce a near-duplicate.
/// `abs_memory_dir` is the jail-resolved absolute path to the Memory/ folder;
/// `rel_memory_dir` is its vault-relative form (for note identity + links).
#[allow(clippy::too_many_arguments)]
pub async fn remember(
    db: &Db,
    owner_kind: &str,
    owner_id: &str,
    abs_memory_dir: &Path,
    rel_memory_dir: &str,
    text: &str,
    ntype: &str,
    source: &str,
    embed_model: &str,
    embed_endpoint: &str,
) -> Result<RememberResult, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("nothing to remember (empty text)".into());
    }

    // 1) Novelty gate: is this the same fact as something we already know?
    let (sim, matched_path, matched_conf, cand_emb) =
        best_match(db, owner_kind, owner_id, text, embed_model, embed_endpoint).await?;

    if sim >= NOVELTY_CUTOFF && !matched_path.is_empty() {
        // REINFORCE the existing atom: bump confidence (toward 1.0), touch
        // `updated`, append this occurrence's provenance. We do NOT rewrite the
        // note body (that risks corruption + drift) — we update the DB index
        // (confidence/updated) which is the source of ranking, and record the
        // extra provenance as a suggested-link edge so the graph reflects it.
        let new_conf = (matched_conf + (1.0 - matched_conf) * 0.34).min(0.99);
        let today = today_ymd();
        let (ok, oi, mp) = (owner_kind.to_string(), owner_id.to_string(), matched_path.clone());
        let src = source.to_string();
        db.write(move |c| {
            let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
            tx.execute(
                "UPDATE note SET confidence=?4, updated=?5 WHERE owner_kind=?1 AND owner_id=?2 AND path=?3",
                params![ok, oi, mp, new_conf, today],
            ).map_err(|e| format!("reinforce update: {e}"))?;
            if !src.is_empty() {
                tx.execute(
                    "INSERT OR IGNORE INTO link (owner_kind,owner_id,src_path,dst_path,kind) VALUES (?1,?2,?3,?4,'supersedes')",
                    params![ok, oi, mp, src],
                ).map_err(|e| format!("reinforce provenance: {e}"))?;
            }
            tx.commit().map_err(|e| format!("commit: {e}"))?;
            Ok(())
        })?;
        return Ok(RememberResult {
            action: "reinforced".into(),
            path: matched_path.clone(),
            title: text.chars().take(80).collect(),
            similarity: sim,
            matched_path,
            confidence: new_conf,
        });
    }

    // 2) CREATE a fresh atom. Slug from the text; suffix on collision so we
    //    never overwrite an existing note (create_note also refuses).
    let base_slug = slugify(text);
    let mut slug = base_slug.clone();
    let mut n = 2;
    loop {
        let candidate = abs_memory_dir.join(format!("{slug}.md"));
        if !candidate.exists() { break; }
        slug = format!("{base_slug}-{n}");
        n += 1;
        if n > 50 { return Err("could not find a free slug".into()); }
    }
    let abs_path = abs_memory_dir.join(format!("{slug}.md"));
    let rel_path = format!("{}/{}.md", rel_memory_dir.trim_end_matches('/'), slug);
    let confidence = 0.8f64;
    let tags = vec![ntype.to_string()];
    let contents = compose_atom(ntype, owner_id, "", confidence, source, &tags, text);

    // Gate-guarded atomic create (never overwrites).
    let receipt = crate::vault_write::create_note(&abs_path, &contents)?;
    let _ = receipt;

    // 3) Index the new atom immediately: note + vec (+ provenance link edge).
    let parsed = parse_note(&contents, &abs_path);
    let sha = sha_hex(&contents);
    let emb_blob = (cand_emb.len() as i64, f32_to_blob(&cand_emb));
    let (ok, oi, rp) = (owner_kind.to_string(), owner_id.to_string(), rel_path.clone());
    let src = source.to_string();
    let (title, ntype_s, agent_s, body_s, created_s, updated_s) =
        (parsed.title.clone(), parsed.ntype.clone(), owner_id.to_string(), parsed.body.clone(), today_ymd(), today_ymd());
    db.write(move |c| {
        let tx = c.transaction().map_err(|e| format!("txn: {e}"))?;
        tx.execute(
            "INSERT INTO note (owner_kind,owner_id,path,sha,title,ntype,agent,pool,confidence,status,body,created,updated,indexed_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'',?8,'active',?9,?10,?11,?12)",
            params![ok, oi, rp, sha, title, ntype_s, agent_s, confidence, body_s, created_s, updated_s, now()],
        ).map_err(|e| format!("insert atom note: {e}"))?;
        tx.execute(
            "INSERT INTO vec (owner_kind,owner_id,path,dim,embedding) VALUES (?1,?2,?3,?4,?5)",
            params![ok, oi, rp, emb_blob.0, emb_blob.1],
        ).map_err(|e| format!("insert atom vec: {e}"))?;
        if !src.is_empty() {
            tx.execute(
                "INSERT OR IGNORE INTO link (owner_kind,owner_id,src_path,dst_path,kind) VALUES (?1,?2,?3,?4,'wikilink')",
                params![ok, oi, rp, src],
            ).map_err(|e| format!("insert atom provenance: {e}"))?;
        }
        tx.commit().map_err(|e| format!("commit: {e}"))?;
        Ok(())
    })?;

    Ok(RememberResult {
        action: "created".into(),
        path: rel_path,
        title: parsed.title,
        similarity: sim,
        matched_path: String::new(),
        confidence,
    })
}
