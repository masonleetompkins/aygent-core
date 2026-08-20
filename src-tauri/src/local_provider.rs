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
// SPEED: turns are served by a persistent SESSION (one thread per model+window
// that owns the llama context). The KV cache survives across turns, so each
// turn only decodes the NEW tokens since last time (prefix reuse) instead of
// re-decoding the whole conversation — prompt work per turn is ~constant, not
// linear in chat length. Sessions idle out after a few minutes to free the KV.
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
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

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
/// Logical batch size for the persistent context. Prompt suffixes longer than
/// this are decoded in chunks of N_BATCH (the llama-server approach), so the
/// context never needs prompt-sized compute buffers AND llama.cpp's
/// GGML_ASSERT(n_tokens <= n_batch) — which abort()s the whole app — can't fire.
const N_BATCH: u32 = 2048;
/// Floor for the embed context's logical batch size (llama.cpp's default).
const MIN_BATCH_TOKENS: u32 = 512;
/// A session with no turns for this long drops its context (frees the KV cache;
/// the model weights stay cached in MODELS, so the next turn is still warm-ish).
const SESSION_IDLE_SECS: u64 = 300;

fn backend() -> Result<&'static LlamaBackend, String> {
    BACKEND.as_ref().map_err(|e| e.clone())
}

/// Decide GPU offload for a model on THIS machine. Metal caps a process's GPU
/// working set (~2/3 of RAM on Macs ≤36GB, ~75% above). If weights + KV cache
/// + compute buffers exceed that cap, a fully-offloaded model LOADS fine and
/// even decodes the prompt — then llama_decode dies mid-generation with a fatal
/// backend error ("Decode Error -3"). That's exactly what happened with a 16GB
/// 27B GGUF on a 24GB Mac: the weights alone fill the ~16GB working set.
///
/// PARTIAL offload: when the whole model doesn't fit, offload as many layers as
/// the budget allows instead of falling all the way to CPU — a model 10% over
/// budget used to drop from ~30 tok/s to ~5 (the CPU cliff). Layer count comes
/// from the GGUF header (`<arch>.block_count`); if the header doesn't expose it
/// we keep the old all-or-nothing behavior (never guess).
fn gpu_layers_for(path: &str, ctx_tokens: u32) -> u32 {
    let hw = crate::hardware::detect();
    let ram = hw.ram_gb as f64;
    let working_set = if ram <= 36.0 { ram * (2.0 / 3.0) } else { ram * 0.75 };
    let weights_gb = std::fs::metadata(path)
        .map(|m| m.len() as f64 / 1_073_741_824.0)
        .unwrap_or(0.0);
    let kv_gb = ctx_tokens as f64 * 0.00025; // ~0.25MB/token, conservative
    const OVERHEAD_GB: f64 = 1.5; // compute buffers + scratch
    if weights_gb + kv_gb + OVERHEAD_GB <= working_set {
        return u32::MAX; // everything fits — full offload
    }

    // Doesn't all fit: offload the fraction of layers the leftover budget covers.
    let Some(n_layers) = crate::gguf::read_block_count(path) else { return 0 };
    if n_layers == 0 { return 0; }
    // KV cache lives on the GPU only for offloaded layers, so it scales WITH the
    // fraction we choose. Solve: frac*(weights+kv) + OVERHEAD <= working_set.
    let budget = working_set - OVERHEAD_GB;
    if budget <= 0.0 || weights_gb + kv_gb <= 0.0 { return 0; }
    let frac = budget / (weights_gb + kv_gb);
    // Count the embedding/output tensors as one extra "layer" of weight so the
    // per-layer estimate stays conservative (they stay on GPU with any offload).
    let n = (frac * n_layers as f64 - 1.0).floor();
    if n < 1.0 { return 0; } // not even one layer fits — CPU
    (n as u32).min(n_layers)
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

// ---------------------------------------------------------------------------
// PERSISTENT SESSIONS — KV-cache reuse across turns.
//
// A `LlamaContext` borrows its `LlamaModel`, so it can't live in a static map.
// Instead each (model path, context size) gets a dedicated OS thread that OWNS
// the model Arc + context as stack locals and serves turns from a channel. The
// conversation's tokens stay in the KV cache between turns; the next turn
// prefix-matches its retokenized prompt against the cache and only decodes the
// suffix (usually just the newest user message + template framing).
//
// Correctness never depends on the cache: any divergence (edited history,
// switched conversation, truncation after overflow) shortens the matched
// prefix, we `kv_cache_seq_rm` the stale tail, and re-decode from there. Worst
// case is exactly the old behavior (full re-decode); best case is ~constant
// prompt work per turn instead of linear in conversation length.
// ---------------------------------------------------------------------------

/// One turn's work order, sent to a session thread.
struct TurnRequest {
    prompt: String,
    /// Live token stream back to the async side.
    tokens_tx: tokio::sync::mpsc::UnboundedSender<String>,
    /// Final (prompt_tokens, generated) or error.
    done_tx: std::sync::mpsc::Sender<Result<(u64, u64), String>>,
}

struct SessionEntry {
    id: u64,
    tx: std::sync::mpsc::Sender<TurnRequest>,
}

/// Live sessions keyed by "path#ctx". A session removes ITSELF on exit, but
/// only if the registry still holds its own id — a dispatcher may already have
/// replaced a dead entry, and we must not tear down the replacement.
static SESSIONS: Lazy<Mutex<HashMap<String, SessionEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static SESSION_IDS: AtomicU64 = AtomicU64::new(1);

fn session_key(path: &str, ctx_tokens: u32) -> String {
    format!("{path}#{ctx_tokens}")
}

/// Get (or spawn) the session for this model+window and return its sender.
fn session_sender(path: &str, ctx_tokens: u32) -> std::sync::mpsc::Sender<TurnRequest> {
    let key = session_key(path, ctx_tokens);
    let mut map = SESSIONS.lock().unwrap();
    if let Some(entry) = map.get(&key) {
        return entry.tx.clone();
    }
    let id = SESSION_IDS.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel::<TurnRequest>();
    let (p, k) = (path.to_string(), key.clone());
    let _ = std::thread::Builder::new()
        .name("llama-session".into())
        .spawn(move || run_session(id, k, p, ctx_tokens, rx));
    map.insert(key, SessionEntry { id, tx: tx.clone() });
    tx
}

/// Drop a session's registry entry (it may already be gone — fine).
fn remove_session(path: &str, ctx_tokens: u32) {
    SESSIONS.lock().unwrap().remove(&session_key(path, ctx_tokens));
}

/// Session thread body: owns the model + context, serves turns until idle.
fn run_session(
    id: u64,
    key: String,
    path: String,
    ctx_tokens: u32,
    rx: std::sync::mpsc::Receiver<TurnRequest>,
) {
    // Remove OUR OWN entry on the way out (not a replacement made after a
    // dispatcher declared us dead).
    let cleanup = || {
        let mut map = SESSIONS.lock().unwrap();
        if map.get(&key).is_some_and(|e| e.id == id) {
            map.remove(&key);
        }
    };

    // Heavy setup. On failure, deliver the error to every queued request so the
    // user sees a real message, then exit.
    let fail_all = |rx: std::sync::mpsc::Receiver<TurnRequest>, err: String| {
        while let Ok(req) = rx.recv_timeout(Duration::from_millis(100)) {
            let _ = req.done_tx.send(Err(err.clone()));
        }
    };
    let be = match backend() {
        Ok(b) => b,
        Err(e) => { fail_all(rx, e); cleanup(); return; }
    };
    let model = match load_model(&path, gpu_layers_for(&path, ctx_tokens)) {
        Ok(m) => m,
        Err(e) => { fail_all(rx, e); cleanup(); return; }
    };
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(ctx_tokens))
        .with_n_batch(N_BATCH);
    let mut ctx = match model.new_context(be, ctx_params) {
        Ok(c) => c,
        Err(e) => { fail_all(rx, format!("create context: {e}")); cleanup(); return; }
    };

    // Exactly the tokens currently held in the KV cache, in order.
    let mut cached: Vec<LlamaToken> = Vec::new();

    loop {
        let req = match rx.recv_timeout(Duration::from_secs(SESSION_IDLE_SECS)) {
            Ok(r) => r,
            // Idle (or all senders dropped): free the context + KV and exit.
            Err(_) => break,
        };
        let res = run_turn(&model, &mut ctx, &mut cached, ctx_tokens, &req);
        if res.is_err() {
            // A failed turn leaves the KV cache in an unknown state — never
            // trust it for prefix reuse again. Clear and start fresh.
            ctx.clear_kv_cache();
            cached.clear();
        }
        let _ = req.done_tx.send(res);
    }
    cleanup();
}

/// One turn against the persistent context: prefix-match, trim, decode suffix,
/// then the token-by-token generation loop.
fn run_turn(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    cached: &mut Vec<LlamaToken>,
    ctx_tokens: u32,
    req: &TurnRequest,
) -> Result<(u64, u64), String> {
    let mut tokens = model
        .str_to_token(&req.prompt, AddBos::Always)
        .map_err(|e| format!("tokenize: {e}"))?;

    // The context holds CTX_TOKENS total; leave room for the reply we're about
    // to generate. If the conversation has grown past that, keep the MOST RECENT
    // tokens (drop the oldest) so a long chat degrades gracefully. (This also
    // invalidates the cached prefix — the match below handles that naturally.)
    let max_prompt = (ctx_tokens as usize).saturating_sub(MAX_NEW_TOKENS + 8);
    if tokens.len() > max_prompt {
        let drop = tokens.len() - max_prompt;
        tokens.drain(0..drop);
    }
    if tokens.is_empty() {
        return Err("empty prompt after tokenize".into());
    }

    // KV-CACHE REUSE: how much of the new prompt is already decoded?
    let mut common = cached
        .iter()
        .zip(tokens.iter())
        .take_while(|(a, b)| a == b)
        .count();
    // Always leave at least the final token to decode — generation needs fresh
    // logits, and a fully-cached prompt (e.g. a regenerate) would otherwise
    // decode nothing.
    if common == tokens.len() {
        common -= 1;
    }
    // Drop the stale tail (positions >= common) from the cache, keep the prefix.
    if common < cached.len() {
        ctx.kv_cache_seq_rm(0, Some(common as u32), None)
            .map_err(|e| format!("kv trim: {e}"))?;
    }
    cached.truncate(common);

    // Decode the new suffix in N_BATCH-sized chunks (only the overall last
    // token requests logits). Chunking keeps the context's compute buffers
    // small and can never trip llama.cpp's n_tokens <= n_batch abort().
    let suffix = &tokens[common..];
    let mut batch = LlamaBatch::new(N_BATCH as usize, 1);
    let mut pos = common as i32;
    let mut last_logits_idx: i32 = 0;
    let last_overall = tokens.len() as i32 - 1;
    for chunk in suffix.chunks(N_BATCH as usize) {
        batch.clear();
        for (j, tok) in chunk.iter().enumerate() {
            let p = pos + j as i32;
            batch.add(*tok, p, &[0], p == last_overall)
                .map_err(|e| format!("batch add: {e}"))?;
        }
        ctx.decode(&mut batch).map_err(|e| decode_err("decode prompt", e))?;
        pos += chunk.len() as i32;
        last_logits_idx = chunk.len() as i32 - 1;
    }
    cached.extend_from_slice(suffix);

    // Greedy-ish sampler with light temperature for natural chat.
    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(0.7),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::greedy(),
    ]);

    let prompt_tokens = tokens.len() as u64;
    let mut generated: u64 = 0;
    let mut n_cur = tokens.len() as i32;
    let mut logits_i = last_logits_idx;
    for _ in 0..MAX_NEW_TOKENS {
        if n_cur as u32 >= ctx_tokens { break; } // window full — stop cleanly
        let token = sampler.sample(ctx, logits_i);
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
        if req.tokens_tx.send(piece).is_err() { break; } // UI hung up

        batch.clear();
        batch.add(token, n_cur, &[0], true).map_err(|e| format!("batch add: {e}"))?;
        ctx.decode(&mut batch).map_err(|e| decode_err("decode token", e))?;
        cached.push(token); // it's in the KV cache now — next turn can reuse it
        n_cur += 1;
        generated += 1;
        logits_i = 0; // single-token batches: logits always at index 0
    }
    Ok((prompt_tokens, generated))
}

/// Hand a turn to the session, retrying once if the session died between
/// lookup and send (idle-out race) or mid-flight.
fn dispatch_turn(
    path: &str,
    ctx_tokens: u32,
    prompt: String,
    tokens_tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<(u64, u64), String> {
    for _ in 0..2 {
        let sender = session_sender(path, ctx_tokens);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let req = TurnRequest { prompt: prompt.clone(), tokens_tx: tokens_tx.clone(), done_tx };
        if sender.send(req).is_err() {
            remove_session(path, ctx_tokens); // stale entry — respawn and retry
            continue;
        }
        match done_rx.recv() {
            Ok(res) => return res,
            Err(_) => {
                remove_session(path, ctx_tokens); // thread died mid-flight
                continue;
            }
        }
    }
    Err("local model session failed to start (see logs)".into())
}

/// Stream one CHAT turn from a local GGUF model. Emits TextDelta events as
/// tokens are produced, then Done. Returns the assistant content array (matching
/// the Anthropic shape: `[{type:"text", text:"..."}]`) so history is uniform.
///
/// The turn runs on the model's persistent session thread (KV cache reuse);
/// this async side just forwards tokens as they arrive.
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

    // The dispatch blocks on the session thread's reply; keep it off the runtime.
    let handle = tokio::task::spawn_blocking(move || dispatch_turn(&path, ctx, prompt, tx));

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
