# Local Models — Memory Architecture Plan (24GB Mac ⇒ run up to ~18GB GGUFs)

**Goal:** a 24GB unified-memory Mac runs models up to ~18GB with no crashes, quantized
tiers (2-bit/4-bit) are visible in the picker, heavy models spill to CPU gracefully,
and the context window stays usable + compacts safely.

## Findings (current stage code)

1. **Download gate regression.** `hardware::predict_with_ctx` marks a model `wont_fit`
   when `weights + KV + 1.5GB > working_set_gb(ram)` — on 24GB that's a 16GB Metal cap,
   so a 16GB file can never pass. The UI (`Settings.tsx:494`) disables Download on
   `!perf.fits`. But the runtime (`local_provider::gpu_layers_for`) already does
   PARTIAL offload and would run that model. **The predictor gates on "fully offloads";
   the engine only needs "fits in total RAM."**

2. **Quant tiers hide small quants.** `catalog::extract_quants` surfaces only
   Q4_K_M-class and Q6_K/Q8_0-class files. Q2_K and all i-quants (IQ2/IQ3/IQ4) are
   invisible — exactly the sizes that make 27B+ models viable on 24GB.

3. **KV cache is fp16.** No flash attention, no KV quantization. At 32k on a ~27B
   model the KV alone is ~14GB — this is what makes big-context verdicts fail.

4. **Context truncation is unsafe.** `run_turn` does `tokens.drain(0..drop)` on the
   RAW token stream when the prompt exceeds the window — this deletes the SYSTEM
   PROMPT first and can cut mid-message, degrading the model exactly when the chat
   is longest.

## Plan

### A. Fix the fit verdict — three tiers, never block what runs (hardware.rs, Settings.tsx)

Replace the binary `fits` with an offload-aware verdict:

| tier         | condition                                              | badge | Download |
|--------------|--------------------------------------------------------|-------|----------|
| `great`      | weights + KV(rec ctx) + overhead ≤ working_set          | ⚡    | enabled  |
| `partial`    | weights + KV + overhead ≤ ram − OS_RESERVE (≈3GB)       | 🟡    | enabled  |
| `cpu`        | fits RAM but <1 GPU layer fits                          | 🐢    | enabled  |
| `wont_fit`   | weights + KV(min 2k ctx) + overhead > ram − OS_RESERVE  | ❌    | disabled |

- `partial` note: "Runs with N of M layers on GPU, rest on CPU — expect ~X tok/s."
  Estimate speed as `frac_gpu × metal_rate + (1−frac_gpu) × cpu_rate` (frac from the
  same math as `gpu_layers_for`, so prediction == runtime).
- On 24GB: truly-won't-fit threshold ≈ 21GB of weights ⇒ 16–18GB models downloadable
  again, honestly labeled.
- UI: keep the button enabled for `partial`/`cpu`; show the tier note inline.

### B. Surface small quants — add an "Efficient" tier + fit-aware pick (catalog.rs)

- Third tier `Efficient / smallest` preferring: `IQ4_XS, Q3_K_M, IQ3_M, IQ3_XXS, Q2_K,
  IQ2_M`. Extend `estimate_size_gb` bpw table:
  `IQ4_XS 4.25, IQ3_M 3.66, IQ3_XXS 3.06, Q2_K 3.35 (keep), IQ2_M 2.7, IQ2_XS 2.31`.
- Hardware-aware recommendation: after computing per-quant verdicts (lib.rs already
  attaches `perf` per quant), tag the LARGEST quant that scores `great` on THIS
  machine as "Best for your Mac". A 27B repo on 24GB then recommends IQ4_XS/Q3_K_M
  instead of showing two dead buttons.
- Optional power-user toggle "show all quants" listing every single-file GGUF in the
  repo with its verdict badge.

### C. Runtime memory efficiency (local_provider.rs)

1. **Flash attention + quantized KV.** Build chat contexts with flash attention on
   and KV cache at q8_0 (`LlamaContextParams` — reconcile exact llama-cpp-2 0.1 API:
   `with_flash_attention(true)`, kv type params if exposed; if the crate doesn't
   expose type_k/type_v yet, bump/patch the crate — llama.cpp supports it).
   Effect: KV memory ≈ halves ⇒ `kv_per_token_gb` halves ⇒ bigger contexts fit and
   more layers offload. Update `hardware::kv_per_token_gb` in lockstep (single source
   of truth: move the constant into one fn both sides call — they already must match).
2. **Small physical ubatch.** Keep logical `n_batch = 2048` but set `n_ubatch = 512`
   so compute buffers stay small on partial-offload runs.
3. **Partial offload already correct** — keep `gpu_layers_for` as the single budget
   authority; A's predictor must call the same function so UI and runtime never
   disagree.
4. **mmap stays default**: CPU-side layers page from disk cache, not wired memory —
   this is what makes an 18GB model on 24GB safe (OS evicts cold pages under pressure).
5. **One-model residency** (already done in `session_sender`) — keep.

### D. Safe context compaction (local_provider.rs + lib.rs)

1. **Fix the drain bug.** Never truncate raw tokens from the front. Compact at
   MESSAGE boundaries before rendering:
   - Always keep: system block + the last K turns (K chosen to fit
     `ctx − MAX_NEW_TOKENS − margin`).
   - Evict oldest middle messages whole.
   This also preserves the KV prefix (system prompt unchanged ⇒ prefix cache hit).
2. **Summarize-on-evict (phase 2).** When >80% full, run a one-shot local summary of
   the evicted turns (same session, cheap) and inject it as a
   `"[Earlier conversation summary: …]"` system-adjacent message. The context meter
   already reports the real window, so the UI shows compaction honestly.
3. **Keep `tokens.drain` only as a last-resort guard** after message-level compaction,
   but anchor it to preserve the system prefix (drop from after the system block).

### E. Optional advanced knob (Settings)

macOS allows raising the Metal wired limit: `sudo sysctl iogpu.wired_limit_mb=N`.
Do NOT ship as default; offer as a documented "Advanced" note for power users. If set,
read the sysctl at detect() time and use it as the working set instead of the 2/3 rule.

## Order of work

1. A (verdict tiers + UI unblock) — smallest change, restores 16–18GB downloads.
2. B (quant tiers) — catalog-only, no engine risk.
3. C1/C2 (flash-attn + KV q8_0 + ubatch) — biggest capacity win; needs crate check.
4. D (compaction) — correctness fix (drain bug) first, summarizer second.

## Invariants

- Predictor and runtime share ONE budget function (`gpu_layers_for` math).
- Never disable Download for a model that the engine would run.
- Never destroy the system prompt during truncation.
- Estimates stay labeled as estimates.
