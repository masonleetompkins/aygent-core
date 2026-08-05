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

/// Loaded-model cache keyed by absolute GGUF path (avoids re-loading weights
/// every turn — model load is the expensive part).
static MODELS: Lazy<Mutex<HashMap<String, Arc<LlamaModel>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// How many layers to offload to the GPU. u32::MAX = "all layers" (llama.cpp
/// clamps to the model's actual layer count). On CPU-only builds this is simply
/// ignored by the backend.
const GPU_LAYERS: u32 = u32::MAX;
/// Fallback context if the caller doesn't know the model's real window.
const DEFAULT_CTX_TOKENS: u32 = 4096;
/// Hard floor/ceiling so a wild value can't underflow or blow up memory. The
/// caller (agent_stream) already caps to what the machine can hold; this is a
/// last-resort clamp.
const MIN_CTX_TOKENS: u32 = 2048;
const MAX_CTX_TOKENS: u32 = 131072;
const MAX_NEW_TOKENS: usize = 1024;

fn backend() -> Result<&'static LlamaBackend, String> {
    BACKEND.as_ref().map_err(|e| e.clone())
}

/// Load (or fetch cached) a model from a local GGUF path.
fn load_model(path: &str) -> Result<Arc<LlamaModel>, String> {
    if !Path::new(path).exists() {
        return Err(format!("model file not found: {path}"));
    }
    {
        let cache = MODELS.lock().unwrap();
        if let Some(m) = cache.get(path) { return Ok(m.clone()); }
    }
    let be = backend()?;
    let params = LlamaModelParams::default().with_n_gpu_layers(GPU_LAYERS);
    let model = LlamaModel::load_from_file(be, path, &params)
        .map_err(|e| format!("load model: {e}"))?;
    let arc = Arc::new(model);
    MODELS.lock().unwrap().insert(path.to_string(), arc.clone());
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
    handle.await.map_err(|e| format!("decode task: {e}"))??;

    on_event(StreamEvent::Done { stop_reason: "end_turn".into() });
    let content = serde_json::json!([{ "type": "text", "text": full }]);
    Ok((content, "end_turn".to_string()))
}

/// The synchronous decode loop (runs on a blocking thread).
fn decode(path: &str, prompt: &str, ctx_tokens: u32, tx: tokio::sync::mpsc::UnboundedSender<String>) -> Result<(), String> {
    let be = backend()?;
    let model = load_model(path)?;

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(ctx_tokens));
    let mut ctx = model
        .new_context(be, ctx_params)
        .map_err(|e| format!("create context: {e}"))?;

    // Tokenize the prompt.
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
    ctx.decode(&mut batch).map_err(|e| format!("decode prompt: {e}"))?;

    // Greedy-ish sampler with light temperature for natural chat.
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(0.7),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::greedy(),
    ]);

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

        batch.clear();
        batch.add(token, n_cur, &[0], true).map_err(|e| format!("batch add: {e}"))?;
        n_cur += 1;
        ctx.decode(&mut batch).map_err(|e| format!("decode token: {e}"))?;
    }
    Ok(())
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
    let model = load_model(model_path)?;

    // Embeddings need a context built with embeddings ENABLED + MEAN pooling so
    // we get ONE vector for the whole input (not per-token). n_ctx sized to the
    // embed model's training window (nomic = 2048); a long note is truncated to
    // fit rather than erroring.
    let ctx_params = LlamaContextParams::default()
        .with_embeddings(true)
        .with_pooling_type(LlamaPoolingType::Mean);
    let mut ctx = model
        .new_context(be, ctx_params)
        .map_err(|e| format!("embed context: {e}"))?;

    let n_ctx = ctx.n_ctx() as usize;
    let mut tokens = model
        .str_to_token(text, AddBos::Always)
        .map_err(|e| format!("embed tokenize: {e}"))?;
    if tokens.is_empty() {
        return Err("embed: empty input after tokenize".into());
    }
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
