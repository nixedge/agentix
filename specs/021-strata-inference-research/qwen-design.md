# Independent Qwen execution design

**Implementation status**: CPU reference and state/storage foundations first; full-model execution and GPU kernels remain unimplemented.

## Authorities and provenance

- Original checkpoint: Qwen/Qwen3.8-Flash-Next revision `de4b8e4d43b917e7706784d8bb445c9af86a3540` (`config.json`).
- Independent operator authority: Hugging Face Transformers revision `2fa1fdd5b27ae54bdcee0516848c7963393a5f5c`, `src/transformers/models/qwen4_exp/{modeling,configuration}_qwen4_exp.py`, Apache-2.0. The earlier 5.8.0 package guess is not a verified implementation source; the pinned Git source is authoritative here.
- Direct pinned upstream reads were authorized after ingestion failed. No Strata code is used.

## Execution contract

The checkpoint specifies 48 layers: three recurrent GatedDeltaNet layers followed by one QSA layer, repeated twelve times. The hidden width is 2560, four residual streams, 512 routed experts with ten selected per token, and a shared expert. PLE is applied at one-based layer 2 before the attention residual mixer. The final mixer collapses the four streams; there is no conventional final RMSNorm substitution.

Ordinary norms multiply by `1 + checkpoint_weight`, groupwise for residual/PLE norms. Gated recurrent output norms instead use their direct weight and the configured output activation (sigmoid in this checkpoint). MoE routing is a float32 softmax followed by top-k and selected-weight normalization (reference default true). Shared expert output has an independent sigmoid gate. Tie ordering is deterministic in the Rust reference but may differ from accelerator top-k; compare nontied fixtures and explicitly test tied behavior.

QSA pools complete visible-token blocks of four, normalizes and rotates pooled keys at block-start positions, scores them with positive query/key dot products summed over indexer heads, selects up to 512 blocks and always includes the incomplete tail. Main attention uses 24 query heads, two KV heads and 256 dimensions; query projection interleaves query and sigmoid-gate features per head. Text-only RoPE uses equal position coordinates; vision is excluded.

GatedDeltaNet retains a depthwise convolution state and an FP32 key-by-value recurrent state per value head. Its state recurrence is: decay the state by exp(g), compute predicted value with k, add outer(k, beta * (v - predicted)), and read with normalized q scaled by inverse sqrt(key width). Query/key L2 normalization uses epsilon 1e-6. Multiple value heads share each key head. Invalid inputs must not partially mutate state.

## N-gram and PLE state

Checkpoint n-gram size is three with eight heads per n-gram length, producing sixteen 160-wide rows. Head vocabularies are successive primes at or above 20,000,000, with cumulative row offsets. SplitMix64-derived odd multipliers use default seed 1234 and PLE-layer ordinal; products are bounded to signed 64-bit before XOR and per-head remainder. EOS is 248044; unavailable history is EOS-filled, and tokens after EOS may not see history from the preceding segment. The EOS token itself retains the preceding segment history.

Keep a bounded chronological history of at most two tokens. A snapshot must restore history together with position, recurrent states, convolution states, KV/indexer caches and PLE convolution state before speculative work is enabled. N-gram history restore alone is not a complete speculative decoder.

The reference row store uses an explicitly declared row width, row count and payload offset with overflow and file-length checks. It reads bounded rows on demand and bounds retained payload cache bytes. Metadata/cache overhead and caller-owned output buffers are separate budget items. Native quantized storage must supply an independently verified row decoder; do not assume the GGUF PLE payload is plain FP32 or accept an unsupported encoding silently.

PLE concatenates selected embedding rows, projects values and per-stream keys, gates with normalized residual queries using signed square-root dot products, then adds dilated depthwise convolution output. Implement lookup correctness independently of cache optimization.

## Initial implementation boundary and dependencies

Create `agentix-qwen` for model-specific reference math, token/state handling and bounded storage; the generic inference crate retains lifecycle/selection ownership. Initial CPU foundations use Rust standard library only and no native bindings. This establishes independent deterministic oracles without committing to an unverified CUDA or quantized loader. No inference backend is registered until complete model execution and agentix contracts work.

The remaining native/quantized dependency selection gate applies to the full-model decoder and CUDA path, not to these dependency-free reference primitives. Likewise the full-model fixture and tensor-layout gate remains open; analytical operator vectors are not claimed as full-model acceptance. No new public service, changes to generic crate dependencies, or constitution exception is needed for this foundation.

## Validation fixed before optimization

Integer n-gram row IDs must match exactly; chunking and EOS boundaries must not change them. Row-store tests use known byte offsets and truncated/overflowing files. Memory arithmetic must reject overflow and never borrow between RAM and VRAM limits. Reference FP32 operator vectors use absolute tolerance 1e-6 on small analytical cases; optimized full-model tolerances must be established against independent full-model reference outputs before GPU acceptance. Invalid numeric/shape input leaves mutable state unchanged. Neither these vectors nor successful compilation establishes inference capability or throughput.
