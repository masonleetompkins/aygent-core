// AYGENT — Local-model catalog (curated families, live-populated from HF).
//
// Mason's call: a CURATED list of families we trust (Qwen, Mistral, Kimi,
// Meta/Llama) but the actual model VERSIONS populate DYNAMICALLY from Hugging
// Face at runtime — so "latest Qwen" stays current without recoding.
//
// We query HF for each family's recent GGUF repos, then FILTER out junk
// (abliterated/uncensored/LoRA-adapter/imatrix-only/roleplay) so the list stays
// clean, safe, and known-good. Each entry exposes its quant files with sizes so
// the hardware predictor can score fit + speed per quant.

use serde::Serialize;

const HF_API: &str = "https://huggingface.co/api/models";

/// The families we curate. `label` is the display name; `query` seeds the HF
/// search; `params_hint` lets us estimate params when the repo id is ambiguous.
struct Family {
    key: &'static str,
    label: &'static str,
    query: &'static str,
    /// Prefer these author namespaces (well-known, high-quality GGUF packagers).
    trusted_authors: &'static [&'static str],
}

const FAMILIES: &[Family] = &[
    Family { key: "qwen",    label: "Qwen",           query: "qwen gguf instruct",
             trusted_authors: &["Qwen", "bartowski", "unsloth", "lmstudio-community"] },
    Family { key: "mistral", label: "Mistral",        query: "mistral gguf instruct",
             trusted_authors: &["mistralai", "bartowski", "unsloth", "lmstudio-community", "TheBloke"] },
    Family { key: "kimi",    label: "Kimi",           query: "kimi gguf",
             trusted_authors: &["moonshotai", "bartowski", "unsloth", "lmstudio-community"] },
    Family { key: "llama",   label: "Llama (Meta)",   query: "llama gguf instruct",
             trusted_authors: &["meta-llama", "bartowski", "unsloth", "lmstudio-community"] },
];

/// One downloadable quantization of a model.
#[derive(Debug, Clone, Serialize)]
pub struct QuantOption {
    pub quant: String,      // e.g. "Q4_K_M"
    pub filename: String,   // the .gguf file within the repo
    pub size_gb: f32,       // file size
    pub download_url: String,
}

/// A catalog entry = one model repo with its usable quants.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogModel {
    pub family: String,        // "qwen" | "mistral" | "kimi" | "llama"
    pub family_label: String,
    pub repo: String,          // HF repo id
    pub name: String,          // cleaned display name
    pub params_billions: f32,  // parsed from the id (e.g. 7, 8, 32) — best effort
    pub quants: Vec<QuantOption>,
    pub downloads: u64,
    pub updated: String,
}

/// Fetch the catalog: for each family, query HF, filter, and assemble entries
/// with their quant files. `per_family` caps how many repos we surface each.
pub async fn fetch(per_family: usize) -> Result<Vec<CatalogModel>, String> {
    let client = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let mut out = Vec::new();
    for fam in FAMILIES {
        match fetch_family(&client, fam, per_family).await {
            Ok(mut models) => out.append(&mut models),
            Err(_) => { /* one family failing shouldn't kill the whole catalog */ }
        }
    }
    Ok(out)
}

async fn fetch_family(
    client: &reqwest::Client,
    fam: &Family,
    limit: usize,
) -> Result<Vec<CatalogModel>, String> {
    // Ask for recent GGUF repos in this family, newest first, WITH siblings
    // (the file list) so we can read quant filenames + sizes in one round trip.
    let url = format!(
        "{HF_API}?search={}&filter=gguf&sort=downloads&direction=-1&limit=40&full=true",
        urlencoding(fam.query)
    );
    let resp = client.get(&url).send().await.map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("hf {}", resp.status()));
    }
    let arr: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    let Some(list) = arr.as_array() else { return Ok(vec![]) };

    let mut models = Vec::new();
    for repo in list {
        if models.len() >= limit { break; }
        let id = repo.get("id").and_then(|i| i.as_str()).unwrap_or("");
        if id.is_empty() { continue; }
        let author = id.split('/').next().unwrap_or("");
        let lower = id.to_lowercase();

        // FILTER junk: keep it clean + safe.
        if is_junk(&lower) { continue; }
        // Prefer trusted authors; skip unknown authors to guarantee quality.
        if !fam.trusted_authors.iter().any(|a| a.eq_ignore_ascii_case(author)) { continue; }

        let quants = extract_quants(repo, id);
        if quants.is_empty() { continue; } // no usable single-file gguf → skip

        models.push(CatalogModel {
            family: fam.key.to_string(),
            family_label: fam.label.to_string(),
            repo: id.to_string(),
            name: clean_name(id),
            params_billions: parse_params(&lower),
            quants,
            downloads: repo.get("downloads").and_then(|d| d.as_u64()).unwrap_or(0),
            updated: repo.get("lastModified").and_then(|d| d.as_str()).unwrap_or("").to_string(),
        });
    }
    Ok(models)
}

/// Reject repos that aren't clean instruct models suitable for a general user.
fn is_junk(lower: &str) -> bool {
    const BAD: &[&str] = &[
        "abliterated", "uncensored", "heretic", "lora", "adapter", "-sft",
        "roleplay", "rp-", "erotic", "nsfw", "toxic", "base_model", "draft",
        "vision", "vl-", "-vl", "audio", "embedding", "reranker",
    ];
    BAD.iter().any(|b| lower.contains(b))
}

/// Pull single-file GGUF quants (skip split/sharded and imatrix-only files).
/// Prefer the popular balanced quants users actually want.
fn extract_quants(repo: &serde_json::Value, repo_id: &str) -> Vec<QuantOption> {
    let Some(sibs) = repo.get("siblings").and_then(|s| s.as_array()) else { return vec![] };
    // Preference order — surface a small, sensible set (not every exotic quant).
    const WANT: &[&str] = &["Q4_K_M", "Q5_K_M", "Q8_0", "Q6_K", "Q4_0", "Q3_K_M"];

    let mut found: Vec<QuantOption> = Vec::new();
    for sib in sibs {
        let fname = sib.get("rfilename").and_then(|f| f.as_str()).unwrap_or("");
        let up = fname.to_uppercase();
        if !up.ends_with(".GGUF") { continue; }
        // skip multi-part/sharded files (e.g. "-00001-of-00003")
        if fname.contains("-of-") || up.contains("SPLIT") { continue; }
        let Some(&quant) = WANT.iter().find(|q| up.contains(&q.to_uppercase().as_str())) else { continue };
        if found.iter().any(|q| q.quant == quant) { continue; } // one per quant
        let size_gb = sib.get("size").and_then(|s| s.as_u64())
            .map(|b| (b as f64 / 1_073_741_824.0) as f32)
            .unwrap_or(0.0);
        found.push(QuantOption {
            quant: quant.to_string(),
            filename: fname.to_string(),
            size_gb,
            download_url: format!("https://huggingface.co/{repo_id}/resolve/main/{fname}"),
        });
    }
    // Sort by our preference order for a tidy UI.
    found.sort_by_key(|q| WANT.iter().position(|w| *w == q.quant).unwrap_or(99));
    found
}

/// Best-effort model display name from a repo id.
fn clean_name(id: &str) -> String {
    let tail = id.split('/').last().unwrap_or(id);
    tail.replace("-GGUF", "").replace("-gguf", "").replace('-', " ")
}

/// Parse parameter count (billions) from the id, e.g. "7b", "8B", "32b".
fn parse_params(lower: &str) -> f32 {
    // find a number immediately followed by 'b'
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'b' && i > 0 {
            // walk back over digits (and a possible decimal point)
            let mut j = i;
            while j > 0 && (bytes[j - 1].is_ascii_digit() || bytes[j - 1] == b'.') { j -= 1; }
            if j < i {
                if let Ok(n) = lower[j..i].parse::<f32>() {
                    if n >= 0.5 && n <= 500.0 { return n; }
                }
            }
        }
        i += 1;
    }
    7.0 // sensible default when unparseable
}

/// Minimal URL-encoding for the query string (space + a few reserved chars).
fn urlencoding(s: &str) -> String {
    s.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
        _ => format!("%{:02X}", c as u32),
    }).collect()
}
