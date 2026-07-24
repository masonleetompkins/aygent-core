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
    /// Discrete NVIDIA GPU w/ CUDA (VRAM-bound).
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
}

/// Predict performance for a model of `params_billions` at a quant whose file is
/// `file_size_gb`. We estimate required memory as the weight file + a KV-cache /
/// runtime overhead that scales with model size, then compare to usable accel
/// memory and pick a speed tier.
pub fn predict(hw: &HardwareInfo, params_billions: f32, file_size_gb: f32) -> PerfVerdict {
    // Runtime overhead beyond the weights: KV cache + compute buffers. Rough but
    // honest — scales with model size. ~0.6GB per 7B of params + 0.7GB base.
    let overhead = 0.7 + (params_billions / 7.0) * 0.6;
    let required = file_size_gb + overhead;
    let usable = hw.accel_mem_gb;

    // Won't fit at all (can't even hold weights in usable memory + a little slack)
    if required > usable * 1.05 {
        return PerfVerdict {
            tier: "wont_fit",
            badge: "❌",
            tokens_per_sec: String::new(),
            fits: false,
            note: format!(
                "Needs ~{required:.1}GB, you have ~{usable:.0}GB usable. Try a smaller model or quant."
            ),
        };
    }

    // Tight fit (fits but eats most of memory → will be sluggish / may swap)
    let tight = required > usable * 0.85;

    match hw.accel {
        Accelerator::AppleSilicon => {
            // Metal unified memory: strong. Speed scales inversely with size.
            let (lo, hi) = speed_range(params_billions, 1.0);
            if tight {
                PerfVerdict { tier: "usable", badge: "👍", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Fits, but uses most of your memory — close other apps for best speed.".into() }
            } else {
                PerfVerdict { tier: "great", badge: "⚡", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Runs great — fully GPU-offloaded on Metal.".into() }
            }
        }
        Accelerator::Cuda => {
            let (lo, hi) = speed_range(params_billions, 1.1);
            if tight {
                PerfVerdict { tier: "usable", badge: "👍", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Fits your VRAM but tightly — expect some slowdown.".into() }
            } else {
                PerfVerdict { tier: "great", badge: "⚡", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Runs great — fully offloaded to your GPU (CUDA).".into() }
            }
        }
        Accelerator::Cpu => {
            // CPU inference: works, but slow — and big models crawl.
            let (lo, hi) = speed_range(params_billions, 0.18);
            if params_billions > 14.0 {
                PerfVerdict { tier: "slow", badge: "🐢", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Will run on CPU but slowly at this size — a 7–8B model will feel much better.".into() }
            } else {
                PerfVerdict { tier: "usable", badge: "👍", tokens_per_sec: format!("{lo}–{hi} tok/s"), fits: true,
                    note: "Runs on CPU — usable for chat, not instant. No GPU offload detected.".into() }
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
