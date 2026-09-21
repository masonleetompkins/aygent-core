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
    /// Context window in tokens (how much conversation the model can hold),
    /// from known family specs — best effort, 0 = unknown.
    pub context_tokens: u32,
    pub quants: Vec<QuantOption>,
    pub downloads: u64,
    pub updated: String,
    pub custom_code: bool, // repo ships its own loader code (serve needs consent)
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

        // Curated default list: technically-usable AND standard instruct models.
        if is_unusable(&lower) || is_off_catalog(&lower) { continue; }
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
            context_tokens: context_window(&lower),
            quants,
            downloads: repo.get("downloads").and_then(|d| d.as_u64()).unwrap_or(0),
            updated: repo.get("lastModified").and_then(|d| d.as_str()).unwrap_or("").to_string(),
            custom_code: has_custom_loader(repo),
        });
    }
    Ok(models)
}

/// Reject repos that WON'T WORK as a chat model in AYGENT: adapters and base
/// models (no instruct tuning), and non-chat modalities (vision/audio/
/// embedding/reranker GGUFs). This is a TECHNICAL filter, not a content one —
/// these downloads would simply not function as a chat agent.
fn is_unusable(lower: &str) -> bool {
    const BAD: &[&str] = &[
        "lora", "adapter", "-sft", "base_model", "draft",
        "vision", "vl-", "-vl", "audio", "embedding", "reranker",
    ];
    BAD.iter().any(|b| lower.contains(b))
}

/// Content-style tags (uncensored/abliterated/roleplay...) — kept OUT of the
/// curated default catalog so the out-of-the-box list stays predictable, but
/// deliberately NOT applied to search/lookup: the user owns this machine and
/// can download any model they explicitly go looking for (Mason 08-19 —
/// "if I'm using local models, I understand the risk").
fn is_off_catalog(lower: &str) -> bool {
    const TAGS: &[&str] = &[
        "abliterated", "uncensored", "heretic", "roleplay", "rp-",
        "erotic", "nsfw", "toxic",
    ];
    TAGS.iter().any(|t| lower.contains(t))
}

/// Pull GGUF quants and collapse them to THREE friendly choices for an
/// inexperienced user: a "Recommended" balanced quant (Q4_K_M), a "Higher
/// quality" one (Q6_K/Q8_0), and an "Efficient" small quant (IQ4_XS → Q2_K)
/// that makes big models viable on small machines (Mason: 24GB Macs never even
/// SAW the 2/3-bit quants that would run a 27B). Still not all N cryptic codes.
///
/// SIZE: the bulk HF search endpoint returns `siblings` WITHOUT file sizes, so
/// we ESTIMATE size from params × bits-per-weight (accurate within a few %).
/// This avoids a slow extra network round-trip per model and never shows 0.0GB.
fn extract_quants(repo: &serde_json::Value, repo_id: &str, params_b: f32) -> Vec<QuantOption> {
    let Some(sibs) = repo.get("siblings").and_then(|s| s.as_array()) else { return vec![] };

    // Which quant fills each friendly "tier", in order of preference.
    let recommended = ["Q4_K_M", "Q4_0", "Q3_K_M"];
    let higher = ["Q6_K", "Q8_0", "Q5_K_M"];
    // Smallest usable quants first by quality-per-bit: IQ4_XS ≈ Q4 quality at
    // ~12% less memory; the IQ3/IQ2 i-quants degrade gracefully and are the
    // difference between "won't fit" and "runs" for 20B+ models on 24GB.
    let efficient = ["IQ4_XS", "IQ3_M", "Q3_K_S", "IQ3_XXS", "Q2_K", "IQ2_M", "IQ2_XS"];

    // Collect the single-file gguf names actually present in the repo.
    let present: Vec<(String, String)> = sibs.iter().filter_map(|sib| {
        let fname = sib.get("rfilename").and_then(|f| f.as_str())?;
        let up = fname.to_uppercase();
        if !up.ends_with(".GGUF") { return None; }
        if fname.contains("-of-") || up.contains("SPLIT") { return None; } // no sharded files
        Some((up, fname.to_string()))
    }).collect();

    // Find the first present filename matching any quant in `prefs`. The match
    // requires the quant code as a delimited token (".Q2_K." matches "Q2_K" but
    // "Q2_K_S" and "IQ2_K" do not) so tiers never grab a neighboring quant.
    let has_token = |up: &str, q: &str| -> bool {
        let qu = q.to_uppercase();
        let b = up.as_bytes();
        let mut start = 0;
        while let Some(i) = up[start..].find(&qu) {
            let i = start + i;
            let before_ok = i == 0 || !b[i - 1].is_ascii_alphanumeric();
            let j = i + qu.len();
            let after_ok = j >= b.len() || (!b[j].is_ascii_alphanumeric() && b[j] != b'_');
            if before_ok && after_ok { return true; }
            start = i + 1;
        }
        false
    };
    let pick = |prefs: &[&str]| -> Option<(String, String)> {
        for q in prefs {
            if let Some((_, fname)) = present.iter().find(|(up, _)| has_token(up, q)) {
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
    if let Some((quant, fname)) = pick(&efficient) {
        if !out.iter().any(|o| o.filename == fname) {
            out.push(QuantOption {
                tier: "Efficient".into(),
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
        "IQ2_XS" => 2.31, "IQ2_M" => 2.7, "IQ3_XXS" => 3.06, "Q2_K" => 3.35,
        "IQ3_M" => 3.66, "Q3_K_S" => 3.5, "Q3_K_M" => 3.91, "IQ4_XS" => 4.25,
        "Q4_0" => 4.55, "Q4_K_M" => 4.85, "Q5_K_M" => 5.69, "Q6_K" => 6.56,
        "Q8_0" => 8.5, _ => 5.0,
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
pub fn parse_params(lower: &str) -> f32 {
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

/// Known context windows by family/version (tokens). Best-effort from each
/// family's published specs; 0 = unknown (UI shows "—"). Order matters: more
/// specific patterns first.
pub fn context_window(lower: &str) -> u32 {
    const K: u32 = 1024;
    let rules: &[(&str, u32)] = &[
        // Qwen
        ("qwen3", 32 * K),            // Qwen3 base 32k (128k w/ yarn — be conservative)
        ("qwen2.5-coder", 32 * K),
        ("qwen2.5-1m", 1024 * K),
        ("qwen2.5", 128 * K),
        ("qwen2", 32 * K),
        // Mistral
        ("mistral-7b-instruct-v0.1", 8 * K),
        ("mistral-7b-instruct-v0.2", 32 * K),
        ("mistral-7b-instruct-v0.3", 32 * K),
        ("mistral-small", 32 * K),    // Small 3.x = 32k (128k on 3.1+ but varies; conservative)
        ("mistral-nemo", 128 * K),
        ("mixtral", 32 * K),
        ("mistral", 32 * K),
        // Kimi
        ("kimi-k2", 128 * K),
        ("kimi", 128 * K),
        // Meta Llama
        ("llama-3.3", 128 * K),
        ("llama-3.2", 128 * K),
        ("llama-3.1", 128 * K),
        ("llama-3", 8 * K),
        ("llama-2", 4 * K),
        ("llama", 8 * K),
    ];
    for (pat, ctx) in rules {
        if lower.contains(pat) { return *ctx; }
    }
    0
}

/// Search Hugging Face for GGUF repos matching an arbitrary query (power-user path).
/// MLX (Apple-silicon safetensors) hits are appended after the GGUF results.
/// Unlike the curated `fetch`, this does NOT restrict to trusted authors — so a user
/// can find any model they know exists on HF (e.g. a DeepSeek or Gemma GGUF pack).
/// Still filters junk + requires at least one usable single-file GGUF quant.
pub async fn search(query: String, limit: usize) -> Result<Vec<CatalogModel>, String> {
    let q = query.trim();
    if q.is_empty() { return Ok(vec![]); }
    if q.contains('/') && !q.contains(' ') {
        // Looks like a repo id pasted into search — delegate to lookup for exact match.
        if let Ok(one) = lookup(q.to_string()).await { return Ok(vec![one]); }
    }
    let client = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let url = format!(
        "{HF_API}?search={}&filter=gguf&sort=downloads&direction=-1&limit={}&full=true",
        urlencoding(q), limit.clamp(1, 40)
    );
    let resp = client.get(&url).send().await.map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() { return Err(format!("hf {}", resp.status())); }
    let arr: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    let Some(list) = arr.as_array() else { return Ok(vec![]); };
    let mut out = Vec::new();
    for repo in list {
        if out.len() >= limit { break; }
        let id = repo.get("id").and_then(|i| i.as_str()).unwrap_or("");
        if id.is_empty() { continue; }
        let lower = id.to_lowercase();
        // Search is the power-user path: only skip repos that literally will not
        // run as a chat model. No content filtering — the user searched for it.
        if is_unusable(&lower) { continue; }
        let params = parse_params(&lower);
        let quants = extract_quants(repo, id, params);
        if quants.is_empty() { continue; }
        let (family, family_label) = infer_family(&lower);
        out.push(CatalogModel {
            family: family.to_string(),
            family_label: family_label.to_string(),
            repo: id.to_string(),
            name: clean_name(id),
            params_billions: params,
            context_tokens: context_window(&lower),
            quants,
            downloads: repo.get("downloads").and_then(|d| d.as_u64()).unwrap_or(0),
            updated: repo.get("lastModified").and_then(|d| d.as_str()).unwrap_or("").to_string(),
            custom_code: has_custom_loader(repo),
        });
    }
    // MLX pass (Apple-silicon weights): same query WITHOUT the gguf filter,
    // keeping only repos that look like MLX + carry safetensors. Appended after
    // GGUF hits so existing behavior is unchanged.
    if out.len() < limit {
        let seen: std::collections::HashSet<String> = out.iter().map(|m| m.repo.clone()).collect();
        let url = format!(
            "{HF_API}?search={}&sort=downloads&direction=-1&limit=40&full=true",
            urlencoding(q)
        );
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(arr) = resp.json::<serde_json::Value>().await {
                if let Some(list) = arr.as_array() {
                    for repo in list {
                        if out.len() >= limit { break; }
                        let id = repo.get("id").and_then(|i| i.as_str()).unwrap_or("");
                        if id.is_empty() || seen.contains(id) { continue; }
                        let lower = id.to_lowercase();
                        if is_unusable(&lower) { continue; }
                        // Skip repos the GGUF pass already claimed (dual-format).
                        let params = parse_params(&lower);
                        if !extract_quants(repo, id, params).is_empty() { continue; }
                        if let Some(entry) = mlx_entry(repo, id, &lower, params) {
                            out.push(entry);
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Lookup one exact HF repo by id (e.g. "bartowski/Qwen3-14B-GGUF" or
/// "mlx-community/Qwen3-4B-4bit") and return its catalog entry. Used for the
/// paste-a-repo-ID power-user path.
pub async fn lookup(repo_id: String) -> Result<CatalogModel, String> {
    let id = repo_id.trim().trim_matches('/').to_string();
    if !id.contains('/') { return Err("repo id should be 'author/name' (e.g. bartowski/Qwen3-14B-GGUF)".into()); }
    let client = reqwest::Client::builder()
        .user_agent("aygent/0.1")
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let url = format!("{HF_API}/{}", id);
    let resp = client.get(&url).send().await.map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() { return Err(format!("hf {} — repo not found?", resp.status())); }
    let repo: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    let lower = id.to_lowercase();
    // Allow even junk-tagged lookups to surface (user asked for it explicitly).
    let params = parse_params(&lower);
    let quants = extract_quants(&repo, &id, params);
    if quants.is_empty() {
        // No GGUF — maybe an MLX (safetensors) repo pasted for the MLX runner.
        if let Some(entry) = mlx_entry(&repo, &id, &lower, params) { return Ok(entry); }
        return Err("no usable weights found in that repo (needs a single-file GGUF quant or MLX safetensors)".into());
    }
    let (family, family_label) = infer_family(&lower);
    Ok(CatalogModel {
        family: family.to_string(),
        family_label: family_label.to_string(),
        repo: id.clone(),
        name: clean_name(&id),
        params_billions: params,
        context_tokens: context_window(&lower),
        quants,
        downloads: repo.get("downloads").and_then(|d| d.as_u64()).unwrap_or(0),
        updated: repo.get("lastModified").and_then(|d| d.as_str()).unwrap_or("").to_string(),
        custom_code: has_custom_loader(&repo),
    })
}

/// Is this repo plausibly Apple-MLX weights? Signal = "mlx" in the id (the
/// community convention: mlx-community/*, *-mlx-4bit) or the mlx-community
/// author — PLUS actual .safetensors weight files. AWQ/GPTQ/EXL2 (CUDA-only)
/// repos don't carry the mlx marker, so they stay out.
fn is_mlx_repo(id_lower: &str, repo: &serde_json::Value) -> bool {
    let author = id_lower.split('/').next().unwrap_or("");
    let marked = id_lower.contains("mlx") || author == "mlx-community";
    if !marked { return false; }
    repo.get("siblings").and_then(|s| s.as_array()).map(|sibs| {
        sibs.iter().any(|sib| sib.get("rfilename").and_then(|f| f.as_str())
            .map(|f| f.to_lowercase().ends_with(".safetensors")).unwrap_or(false))
    }).unwrap_or(false)
}

/// Bits-per-weight parsed from an MLX repo name ("2bit" in `...-mlx-2bit`,
/// "int4", "fp16"/"bf16"). Defaults to 4.0 (the community's standard MLX
/// quant) when the name doesn't say.
fn parse_mlx_bits(lower: &str) -> f32 {
    for (tag, bits) in [("8bit", 8.0), ("6bit", 6.0), ("5bit", 5.0), ("4bit", 4.0),
                        ("3bit", 3.0), ("2bit", 2.0), ("int8", 8.0), ("int4", 4.0),
                        ("fp16", 16.0), ("bf16", 16.0)] {
        if lower.contains(tag) { return bits; }
    }
    4.0
}

/// Build a catalog entry for an MLX repo: ONE "quant" = the whole repo (MLX
/// weights download as a set via `mlx_pull`, not a single file). filename and
/// download_url stay empty — the UI pulls by repo id when family == "mlx".
fn mlx_entry(repo: &serde_json::Value, id: &str, lower: &str, params: f32) -> Option<CatalogModel> {
    if !is_mlx_repo(lower, repo) { return None; }
    let bits = parse_mlx_bits(lower);
    let size_gb = ((params as f64) * 1e9 * (bits as f64 / 8.0) * 1.02 / 1_073_741_824.0) as f32;
    let label = match bits as i32 {
        2 => "2-bit", 3 => "3-bit", 4 => "4-bit", 5 => "5-bit",
        6 => "6-bit", 8 => "8-bit", 16 => "FP16", _ => "MLX",
    };
    let tail = id.split('/').last().unwrap_or(id);
    let name = tail.replace("-mlx-", "-").trim_end_matches("-mlx").replace('-', " ");
    Some(CatalogModel {
        family: "mlx".to_string(),
        family_label: "MLX".to_string(),
        repo: id.to_string(),
        name,
        params_billions: params,
        context_tokens: context_window(lower),
        quants: vec![QuantOption {
            tier: "MLX".into(), quant: label.into(), filename: String::new(),
            size_gb, download_url: String::new(),
        }],
        downloads: repo.get("downloads").and_then(|d| d.as_u64()).unwrap_or(0),
        updated: repo.get("lastModified").and_then(|d| d.as_str()).unwrap_or("").to_string(),
        custom_code: has_custom_loader(repo),
    })
}

/// A repo ships custom loader code when it carries its own runtime Python
/// (runtime/*.py) or the pack contract doc. Serving those repos executes
/// third-party code - allowed only with explicit per-repo consent.
fn has_custom_loader(repo: &serde_json::Value) -> bool {
    repo.get("siblings").and_then(|v| v.as_array()).map(|sibs| {
        sibs.iter().filter_map(|s| s.get("rfilename").and_then(|f| f.as_str())).any(|f| {
            let l = f.to_lowercase();
            (l.starts_with("runtime/") && l.ends_with(".py")) || l == "pack-runtime.md"
        })
    }).unwrap_or(false)
}

fn infer_family(lower: &str) -> (&'static str, &'static str) {
    if lower.contains("qwen") { return ("qwen", "Qwen"); }
    if lower.contains("mistral") || lower.contains("mixtral") { return ("mistral", "Mistral"); }
    if lower.contains("kimi") { return ("kimi", "Kimi"); }
    if lower.contains("llama") { return ("llama", "Llama (Meta)"); }
    if lower.contains("deepseek") { return ("deepseek", "DeepSeek"); }
    if lower.contains("gemma") { return ("gemma", "Gemma"); }
    if lower.contains("phi") { return ("phi", "Phi"); }
    ("other", "Other")
}

/// Minimal URL-encoding for the query string (space + a few reserved chars).
fn urlencoding(s: &str) -> String {
    s.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
        _ => format!("%{:02X}", c as u32),
    }).collect()
}
