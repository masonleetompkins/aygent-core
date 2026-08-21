// AYGENT — Hardware detection + local-model performance prediction.
//
// Runs on the RUNTIME machine (the Mac/PC the app is installed on — NOT the
// build box). Detects CPU / RAM / GPU / accelerator, then for a given model
// size + quantization predicts whether it will FIT and roughly how FAST it will
// run. These are ESTIMATES (directional guidance), never benchmarks — labeled
// as such in the UI.
//
// The prediction model is deliberately simple and honest:
//   - "Will it fit?"  = required memory (weights + KV cache + overhead) vs the
//     memory the accelerator can actually use.
//   - "How fast?"     = a tier (Great/Usable/Slow) + a tok/s RANGE derived from
//     accelerator class and whether the model fully offloads.

use serde::Serialize;

/// What the runtime machine looks like. `accel` is the best available path.
#[derive(Debug, Clone, Serialize)]
pub struct HardwareInfo {
    pub cpu: String,
    pub cpu_cores: usize,
    pub ram_gb: f32,
    pub gpu: String,
    /// Memory (GB) usable for model weights on the accelerator. On Apple Silicon
    /// this is UNIFIED memory (shared with RAM) so it's ~ram_gb; on a discrete
    /// GPU it's dedicated VRAM; CPU-only = system RAM.
    pub accel_mem_gb: f32,
    pub accel: Accelerator,
    /// Human summary line for the Settings header.
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Accelerator {
    /// Apple Silicon w/ Metal + unified memory — the ideal local-inference box.
    AppleSilicon,
    /// Discrete NVIDIA GPU w/ CUDA (VRAM-bound). Dormant until the PC detect
    /// path constructs it (`cuda` cargo feature) — the match arms below keep
    /// the wiring compiling so that port stays honest.
    #[allow(dead_code)]
    Cuda,
    /// Intel Mac / no usable GPU offload — CPU inference.
    Cpu,
}

/// Detect the runtime machine. Uses sysinfo for CPU/RAM (cross-platform) and
/// per-OS heuristics for the accelerator + its usable memory.
pub fn detect() -> HardwareInfo {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();

    let ram_gb = (sys.total_memory() as f64 / 1_073_741_824.0) as f32; // bytes -> GiB
    let cpu_cores = num_cpus::get();
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());

    let (gpu, accel, accel_mem_gb) = detect_accel(ram_gb);

    let summary = match accel {
        Accelerator::AppleSilicon => format!(
            "{cpu} · {ram_gb:.0}GB unified memory · Metal (Apple Silicon)"
        ),
        Accelerator::Cuda => format!(
            "{cpu} · {ram_gb:.0}GB RAM · {gpu} ({accel_mem_gb:.0}GB VRAM, CUDA)"
        ),
        Accelerator::Cpu => format!("{cpu} · {ram_gb:.0}GB RAM · CPU only"),
    };

    HardwareInfo { cpu, cpu_cores, ram_gb, gpu, accel_mem_gb, accel, summary }
}

#[cfg(target_os = "macos")]
fn detect_accel(ram_gb: f32) -> (String, Accelerator, f32) {
    // Apple Silicon = aarch64 on macOS. Unified memory: the GPU can address most
    // of system RAM, but leave headroom for the OS/app (~25%, min 2GB reserve).
    if std::env::consts::ARCH == "aarch64" {
        let usable = (ram_gb * 0.75).max(ram_gb - 2.0).min(ram_gb);
        ("Apple GPU (Metal)".to_string(), Accelerator::AppleSilicon, usable)
    } else {
        // Intel Mac — treat as CPU (discrete AMD GPUs aren't a reliable llama.cpp
        // offload target across the fleet; be conservative + honest).
        ("Integrated / Intel".to_string(), Accelerator::Cpu, (ram_gb - 2.0).max(1.0))
    }
}

#[cfg(not(target_os = "macos"))]
fn detect_accel(ram_gb: f32) -> (String, Accelerator, f32) {
    // Non-mac: we can't reliably read VRAM without extra deps here, so assume CPU
    // inference for the PREDICTION baseline. (A CUDA build still runs on GPU; the
    // predictor just won't over-promise VRAM it can't measure.) Mason builds on a
    // Mac, so this branch is mainly the Windows authoring box + future PC users.
    ("GPU (unmeasured)".to_string(), Accelerator::Cpu, (ram_gb - 2.0).max(1.0))
}

// ---------------------------------------------------------------------------
// REAL FIT CALCULATION — weights + KV cache + overhead vs Metal working set
// ---------------------------------------------------------------------------

/// Metal working-set cap: the GPU's actual limit (~2/3 of RAM on Macs ≤36GB,
/// ~75% above). This is what llama.cpp hits with "Decode Error -3" — distinct
/// from the looser `accel_mem_gb` heuristic used for the header summary.
/// Must match `local_provider::gpu_layers_for` exactly.
pub fn working_set_gb(ram_gb: f32) -> f32 {
    if ram_gb <= 36.0 { ram_gb * (2.0 / 3.0) } else { ram_gb * 0.75 }
}

/// KV cache per token (GB), scales with model size. 7B ~0.25 MB/tok, 27B ~0.45 MB.
/// Conservative + matches llama.cpp's fp16-ish cache. `local_provider` uses a
/// fixed 0.00025; we scale gently so 27B@128k doesn't look like it fits.
pub fn kv_per_token_gb(params_b: f32) -> f64 {
    // 0.18 MB base + 0.01 MB per billion params
    0.00018 + (params_b as f64 * 0.00001)
}

/// KV cache size for a context window.
pub fn kv_gb_for(ctx_tokens: u32, params_b: f32) -> f64 {
    ctx_tokens as f64 * kv_per_token_gb(params_b)
}

/// Max context tokens that fit for this hardware + model size + file.
/// Uses working_set - overhead - weights. Returns at least MIN_CTX.
/// This is conservative (assumes full GPU offload) — the runtime will
/// still apply partial offload if needed, but recommending the fully-fitting
/// window guarantees "just works" without the CPU cliff.
pub fn max_context_tokens(hw: &HardwareInfo, params_b: f32, file_size_gb: f32) -> u32 {
    const OVERHEAD_GB: f64 = 1.5;
    const MIN_CTX: u32 = 2048;
    const MAX_CTX: u32 = 131072;
    let ws = working_set_gb(hw.ram_gb) as f64;
    let avail = ws - file_size_gb as f64 - OVERHEAD_GB;
    if avail <= 0.0 {
        return MIN_CTX;
    }
    let per_tok = kv_per_token_gb(params_b);
    let max = (avail / per_tok) as u32;
    let snapped = snap_context(max);
    snapped.clamp(MIN_CTX, MAX_CTX)
}

fn snap_context(n: u32) -> u32 {
    const STEPS: &[u32] = &[2048, 4096, 8192, 16384, 24576, 32768, 49152, 65536, 98304, 131072];
    let mut best = STEPS[0];
    for &s in STEPS {
        if n >= s { best = s; } else { break; }
    }
    best
}

/// Recommended context: advertised window capped by what fits. This is the
/// "correct" window to auto-set on download, and retroactively for existing
/// models. `advertised` is the catalog's context_window (0 = unknown).
pub fn recommended_context(hw: &HardwareInfo, params_b: f32, file_size_gb: f32, advertised: u32) -> u32 {
    let max_fit = max_context_tokens(hw, params_b, file_size_gb);
    if advertised == 0 {
        return max_fit.min(8192);
    }
    advertised.min(max_fit)
}

/// Performance verdict for a specific model download.
#[derive(Debug, Clone, Serialize)]
pub struct PerfVerdict {
    /// "great" | "usable" | "slow" | "wont_fit"
    pub tier: &'static str,
    /// Emoji badge for the UI.
    pub badge: &'static str,
    /// Estimated tokens/sec range, e.g. "40–60 tok/s". Empty if won't fit.
    pub tokens_per_sec: String,
    /// One-line human explanation.
    pub note: String,
    /// Whether the model can fully offload to the accelerator.
    pub fits: bool,
    /// Recommended context window that actually fits on this machine.
    pub recommended_ctx: u32,
    /// Advertised context window (for comparison).
    pub advertised_ctx: u32,
}

/// Predict performance for a model of `params_billions` at a quant whose file is
/// `file_size_gb`. Now accounts for KV cache at the ADVERTISED context — so a
/// 16GB 27B at 128k correctly shows as tight/won't-fit on 24GB instead of "great".
pub fn predict(hw: &HardwareInfo, params_billions: f32, file_size_gb: f32) -> PerfVerdict {
    predict_with_ctx(hw, params_billions, file_size_gb, 0)
}

/// Same as `predict` but with an explicit advertised context (0 = use catalog default heuristic).
pub fn predict_with_ctx(hw: &HardwareInfo, params_billions: f32, file_size_gb: f32, advertised_ctx: u32) -> PerfVerdict {
    let ctx = if advertised_ctx == 0 { 8192 } else { advertised_ctx };
    let kv = kv_gb_for(ctx, params_billions);
    let overhead = 1.5;
    let required = file_size_gb as f64 + kv + overhead;
    let ws = working_set_gb(hw.ram_gb) as f64;
    let usable = ws;

    // also compute what WOULD fit
    let recommended_ctx = recommended_context(hw, params_billions, file_size_gb, if advertised_ctx==0 { 32768 } else { advertised_ctx });
    let advertised = if advertised_ctx==0 { 0 } else { advertised_ctx };

    if required > usable * 1.02 {
        return PerfVerdict {
            tier: "wont_fit",
            badge: "❌",
            tokens_per_sec: String::new(),
            fits: false,
            note: format!(
                "Needs ~{:.1}GB at {}k context, you have ~{:.0}GB working set. Fits only at ~{}k or try a smaller quant.",
                required, ctx/1024, usable, recommended_ctx/1024
            ),
            recommended_ctx,
            advertised_ctx: advertised,
        };
    }

    let tight = required > usable * 0.85;

    match hw.accel {
        Accelerator::AppleSilicon => {
            let (lo, hi) = speed_range(params_billions, 1.0);
            if tight {
                PerfVerdict { tier: "usable", badge: "👍", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: format!("Fits at ~{}k context (advertised {}k would OOM). Close other apps for best speed.", recommended_ctx/1024, ctx/1024),
                    recommended_ctx, advertised_ctx: advertised }
            } else {
                PerfVerdict { tier: "great", badge: "⚡", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: format!("Runs great at {}k — fully GPU-offloaded on Metal.", recommended_ctx/1024),
                    recommended_ctx, advertised_ctx: advertised }
            }
        }
        Accelerator::Cuda => {
            let (lo, hi) = speed_range(params_billions, 1.1);
            if tight {
                PerfVerdict { tier: "usable", badge: "👍", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: format!("Fits your VRAM at ~{}k (advertised {}k is tight).", recommended_ctx/1024, ctx/1024),
                    recommended_ctx, advertised_ctx: advertised }
            } else {
                PerfVerdict { tier: "great", badge: "⚡", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Runs great — fully offloaded to your GPU (CUDA).".into(),
                    recommended_ctx, advertised_ctx: advertised }
            }
        }
        Accelerator::Cpu => {
            let (lo, hi) = speed_range(params_billions, 0.18);
            if params_billions > 14.0 {
                PerfVerdict { tier: "slow", badge: "🐢", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Will run on CPU but slowly at this size — a 7–8B model will feel much better.".into(),
                    recommended_ctx, advertised_ctx: advertised }
            } else {
                PerfVerdict { tier: "usable", badge: "👍", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Runs on CPU — usable for chat, not instant. No GPU offload detected.".into(),
                    recommended_ctx, advertised_ctx: advertised }
            }
        }
    }
}

/// Rough tok/s range for a model size, scaled by an accelerator factor. Bigger
/// models are slower; the factor captures accelerator class (Metal ~1.0,
/// CUDA ~1.1, CPU ~0.18). Numbers are deliberately conservative estimates.
fn speed_range(params_billions: f32, accel_factor: f32) -> (u32, u32) {
    // Base throughput ~ inversely proportional to params. A 7B on Metal ≈ 45 t/s.
    let base = (320.0 / params_billions.max(1.0)) * accel_factor;
    let lo = (base * 0.75).round().max(1.0) as u32;
    let hi = (base * 1.15).round().max(2.0) as u32;
    (lo, hi)
}
