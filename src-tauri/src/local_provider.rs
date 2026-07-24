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
const CTX_TOKENS: u32 = 4096;
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
    mut on_event: F,
) -> Result<(serde_json::Value, String), String> {
    let prompt = render_prompt(system, messages);
    let path = model_path.to_string();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TokenMsg>();

    // Decode on a blocking thread (llama.cpp is synchronous + heavy).
    let handle = tokio::task::spawn_blocking(move || decode(&path, &prompt, tx));

    // Forward tokens live as they arrive.
    let mut full = String::new();
    while let Some(msg) = rx.recv().await {
        match msg {
            TokenMsg::Text(t) => { full.push_str(&t); on_event(StreamEvent::TextDelta { text: t }); }
            TokenMsg::Err(e) => { on_event(StreamEvent::Error { text: e.clone() }); return Err(e); }
        }
    }
    handle.await.map_err(|e| format!("decode task: {e}"))??;

    on_event(StreamEvent::Done { stop_reason: "end_turn".into() });
    let content = serde_json::json!([{ "type": "text", "text": full }]);
    Ok((content, "end_turn".to_string()))
}

enum TokenMsg { Text(String), Err(String) }

/// The synchronous decode loop (runs on a blocking thread).
fn decode(path: &str, prompt: &str, tx: tokio::sync::mpsc::UnboundedSender<TokenMsg>) -> Result<(), String> {
    let be = backend()?;
    let model = load_model(path)?;

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(CTX_TOKENS));
    let mut ctx = model
        .new_context(be, ctx_params)
        .map_err(|e| format!("create context: {e}"))?;

    // Tokenize the prompt.
    let tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| format!("tokenize: {e}"))?;

    let mut batch = LlamaBatch::new(512, 1);
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
        if tx.send(TokenMsg::Text(piece)).is_err() { break; } // UI hung up

        batch.clear();
        batch.add(token, n_cur, &[0], true).map_err(|e| format!("batch add: {e}"))?;
        n_cur += 1;
        ctx.decode(&mut batch).map_err(|e| format!("decode token: {e}"))?;
    }
    Ok(())
}
