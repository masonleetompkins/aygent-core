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
//     TOTAL memory pool (unified RAM minus an OS reserve). The Metal working
//     set only caps the GPU-RESIDENT portion — the runtime's partial offload
//     (local_provider::gpu_layers_for) keeps the remainder on CPU, so a model
//     bigger than the working set still RUNS, just slower.
//   - "How fast?"     = a tier (Great/Partial/Slow) + a tok/s RANGE derived
//     from accelerator class and HOW MUCH of the model offloads.
//
// INVARIANT (Mason, local-models-memory): the predictor must never call
// "won't fit" on a model the runtime would actually run. Fit here == fits the
// RAM pool; the working set only decides the SPEED tier.

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
// REAL FIT CALCULATION — weights + KV cache + overhead vs the memory pools.
// ---------------------------------------------------------------------------

/// Fixed compute-buffer + scratch overhead (GB). Shared by the predictor and
/// the runtime offload budget (`local_provider::gpu_layers_for`) so the two
/// can never disagree.
pub const OVERHEAD_GB: f64 = 1.5;

/// RAM the OS + AYGENT itself need to stay responsive (GB). Everything above
/// this is the model's pool on a unified-memory machine.
const OS_RESERVE_GB: f64 = 3.0;

/// Minimum context we'll ever recommend/run.
const MIN_CTX: u32 = 2048;

/// Metal working-set cap: the GPU's actual limit (~2/3 of RAM on Macs ≤36GB,
/// ~75% above). This is what llama.cpp hits with "Decode Error -3" — distinct
/// from the looser `accel_mem_gb` heuristic used for the header summary.
/// Must match `local_provider::gpu_layers_for` exactly. NOTE: this caps the
/// GPU-RESIDENT slice only — it is a SPEED boundary (full vs partial offload),
/// never a fit boundary.
pub fn working_set_gb(ram_gb: f32) -> f32 {
    if ram_gb <= 36.0 { ram_gb * (2.0 / 3.0) } else { ram_gb * 0.75 }
}

/// TOTAL memory pool a local model may consume (weights + KV + overhead):
/// unified RAM minus the OS reserve. This is the real "will it run at all"
/// ceiling — partial offload + mmap'd CPU-side weights make everything up to
/// this pool runnable (an 18GB model on a 24GB Mac lands here).
pub fn total_budget_gb(ram_gb: f32) -> f64 {
    ((ram_gb as f64) - OS_RESERVE_GB).max(1.0)
}

/// KV cache per token (GB), scales with model size. Chat contexts run a
/// q8_0-quantized KV cache with flash attention (see local_provider) — about
/// half the memory of the old fp16 cache; the 0.55 factor keeps a little
/// margin for the rare f16 fallback. 7B ≈ 0.14 MB/tok, 27B ≈ 0.25 MB/tok.
const KV_Q8_FACTOR: f64 = 0.55;
pub fn kv_per_token_gb(params_b: f32) -> f64 {
    // 0.18 MB base + 0.01 MB per billion params (fp16), scaled for q8_0.
    (0.00018 + (params_b as f64 * 0.00001)) * KV_Q8_FACTOR
}

/// KV cache size for a context window.
pub fn kv_gb_for(ctx_tokens: u32, params_b: f32) -> f64 {
    ctx_tokens as f64 * kv_per_token_gb(params_b)
}

/// Max context tokens that fit for this hardware + model size + file.
/// PREFERS full GPU offload: if the weights leave GPU budget for a KV cache,
/// size the window inside the working set (fast path, no CPU cliff). If the
/// weights alone exceed the working set (partial-offload territory anyway),
/// size the window from the TOTAL RAM pool instead — the model is already
/// split, so the KV rides in unified memory like everything else.
pub fn max_context_tokens(hw: &HardwareInfo, params_b: f32, file_size_gb: f32) -> u32 {
    const MAX_CTX: u32 = 131072;
    let ws = working_set_gb(hw.ram_gb) as f64;
    let total = total_budget_gb(hw.ram_gb);
    let avail_gpu = ws - file_size_gb as f64 - OVERHEAD_GB;
    let avail = if avail_gpu > 0.0 {
        avail_gpu
    } else {
        total - file_size_gb as f64 - OVERHEAD_GB
    };
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
    /// "great" | "usable" | "partial" | "slow" | "wont_fit"
    pub tier: &'static str,
    /// Emoji badge for the UI.
    pub badge: &'static str,
    /// Estimated tokens/sec range, e.g. "40–60 tok/s". Empty if won't fit.
    pub tokens_per_sec: String,
    /// One-line human explanation.
    pub note: String,
    /// Whether the model RUNS on this machine at all (full offload, partial
    /// offload, or CPU). false ONLY when it exceeds the total RAM pool.
    pub fits: bool,
    /// Recommended context window that actually fits on this machine.
    pub recommended_ctx: u32,
    /// Advertised context window (for comparison).
    pub advertised_ctx: u32,
}

/// Predict performance for a model of `params_billions` at a quant whose file
/// is `file_size_gb`, with an explicit advertised context (0 = default heuristic).
///
/// Tiers mirror what the RUNTIME will actually do with this file:
///   great    — weights + KV(recommended ctx) fit the Metal working set: full
///              GPU offload, fast.
///   usable   — fits fully but tight (>85% of the working set).
///   partial  — bigger than the working set but inside the RAM pool: the
///              runtime offloads what fits and runs the rest on CPU. Slower,
///              still runs. Download stays ENABLED.
///   slow     — <10% of layers fit the GPU (or CPU-only box): effectively CPU
///              inference. Runs, patience required.
///   wont_fit — exceeds the RAM pool even at the minimum 2k context. The only
///              tier that disables Download.
pub fn predict_with_ctx(hw: &HardwareInfo, params_billions: f32, file_size_gb: f32, advertised_ctx: u32) -> PerfVerdict {
    let ctx = if advertised_ctx == 0 { 8192 } else { advertised_ctx };
    let ws = working_set_gb(hw.ram_gb) as f64;
    let total = total_budget_gb(hw.ram_gb);
    let file = file_size_gb as f64;

    let recommended_ctx = recommended_context(
        hw, params_billions, file_size_gb,
        if advertised_ctx == 0 { 32768 } else { advertised_ctx },
    );
    let advertised = if advertised_ctx == 0 { 0 } else { advertised_ctx };

    // What we'd ACTUALLY run: memory needed at the recommended (memory-capped)
    // context, and the absolute floor at the minimum context.
    let required_rec = file + kv_gb_for(recommended_ctx, params_billions) + OVERHEAD_GB;
    let required_min = file + kv_gb_for(MIN_CTX, params_billions) + OVERHEAD_GB;

    // HARD gate: only refuse what the engine truly cannot run.
    if required_min > total * 1.02 {
        return PerfVerdict {
            tier: "wont_fit",
            badge: "❌",
            tokens_per_sec: String::new(),
            fits: false,
            note: format!(
                "Needs ~{required_min:.1}GB even at a 2k context — this machine has a ~{total:.0}GB pool. Try a smaller quant (Efficient tier) or a smaller model."
            ),
            recommended_ctx,
            advertised_ctx: advertised,
        };
    }

    match hw.accel {
        Accelerator::AppleSilicon | Accelerator::Cuda => {
            let accel_factor: f32 = if hw.accel == Accelerator::Cuda { 1.1 } else { 1.0 };
            if required_rec <= ws {
                // FULL OFFLOAD — the fast path.
                let (lo, hi) = speed_range(params_billions, accel_factor);
                let tight = required_rec > ws * 0.85;
                if tight {
                    PerfVerdict { tier: "usable", badge: "👍", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                        note: format!("Fits fully at ~{}k context (advertised {}k would spill). Close other apps for best speed.", recommended_ctx/1024, ctx/1024),
                        recommended_ctx, advertised_ctx: advertised }
                } else {
                    PerfVerdict { tier: "great", badge: "⚡", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                        note: format!("Runs great at {}k — fully GPU-offloaded.", recommended_ctx/1024),
                        recommended_ctx, advertised_ctx: advertised }
                }
            } else {
                // PARTIAL OFFLOAD — same split math as gpu_layers_for: the GPU
                // holds frac of (weights + KV), the CPU the rest.
                let frac = ((ws - OVERHEAD_GB) / (file + kv_gb_for(recommended_ctx, params_billions)))
                    .clamp(0.0, 1.0);
                if frac >= 0.10 {
                    // Blended throughput: time = frac/r_gpu + (1-frac)/r_cpu
                    // (harmonic — the CPU share dominates, honestly).
                    let r_cpu = 0.18f64;
                    let eff = 1.0 / (frac / accel_factor as f64 + (1.0 - frac) / r_cpu);
                    let (lo, hi) = speed_range(params_billions, eff as f32);
                    PerfVerdict { tier: "partial", badge: "🟡", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                        note: format!(
                            "Bigger than the GPU's working set — runs with ~{:.0}% of layers on GPU, the rest on CPU, at ~{}k context. Works, but a smaller quant would be much faster.",
                            frac * 100.0, recommended_ctx/1024
                        ),
                        recommended_ctx, advertised_ctx: advertised }
                } else {
                    let (lo, hi) = speed_range(params_billions, 0.18);
                    PerfVerdict { tier: "slow", badge: "🐢", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                        note: format!("Barely any layers fit the GPU — effectively CPU inference at ~{}k context. Runs, but slowly; pick a smaller quant for real use.", recommended_ctx/1024),
                        recommended_ctx, advertised_ctx: advertised }
                }
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
/// CUDA ~1.1, CPU ~0.18) or a partial-offload blend. Numbers are deliberately
/// conservative estimates.
fn speed_range(params_billions: f32, accel_factor: f32) -> (u32, u32) {
    // Base throughput ~ inversely proportional to params. A 7B on Metal ≈ 45 t/s.
    let base = (320.0 / params_billions.max(1.0)) * accel_factor;
    let lo = (base * 0.75).round().max(1.0) as u32;
    let hi = (base * 1.15).round().max(2.0) as u32;
    (lo, hi)
}
