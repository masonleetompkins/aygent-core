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
    pub tier: String,       // friendly label: "Recommended" | "Higher quality"
    pub quant: String,      // raw code e.g. "Q4_K_M" (shown small/secondary)
    pub filename: String,   // the .gguf file within the repo
    pub size_gb: f32,       // ESTIMATED file size (params × bits-per-weight)
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

        let params = parse_params(&lower);
        let quants = extract_quants(repo, id, params);
        if quants.is_empty() { continue; } // no usable single-file gguf → skip

        models.push(CatalogModel {
            family: fam.key.to_string(),
            family_label: fam.label.to_string(),
            repo: id.to_string(),
            name: clean_name(id),
            params_billions: params,
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

/// Pull GGUF quants and collapse them to just TWO friendly choices for an
/// inexperienced user: a "Recommended" balanced quant (Q4_K_M) and a "Higher
/// quality" one (Q6_K/Q8_0). We do NOT surface all six cryptic codes.
///
/// SIZE: the bulk HF search endpoint returns `siblings` WITHOUT file sizes, so
/// we ESTIMATE size from params × bits-per-weight (accurate within a few %).
/// This avoids a slow extra network round-trip per model and never shows 0.0GB.
fn extract_quants(repo: &serde_json::Value, repo_id: &str, params_b: f32) -> Vec<QuantOption> {
    let Some(sibs) = repo.get("siblings").and_then(|s| s.as_array()) else { return vec![] };

    // Which quant fills each friendly "tier", in order of preference.
    let recommended = ["Q4_K_M", "Q4_0", "Q3_K_M"];
    let higher = ["Q6_K", "Q8_0", "Q5_K_M"];

    // Collect the single-file gguf names actually present in the repo.
    let present: Vec<(String, String)> = sibs.iter().filter_map(|sib| {
        let fname = sib.get("rfilename").and_then(|f| f.as_str())?;
        let up = fname.to_uppercase();
        if !up.ends_with(".GGUF") { return None; }
        if fname.contains("-of-") || up.contains("SPLIT") { return None; } // no sharded files
        Some((up, fname.to_string()))
    }).collect();

    // Find the first present filename matching any quant in `prefs`.
    let pick = |prefs: &[&str]| -> Option<(String, String)> {
        for q in prefs {
            if let Some((_, fname)) = present.iter().find(|(up, _)| up.contains(&q.to_uppercase())) {
                return Some((q.to_string(), fname.clone()));
            }
        }
        None
    };

    let mut out = Vec::new();
    if let Some((quant, fname)) = pick(&recommended) {
        out.push(QuantOption {
            tier: "Recommended".into(),
            quant: quant.clone(),
            size_gb: estimate_size_gb(params_b, &quant),
            download_url: format!("https://huggingface.co/{repo_id}/resolve/main/{fname}"),
            filename: fname,
        });
    }
    if let Some((quant, fname)) = pick(&higher) {
        // don't duplicate if the same file somehow filled both tiers
        if !out.iter().any(|o| o.filename == fname) {
            out.push(QuantOption {
                tier: "Higher quality".into(),
                quant: quant.clone(),
                size_gb: estimate_size_gb(params_b, &quant),
                download_url: format!("https://huggingface.co/{repo_id}/resolve/main/{fname}"),
                filename: fname,
            });
        }
    }
    out
}

/// Estimate GGUF file size (GB) from parameter count + quant. Based on the
/// quant's average bits-per-weight (llama.cpp published figures), plus a small
/// overhead for metadata/embeddings. Good to within a few percent.
fn estimate_size_gb(params_b: f32, quant: &str) -> f32 {
    let bpw = match quant {
        "Q2_K" => 3.35, "Q3_K_M" => 3.91, "Q4_0" => 4.55, "Q4_K_M" => 4.85,
        "Q5_K_M" => 5.69, "Q6_K" => 6.56, "Q8_0" => 8.5, _ => 5.0,
    };
    // bytes = params * bpw / 8 ; then to GB, + ~5% overhead
    let bytes = (params_b as f64) * 1e9 * (bpw / 8.0);
    ((bytes * 1.05) / 1_073_741_824.0) as f32
}

/// Best-effort model display name from a repo id.
fn clean_name(id: &str) -> String {
    let tail = id.split('/').last().unwrap_or(id);
    tail.replace("-GGUF", "").replace("-gguf", "").replace('-', " ")
}

/// Parse parameter count (billions) from the id, e.g. "7b", "8B", "32b", "30b-a3b".
/// IMPORTANT: must NOT read a version number like the "3" in "Qwen3". We only
/// accept a number+`b` where the number is a plausible param count AND is not
/// glued to a family word (Qwen3, Llama3). Strategy: collect ALL <num>b matches,
/// then pick the LARGEST plausible one — the real size (e.g. "30" in
/// "qwen3-coder-30b-a3b") always dominates the version digit and the small MoE
/// active-params suffix.
fn parse_params(lower: &str) -> f32 {
    let bytes = lower.as_bytes();
    let mut best = 0.0f32;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'b' && i > 0 {
            // Char after 'b' must be a boundary (not alphanumeric) so "bf16" and
            // "blah" don't match; "7b-", "7b.", or "7b" at end-of-string are ok.
            let after_ok = bytes.get(i + 1).map(|c| !c.is_ascii_alphanumeric()).unwrap_or(true);
            // Walk back over the digits (and a possible decimal point).
            let mut j = i;
            while j > 0 && (bytes[j - 1].is_ascii_digit() || bytes[j - 1] == b'.') { j -= 1; }
            // Char before the number must be a boundary too, so the digits aren't
            // the tail of a family word like "qwen3" (the '3' is preceded by 'n').
            let before_ok = j == 0 || !bytes[j - 1].is_ascii_alphabetic();
            if after_ok && before_ok && j < i {
                if let Ok(n) = lower[j..i].parse::<f32>() {
                    // Largest plausible <num>b wins: the real size (e.g. "30" in
                    // "qwen3-coder-30b-a3b") dominates the version digit and the
                    // small MoE active-params suffix.
                    if n >= 0.5 && n <= 500.0 && n > best { best = n; }
                }
            }
        }
        i += 1;
    }
    if best > 0.0 { best } else { 7.0 } // sensible default when unparseable
}

/// Minimal URL-encoding for the query string (space + a few reserved chars).
fn urlencoding(s: &str) -> String {
    s.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
        _ => format!("%{:02X}", c as u32),
    }).collect()
}
