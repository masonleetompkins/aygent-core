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
//   3. Populate the DERIVED index: note + link + vec (embedding via local Ollama
//      nomic-embed-text). sha lets us skip re-embedding unchanged files.
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
use serde::{Deserialize, Serialize};
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
// EMBEDDINGS (local Ollama nomic-embed-text — zero cloud, same as memorySearch)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OllamaEmbedReq<'a> {
    model: &'a str,
    prompt: &'a str,
}
#[derive(Deserialize)]
struct OllamaEmbedResp {
    embedding: Vec<f32>,
}

/// Embed one text via a local Ollama server. Default endpoint; overridable so a
/// power user who runs Ollama elsewhere still works.
pub async fn embed(text: &str, model: &str, endpoint: &str) -> Result<Vec<f32>, String> {
    let url = format!("{}/api/embeddings", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&OllamaEmbedReq { model, prompt: text })
        .send()
        .await
        .map_err(|e| format!("ollama embed request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("ollama embed HTTP {}", resp.status()));
    }
    let parsed: OllamaEmbedResp = resp
        .json()
        .await
        .map_err(|e| format!("ollama embed decode: {e}"))?;
    if parsed.embedding.is_empty() {
        return Err("ollama returned empty embedding".into());
    }
    Ok(parsed.embedding)
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
                if name.starts_with('.') {
                    continue; // .git, .aygent, .obsidian, .trash
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

    // Pass 2: embed (async, only changed files), then persist everything.
    for p in &pending {
        let sha = sha_hex(&p.raw);
        let unchanged = existing_sha.get(&p.path).map(|s| s == &sha).unwrap_or(false);

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
