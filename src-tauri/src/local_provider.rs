// AYGENT — Local model engine (llama.cpp compiled IN via llama-cpp-2).
//
// This is the "AI agent you actually own" in its truest form: a GGUF model the
// user downloaded runs entirely inside AYGENT — no Ollama, no terminal, no
// external process. The engine is compiled into the binary (Metal on macOS,
// optional CUDA on PC). Chat FIRST (Mason's call); tool-use is a fast-follow.
//
// This module emits the SAME `StreamEvent`s as the Anthropic provider, so the
// Chat UI needs zero changes — the provider-agnostic foundation pays off.
//
// NOTE ON THE BUILD: llama-cpp-2 links llama.cpp. The first compile is SLOW
// (builds the engine) — expected, not a hang. If the crate's API drifted from
// what's coded here, this is the single file to reconcile; the surface is small
// (load model → build context → decode a prompt → stream tokens).

use crate::provider::StreamEvent;
use std::path::Path;
use std::sync::Mutex;

// The heavy llama.cpp backend is initialized ONCE per process and reused. Model
// loads are cached by path so repeated turns don't re-read the GGUF off disk.
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::sampling::LlamaSampler;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;

/// Process-wide llama.cpp backend (initializes GPU/Metal once).
static BACKEND: Lazy<Result<LlamaBackend, String>> =
    Lazy::new(|| LlamaBackend::init().map_err(|e| format!("llama backend init: {e}")));

/// Loaded-model cache keyed by absolute GGUF path + offload mode (avoids
/// re-loading weights every turn — model load is the expensive part).
static MODELS: Lazy<Mutex<HashMap<String, Arc<LlamaModel>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Fallback context if the caller doesn't know the model's real window.
const DEFAULT_CTX_TOKENS: u32 = 4096;
/// Hard floor/ceiling so a wild value can't underflow or blow up memory. The
/// caller (agent_stream) already caps to what the machine can hold; this is a
/// last-resort clamp.
const MIN_CTX_TOKENS: u32 = 2048;
const MAX_CTX_TOKENS: u32 = 131072;
const MAX_NEW_TOKENS: usize = 1024;
/// Floor for the context's logical batch size (llama.cpp's default).
const MIN_BATCH_TOKENS: u32 = 512;

fn backend() -> Result<&'static LlamaBackend, String> {
    BACKEND.as_ref().map_err(|e| e.clone())
}

/// Decide GPU offload for a model on THIS machine. Metal caps a process's GPU
/// working set (~2/3 of RAM on Macs ≤36GB, ~75% above). If weights + KV cache
/// + compute buffers exceed that cap, a fully-offloaded model LOADS fine and
/// even decodes the prompt — then llama_decode dies mid-generation with a fatal
/// backend error ("Decode Error -3"). That's exactly what happened with a 16GB
/// 27B GGUF on a 24GB Mac: the weights alone fill the ~16GB working set.
/// All-or-nothing: offload every layer when the total fits, otherwise run on
/// CPU (slower, but it completes — RAM is the budget there, not the GPU cap).
fn gpu_layers_for(path: &str, ctx_tokens: u32) -> u32 {
    let hw = crate::hardware::detect();
    let ram = hw.ram_gb as f64;
    let working_set = if ram <= 36.0 { ram * (2.0 / 3.0) } else { ram * 0.75 };
    let weights_gb = std::fs::metadata(path)
        .map(|m| m.len() as f64 / 1_073_741_824.0)
        .unwrap_or(0.0);
    let kv_gb = ctx_tokens as f64 * 0.00025; // ~0.25MB/token, conservative
    const OVERHEAD_GB: f64 = 1.5; // compute buffers + scratch
    if weights_gb + kv_gb + OVERHEAD_GB <= working_set { u32::MAX } else { 0 }
}

/// Load (or fetch cached) a model from a local GGUF path with the given GPU
/// offload (u32::MAX = all layers; 0 = CPU). Cache key includes the offload so
/// a budget change after a context resize can't serve a mismatched instance.
fn load_model(path: &str, gpu_layers: u32) -> Result<Arc<LlamaModel>, String> {
    if !Path::new(path).exists() {
        return Err(format!("model file not found: {path}"));
    }
    let key = format!("{path}#{gpu_layers}");
    {
        let cache = MODELS.lock().unwrap();
        if let Some(m) = cache.get(&key) { return Ok(m.clone()); }
    }
    let be = backend()?;
    let params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    let model = LlamaModel::load_from_file(be, path, &params)
        .map_err(|e| format!("load model: {e}"))?;
    let arc = Arc::new(model);
    MODELS.lock().unwrap().insert(key, arc.clone());
    Ok(arc)
}

/// Flatten the provider-format `messages` array into a single prompt string
/// using a generic chat template. Local GGUFs vary in their exact template, but
/// this ChatML-ish framing works well across Qwen/Mistral/Llama instruct models
/// for CHAT (tool-use fast-follow will use each model's real template).
fn render_prompt(system: &str, messages: &serde_json::Value) -> String {
    let mut out = String::new();
    if !system.is_empty() {
        out.push_str(&format!("<|im_start|>system\n{system}<|im_end|>\n"));
    }
    if let Some(arr) = messages.as_array() {
        for m in arr {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            // content may be a string or an array of blocks; take text parts.
            let content = match m.get("content") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Array(blocks)) => blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            out.push_str(&format!("<|im_start|>{role}\n{content}<|im_end|>\n"));
        }
    }
    out.push_str("<|im_start|>assistant\n");
    out
}

/// Stream one CHAT turn from a local GGUF model. Emits TextDelta events as
/// tokens are produced, then Done. Returns the assistant content array (matching
/// the Anthropic shape: `[{type:"text", text:"..."}]`) so history is uniform.
///
/// Runs the (blocking, CPU/GPU-bound) decode on a blocking thread so it doesn't
/// stall the async runtime; tokens are forwarded to `on_event` via a channel.
pub async fn local_stream_turn<F: FnMut(StreamEvent)>(
    model_path: &str,
    system: &str,
    messages: &serde_json::Value,
    ctx_tokens: u32,
    mut on_event: F,
) -> Result<(serde_json::Value, String), String> {
    let prompt = render_prompt(system, messages);
    let path = model_path.to_string();
    let ctx = if ctx_tokens == 0 { DEFAULT_CTX_TOKENS } else { ctx_tokens }
        .clamp(MIN_CTX_TOKENS, MAX_CTX_TOKENS);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Decode on a blocking thread (llama.cpp is synchronous + heavy).
    let handle = tokio::task::spawn_blocking(move || decode(&path, &prompt, ctx, tx));

    // Forward tokens live as they arrive.
    let mut full = String::new();
    while let Some(t) = rx.recv().await {
        full.push_str(&t);
        on_event(StreamEvent::TextDelta { text: t });
    }
    let (prompt_tokens, generated) = handle.await.map_err(|e| format!("decode task: {e}"))??;

    // USAGE (context meter): local models have no $ cost, but the token counts +
    // the REAL (memory-capped) context window drive the fill %. We report the
    // window we actually ran with so the meter is accurate for THIS machine.
    on_event(StreamEvent::Usage { input: prompt_tokens, output: generated, cache_read: 0, cache_write: 0, context_window: ctx });
    on_event(StreamEvent::Done { stop_reason: "end_turn".into() });
    let content = serde_json::json!([{ "type": "text", "text": full }]);
    Ok((content, "end_turn".to_string()))
}

/// The synchronous decode loop (runs on a blocking thread).
fn decode(path: &str, prompt: &str, ctx_tokens: u32, tx: tokio::sync::mpsc::UnboundedSender<String>) -> Result<(u64, u64), String> {
    let be = backend()?;
    let model = load_model(path, gpu_layers_for(path, ctx_tokens))?;

    // Tokenize BEFORE building the context: the context's logical batch size
    // (n_batch) must cover the whole prompt we submit in one llama_decode call.
    // llama.cpp enforces `n_tokens <= n_batch` with a GGML_ASSERT — which calls
    // abort(), killing the entire app with SIGABRT (no catchable error). The
    // default n_batch is 2048, so this crashed as soon as a conversation grew
    // past ~2048 prompt tokens ("works for a few turns, then AYGENT dies").
    let mut tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| format!("tokenize: {e}"))?;

    // The context holds CTX_TOKENS total; leave room for the reply we're about
    // to generate. If the conversation has grown past that, keep the MOST RECENT
    // tokens (drop the oldest) so a long chat degrades gracefully instead of
    // erroring with "insufficient space" — the bug that surfaced after a few msgs.
    let max_prompt = (ctx_tokens as usize).saturating_sub(MAX_NEW_TOKENS + 8);
    if tokens.len() > max_prompt {
        let drop = tokens.len() - max_prompt;
        tokens.drain(0..drop);
    }

    // n_batch sized to the (truncated) prompt so the single-shot prompt decode
    // below never trips the assert. Always ≤ n_ctx because max_prompt < ctx.
    let n_batch = (tokens.len() as u32).max(MIN_BATCH_TOKENS);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(ctx_tokens))
        .with_n_batch(n_batch);
    let mut ctx = model
        .new_context(be, ctx_params)
        .map_err(|e| format!("create context: {e}"))?;

    // Batch capacity must cover the whole prompt (was hardcoded 512 — too small
    // once the conversation exceeded ~512 tokens). Single-token decode steps
    // below reuse this same batch, so sizing it to the prompt length is enough.
    let cap = tokens.len().max(1);
    let mut batch = LlamaBatch::new(cap, 1);
    let last = tokens.len() as i32 - 1;
    for (i, tok) in tokens.iter().enumerate() {
        batch.add(*tok, i as i32, &[0], i as i32 == last)
            .map_err(|e| format!("batch add: {e}"))?;
    }
    ctx.decode(&mut batch).map_err(|e| decode_err("decode prompt", e))?;

    // Greedy-ish sampler with light temperature for natural chat.
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(0.7),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::greedy(),
    ]);

    let prompt_tokens = tokens.len() as u64;
    let mut generated: u64 = 0;
    let mut n_cur = batch.n_tokens();
    for _ in 0..MAX_NEW_TOKENS {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);

        if model.is_eog_token(token) { break; }

        // NOTE: token_to_str is deprecated in llama-cpp-2 (harmless warning) but
        // its replacement token_to_piece takes a streaming encoding_rs::Decoder +
        // lstrip arg that isn't worth the complexity here. Keep the simple call;
        // revisit if the crate actually removes it.
        #[allow(deprecated)]
        let piece = model
            .token_to_str(token, llama_cpp_2::model::Special::Tokenize)
            .unwrap_or_default();
        if tx.send(piece).is_err() { break; } // UI hung up

        generated += 1;
        batch.clear();
        batch.add(token, n_cur, &[0], true).map_err(|e| format!("batch add: {e}"))?;
        n_cur += 1;
        ctx.decode(&mut batch).map_err(|e| decode_err("decode token", e))?;
    }
    Ok((prompt_tokens, generated))
}

/// Map a llama_decode failure to a message the USER can act on. Fatal backend
/// codes (≤ -2) on Metal almost always mean the model + context blew past the
/// GPU working-set limit — say so instead of the opaque "Decode Error -3".
fn decode_err(stage: &str, e: impl std::fmt::Display) -> String {
    let msg = e.to_string();
    if msg.contains("-2") || msg.contains("-3") {
        format!("{stage}: {msg} — the model ran out of memory on this machine. Try a smaller model or quantization (this often means the GGUF is too large for the GPU's memory limit).")
    } else {
        format!("{stage}: {msg}")
    }
}

// ---------------------------------------------------------------------------
// EMBEDDINGS — IN-PROCESS, through the SAME llama.cpp backend (no Ollama, ever).
//
// Mason's hard stipulation: AYGENT installs NOTHING outside the app. Chat
// already runs GGUF weights in-process via llama-cpp-2; embeddings are the same
// engine with `with_embeddings(true)` + `embeddings_seq_ith`. The embedding
// model is a small GGUF (nomic-embed-text, ~80MB Q4) that AYGENT auto-downloads
// to its own models dir on first use — identical mechanism to the chat catalog.
// Zero terminal, zero external process, zero user setup. THIS is "own your agent."
//
// API mirrors the official llama-cpp-2 embeddings example: build a context with
// embeddings enabled, add the tokens as one sequence, decode, read the pooled
// sequence embedding, L2-normalize (so cosine == dot and scores are stable).
// ---------------------------------------------------------------------------

use llama_cpp_2::context::params::LlamaPoolingType;

/// L2-normalize so downstream cosine is a plain dot product and magnitudes
/// don't skew similarity.
fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let mag = v.iter().fold(0.0f32, |acc, &x| x.mul_add(x, acc)).sqrt();
    if mag == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|&x| x / mag).collect()
}

/// Embed one text with a local embedding GGUF, entirely in-process. Runs the
/// heavy llama.cpp work on a blocking thread (it's synchronous + CPU/GPU-bound)
/// so it never stalls the async runtime. Returns the normalized embedding.
pub async fn embed_local(model_path: String, text: String) -> Result<Vec<f32>, String> {
    tokio::task::spawn_blocking(move || embed_blocking(&model_path, &text))
        .await
        .map_err(|e| format!("embed join: {e}"))?
}

fn embed_blocking(model_path: &str, text: &str) -> Result<Vec<f32>, String> {
    let be = backend()?;
    // Embedding GGUFs are tiny (~80MB) so this virtually always full-offloads;
    // the budget check is just consistency with the chat path.
    let model = load_model(model_path, gpu_layers_for(model_path, 2048))?;

    // Tokenize first so the context's n_batch can cover the whole sequence —
    // same GGML_ASSERT(abort) hazard as the chat path if input > n_batch.
    let mut tokens = model
        .str_to_token(text, AddBos::Always)
        .map_err(|e| format!("embed tokenize: {e}"))?;
    if tokens.is_empty() {
        return Err("embed: empty input after tokenize".into());
    }

    // Embeddings need a context built with embeddings ENABLED + MEAN pooling so
    // we get ONE vector for the whole input (not per-token). n_ctx sized to the
    // embed model's training window (nomic = 2048); a long note is truncated to
    // fit rather than erroring. n_batch must cover the full sequence because
    // pooled embeddings require the whole sequence in one ubatch.
    let n_batch = (tokens.len() as u32).max(MIN_BATCH_TOKENS);
    let ctx_params = LlamaContextParams::default()
        .with_embeddings(true)
        .with_pooling_type(LlamaPoolingType::Mean)
        .with_n_batch(n_batch)
        .with_n_ubatch(n_batch);
    let mut ctx = model
        .new_context(be, ctx_params)
        .map_err(|e| format!("embed context: {e}"))?;

    let n_ctx = ctx.n_ctx() as usize;
    // Truncate to the context window (keep the head — title + lead of the note).
    if tokens.len() > n_ctx {
        tokens.truncate(n_ctx);
    }

    let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
    batch
        .add_sequence(&tokens, 0, false)
        .map_err(|e| format!("embed add_sequence: {e}"))?;

    ctx.clear_kv_cache();
    ctx.decode(&mut batch).map_err(|e| format!("embed decode: {e}"))?;

    let emb = ctx
        .embeddings_seq_ith(0)
        .map_err(|e| format!("embed read: {e}"))?;
    Ok(l2_normalize(emb))
}
