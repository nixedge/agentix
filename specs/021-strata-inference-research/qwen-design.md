# Independent Qwen execution design

**Implementation status**: a reusable CPU GGUF decoder traverses all 48 blocks and preserves state across tokens; a two-token full-artifact smoke pass produces vocabulary logits. Independent full-model parity, tokenizer-backed prompt generation, worker integration, and GPU kernels remain unimplemented.

## Authorities and provenance

- Original checkpoint: Qwen/Qwen3.8-Flash-Next revision `de4b8e4d43b917e7706784d8bb445c9af86a3540` (`config.json`, SHA256 `889658f2508e8c61d409b02e70e0d78d8d4452ec65aaafbe129805d213d2e74b`, verified against the immutable upstream bytes).
- Tokenizer artifact: `tokenizer.json` from the same immutable checkpoint revision; pinned Nix hash `sha256-CZf0EMV6H05TsJ5L6PShctkO3ZVkNo+whHAwk3IpufM=`. It contains 248,077 addressable IDs (maximum ID 248,076), while the model embedding vocabulary is padded to 248,320. These are distinct dimensions and must not be conflated.
- Chat template: `chat_template.jinja` from the same immutable checkpoint revision; pinned Nix hash `sha256-w8+eNKv0+eNsLXIWWqnBMtPipyW2wlhqqjqK+deoEEE=`. The exact template is retained as the semantic authority. MiniJinja 2.24 does not provide the template's `startswith`/`endswith` string methods by default, so the tokenizer registers a narrow unknown-method callback for those two string operations; the pinned template fixtures pass. Full-model tokenization and generation conformance remain unverified.

`tokenizer::generate_greedy` provides the ordinary greedy control loop over a
caller-supplied logits function. It uses deterministic lower-token-ID tie
breaking, stops on the pinned turn-end or EOS IDs, and enforces the requested
new-token limit. The closure receives prompt plus prior generated tokens; a stop
token is retained in the generated IDs. The IQ3_S artifact sampler now supplies
actual logits for one token after traversing all 48 blocks. It does not yet
retain state across generated tokens or connect prompt preparation to a
multi-token model loop, so full chat decoding remains incomplete.
- Independent operator authority: Hugging Face Transformers revision `2fa1fdd5b27ae54bdcee0516848c7963393a5f5c`, `src/transformers/models/qwen4_exp/{modeling,configuration}_qwen4_exp.py`, Apache-2.0. Captured modeling/configuration files have SHA256 `f94401c10c1bbafd814cc174ba6f40dde1f40b35715ce1ae827ccf8ee3cd81fa` / `ac0950ab34516d26fe4f6b28cffe0649d763449effe44e42b5913463806f7e0b`; both independently match raw files fetched from that exact GitHub commit. The earlier 5.8.0 package guess is not a verified implementation source; the pinned Git source is authoritative here.
- Direct pinned upstream reads were authorized after ingestion failed. No Strata code is used.

## Execution contract

The checkpoint specifies 48 layers: three recurrent GatedDeltaNet layers followed by one QSA layer, repeated twelve times. The hidden width is 2560, four residual streams, 512 routed experts with ten selected per token, and a shared expert. PLE is applied at one-based layer 2 before the attention residual mixer. The final mixer collapses the four streams; there is no conventional final RMSNorm substitution.

Ordinary norms multiply by `1 + checkpoint_weight`, groupwise for residual/PLE norms. Gated recurrent output norms instead use their direct weight and the configured output activation (sigmoid in this checkpoint). MoE routing is a float32 softmax followed by top-k and selected-weight normalization controlled by `norm_topk_prob`; the pinned Transformers config defaults it to true when omitted, and the Agentix parser preserves that default while rejecting a non-boolean value. Shared expert output has an independent sigmoid gate. Tie ordering is deterministic in the Rust reference but may differ from accelerator top-k; compare nontied fixtures and explicitly test tied behavior.

The CPU reference implements one-token MoE assembly using router weights `[experts, hidden]`, fused expert gate/up weights `[2*intermediate, hidden]`, expert down weights `[hidden, intermediate]`, and separate shared-expert gate/up/down projections. Each routed expert applies `SiLU(gate) * up`, down-projects, scales by its selected router probability, and accumulates. The shared expert uses the same SwiGLU equation, then its output is scaled by `sigmoid(shared_expert_gate(input))` and added. `reference::moe_block_from_rows` computes router scores and the shared path from bounded row readers, and opens only selected experts. `agentix-qwen-sys::MoeGgufLayer` now validates router/shared/expert-stack geometry and drives that reference from the configured normalization setting. The full IQ3_S artifact runs it on block 17 for a constant-vector input and returns finite output; it has no independent expected-logit oracle. This remains a CPU operator smoke path, not full-layer graph execution, batched expert dispatch, or expert cache placement.

QSA pools complete visible-token blocks of four, normalizes and rotates pooled keys at block-start positions, scores them with positive query/key dot products summed over indexer heads, selects up to 512 blocks and always includes the incomplete tail. Main attention uses 24 query heads, two KV heads and 256 dimensions; query projection interleaves query and sigmoid-gate features per head. Text-only RoPE uses equal position coordinates; vision is excluded.

`reference::qsa_indexer_step` combines the single-token fused Q/K projection, query-head RMSNorm using the source's offset weights, text RoPE, raw-key cache append, and block selection for a decode query. `reference::qsa_select_tokens` implements the block stage: groups visible indices (including non-contiguous masks), averages each complete block, normalizes and rotates at the first visible index, sums positive head scores, selects the highest-scoring blocks, and appends the remaining visible tail. It resolves equal scores by lower block index for deterministic CPU vectors. `reference::TextRotaryEmbedding` generates duplicated-half partial text RoPE angles and applies the pinned `rotate_half` layout while preserving the unrotated tail. Full-sequence indexing and batched prefill remain unimplemented.

`reference::qsa_attention_step_from_rows` runs the main QSA attention operator directly from bounded query/gate, key, value and output matrix readers, while sharing its cache and attention core with the materialized reference. `reference::qsa_indexer_step_from_rows` separately reads the indexer's query and key matrices, normalizes/rotates queries, appends its raw key, and applies the existing sparse block selector. `reference::qsa_decode_step_from_rows` composes both and returns selected indices plus updated raw-index and main KV caches. Its synthetic fixture matches `qsa_decode_step`. The full IQ3_S artifact drives the combined block-3 decoder with one actual current hidden vector and a 4,096-token synthetic past cache; the pinned 2,048-token budget selects 2,049 tokens including the incomplete current tail. Output and caches are finite. This does not provide model-derived past states or validate a token against an independent full-model oracle.

`reference::qsa_attention_step` implements one main-attention decode token: per-head interleaved query/gate projection, Q/K RMSNorm with Qwen's zero-centered norm weights, partial text RoPE, KV cache append, caller visibility through grouped-query softmax, sigmoid gating, and output projection. `reference::qsa_decode_step` connects raw index-key caching and block selection to this attention step by producing the sparse visibility mask. `reference::qsa_prefill` handles one fresh sequence from row-major triangular visibility, while `reference::qsa_prefill_with_cache` continues a supplied prefix cache using a rectangular visibility matrix and rejects future-token visibility. A split-prefill test matches a continuous prefill's outputs, selections, and final caches. Both are sequential CPU references, not batched production kernels; batched prefill and native kernels remain unimplemented.

`QwenTextConfig::parse` checks the pinned top-level architecture and fail-closes on unsupported text dimensions, 48-layer GatedDeltaNet/QSA ordering, routing, PLE, n-gram, RoPE, or norm settings. It is a guard for this checkpoint family, not a general Transformers configuration reader. The pinned JSON fixture is hash-checked. GGUF tensor-name/shape conversion and full weight allocation remain separate loader work.

GatedDeltaNet retains a depthwise convolution state and an FP32 key-by-value recurrent state per value head. Its state recurrence is: decay the state by exp(g), compute predicted value with k, add outer(k, beta * (v - predicted)), and read with normalized q scaled by inverse sqrt(key width). Query/key L2 normalization uses epsilon 1e-6. Multiple value heads share each key head. Invalid inputs must not partially mutate state.

## N-gram and PLE state

Checkpoint n-gram size is three with eight heads per n-gram length, producing sixteen 160-wide rows. Head vocabularies are successive primes at or above 20,000,000, with cumulative row offsets; the resulting logical table has 320,001,446 rows. Padding that row count to a multiple of 128 gives 320,001,536 physical rows. Each address is a per-head offset plus a remainder strictly below that head's prime vocabulary, so valid n-gram addresses stay within the logical extent and cannot reach the 90 physical padding rows. A test walks every model vocabulary token while evolving history and checks all 16 row IDs remain below the logical bound. A dense BF16/FP16 table at that padded size alone would occupy 102,400,491,520 bytes (about 95.4 GiB), before any other model weights. SplitMix64-derived odd multipliers use default seed 1234 and PLE-layer ordinal; products are bounded to signed 64-bit before XOR and per-head remainder. EOS is 248044; unavailable history is EOS-filled, and tokens after EOS may not see history from the preceding segment. The EOS token itself retains the preceding segment history.

The pinned Transformers implementation constructs one dense n-gram embedding parameter, while the pinned safetensors index names 128 separate `ple_embedding.ngram_embedding.shard_0.weight` through `shard_127.weight` entries. Header-only reads of all 128 tensors show identical BF16 `[2500012,160]` shapes, so the shards form contiguous equal-sized row partitions; the numeric shard order is the source order used by the pinned llama.cpp converter. Their row total is exactly 320,001,536, matching the padded GGUF extent. `ple::open_qwen4exp_ple_rows` derives the padded source-shard dimensions from the validated Qwen config, resolves tensor filenames through the checkpoint index, and composes numeric-order BF16 row readers without materializing the table. Synthetic tests cover the public opener, cross-shard row dispatch, malformed index names, unsafe paths and geometry. The published IQ4_NL preset's second GGUF shard header identifies `per_layer_token_embd.weight` as Q4_0 with shape `[160,320001536]`; its payload size is exactly five 32-value blocks per row at 18 bytes per block (90 bytes/row). Agentix range-read one hash-verified 90-byte PLE row and one 320-byte BF16 source row; their decoded values agree within 0.003. No full source weight shard was downloaded. This establishes source-reader mechanics and one-row plus one-vector value parity, not broad source value parity or the complete model payload. IQ2_XS/IQ3_S cannot encode each 160-value row independently because their block size is 256; no cross-row packing is assumed.

The same second-shard inventory lists 1,224 tensors across IQ4_NL, Q8_0, Q4_0,
F32, BF16, and F16; the exact counts and tensor shapes are in
`evidence/ggml-qwen-iq4nl-shard-2-tensors.tsv`. The complete 10.9 MB first shard
was downloaded and hash-verified; its header has split index 0/2 and zero
tensors. Together with the second shard's metadata-only header, the Agentix set
inspector confirms sequential split indices, 1,224 unique tensor names, and the
declared two-file set. This is directory-level completeness only: the second
shard payload and broad source-to-GGUF value parity remain unverified. Header
name, shard, dtype and shape mapping is now checked for all text tensors.

Keep a bounded chronological history of at most two tokens. A snapshot must restore history together with position, recurrent states, convolution states, KV/indexer caches and PLE convolution state before speculative work is enabled. N-gram history restore alone is not a complete speculative decoder.

The reference row store uses an explicitly declared row width, row count and payload offset with overflow and file-length checks. It reads bounded rows on demand and bounds retained payload cache bytes. `rows::SafeTensorFile` parses the bounded header with the pinned `safetensors` metadata type, validates the complete file extent, and opens rank-two BF16 tensors through the PLE adapter or rank-two F32/F16/BF16 tensors through a shared finite F32 row reader. Its bounded `float_vector` reader handles rank-one F32/F16/BF16 vectors, including the common norm and gate vectors. `ple::ShardedEmbeddingRows` composes the BF16 readers into the source checkpoint's logical concatenated PLE table and routes each global row ID to one shard-local reader without materializing the full table. Synthetic SafeTensors fixtures cover vector and row decoding, shape/type/range rejection, cache behavior and sharded-reader composition. A source BF16 PLE row matches its published Q4_0 GGUF row within `0.003`; one complete non-PLE BF16 norm vector matches its published F32 GGUF counterpart exactly across all 128 values after the direct source transform. Broader source payload values remain unverified. Metadata/cache overhead and caller-owned output buffers are separate budget items. `agentix-qwen-sys::QwenGgufTensorStore` maps a schema-validated split set to lazy per-tensor readers; the full IQ3_S artifact validates all 1,224 tensors, with finite samples from all 13 candidate storage types. `GgufMatrixRows` implements the pure `LinearWeightRows` interface and opens ordinary matrices or one contiguous rank-three expert group. `reference::linear_from_rows` applies a dense CPU projection with one row buffer; `reference::moe_block_from_rows` composes row-backed routing, selected gate/up/down experts and the shared gated SwiGLU path. A synthetic Q8_0 linear test and synthetic MoE equivalence test cover these APIs; the full IQ3_S artifact runs a one-token block-17 MoE smoke check with finite output. This does not establish independent real-weight parity, full layer-graph integration, placement, full inference, or kernels. Do not accept an unsupported encoding silently.

PLE concatenates selected embedding rows, projects values and per-stream keys, gates with normalized residual queries using signed square-root dot products, then adds dilated depthwise convolution output. Implement lookup correctness independently of cache optimization.

## Initial implementation boundary and dependencies

Create `agentix-qwen` for model-specific reference math, token/state handling, artifact identity and bounded storage; the generic inference crate retains lifecycle/selection ownership. Initial CPU math and storage do not link native bindings; the CPU crate uses pinned tokenizer/template and SHA-256 dependencies. This establishes independent deterministic oracles without committing to an unverified CUDA or quantized loader. No inference backend is registered until complete model execution and agentix contracts work.

The remaining native/quantized dependency selection gate applies to the full-model decoder and CUDA path, not to these CPU reference primitives. Likewise the full-model fixture and tensor-layout gate remains open; analytical operator vectors are not claimed as full-model acceptance. No new public service, changes to generic crate dependencies, or constitution exception is needed for this foundation.

## Validation fixed before optimization

Integer n-gram row IDs must match exactly; chunking and EOS boundaries must not change them. Row-store tests use known byte offsets and truncated/overflowing files. Memory arithmetic must reject overflow and never borrow between RAM and VRAM limits. Reference FP32 operator vectors use absolute tolerance 1e-6 on small analytical cases; optimized full-model tolerances must be established against independent full-model reference outputs before GPU acceptance. Invalid numeric/shape input leaves mutable state unchanged. Neither these vectors nor successful compilation establishes inference capability or throughput.

## Independent reference protocol and current numerical gates

Tokenizer fixtures are pinned to the checkpoint revision and must compare both
the rendered string and token IDs. The current fixture matrix covers a simple
user turn with exact token IDs and round-trip decode, an assistant reasoning and
tool call followed by a tool response with special-token round-trip, and
rejection of empty/image/high-reasoning-effort requests. It separately asserts
248,077 tokenizer IDs, 248,320 model vocabulary rows, and stop IDs 248,046 and
248,044.

For analytical CPU FP32 operator vectors, require `abs(actual - expected) <=
1e-6`; integer IDs, shapes, mask membership, state-commit decisions, and token
boundaries require exact equality. For sampled source-to-GGUF float transforms,
use `abs(actual - expected) <= 1e-6 * max(1, abs(expected))` to accommodate
last-bit differences in exponentiation; the captured norm and `dt_bias` vectors
match exactly, and the 48-value `A_log` vector passes this bound after `-exp`
and head reordering. These sampled vectors exercise real payload bytes but do not stand
in for full tensor or full-model parity.

The full-model reference run must use the immutable Qwen checkpoint and the
pinned Transformers implementation listed above, the same source BF16 tensor
payloads and tokenizer/template, and deterministic greedy decoding. Persist
the exact config/tokenizer/template and weight-shard hashes, prompt token IDs,
per-position logits, generated IDs, and intermediate outputs at each layer's
attention/linear-attention, MoE, and residual-stream boundaries. First compare
Agentix's source-BF16 CPU path with the reference; compare the quantized GGUF
path in a separate report so source/operator error is not conflated with
quantization error. Require exact integer/state-layout agreement and report
maximum absolute/relative tensor error plus top-1 logit agreement at every
position. Freeze numeric acceptance thresholds only after capturing these
independent outputs. No complete source-BF16 loader, reference output archive,
architecture fixture, or target GPU result exists yet, so this protocol is
specified but not executed and full-model numeric acceptance remains open.

## Further execution details checked against the pinned reference

For each token, repeat its base embedding into all four residual streams. A layer
first adds PLE (when configured), then performs attention and MoE in sequence,
each using its own gated residual mixer. For `C` streams of width `H`, normalize
the `C*H` input groupwise. Project down to the configured low rank, divide by `C`,
apply SiLU, project up to `C*H`, and apply sigmoid. Multiply these gates by the
normalized streams and average across streams to obtain the block input. A separate
projection of the normalized streams, divided by `C`, produces `C` injection
weights through `2*sigmoid`. Add the block output times each injection weight to
the corresponding original, unnormalized stream. The final mixer uses the same
input mixing equations without an injection projection. The CPU reference now
implements dense and row-backed linear projections, these down/SiLU/up gates,
stream averaging, optional injection weights, and residual reinjection. These
are single-token FP32 primitives; the reusable GGUF decoder now composes all
48 blocks and preserves state across token steps, as described below.

The recurrent block projects Q/K/V jointly, applies causal depthwise convolution
and SiLU, and splits Q/K by key-head count and V by value-head count. Contiguous
value-head groups share one key head. Its other projections produce output gate
`z`, update gate `beta = sigmoid(b)`, and
`g = -exp(A_log) * softplus(a + dt_bias)`. `GatedDeltaNetState` implements a
single-token full recurrent layer step from dense projections through causal
depthwise convolution/SiLU, transformed gates, repeated-key multi-head delta
state, per-value-head gated RMS norm, and output projection. It stages both
convolution and recurrent state so failures do not partially commit. Inputs
must already have padding masked to zero. `process` accepts token-major chunks,
matches repeated calls to `step`, and commits the whole chunk atomically. This
is sequential FP32 prefill, not a parallel chunk kernel; GPU kernels and
production-scale performance are not implemented.
Its output norm is per value head, followed by sigmoid(z) and the output projection.

PLE's projected key and residual query are separately normalized by stream. Their
dot product is divided by `sqrt(H)`, transformed to
`sign(x)*sqrt(max(abs(x), 1e-6))`, then passed through sigmoid to gate the shared
projected value. Preserve the exact zero case (`sign(0)=0`). Normalize the gated
values groupwise for the dilated depthwise convolution input. The PLE output is
the unnormalized gated value plus SiLU of that convolution. Dilation equals the
n-gram size and retained convolution history is `(kernel_size-1)*dilation`.
Padding masks zero both convolution inputs and direct gated values; masked PLE
input IDs become EOS before hashing. `reference::ple_gate` implements and tests
the post-projection gate and convolution-input normalization, including zero
dot behavior. `DepthwiseConvState` implements the causal dilated depthwise
cross-correlation and SiLU, retains `(kernel_size-1)*dilation` time-major input
history, commits state atomically, and is tested for chunk-equivalent output.
`ple::lookup_embedding` joins `Ngram` addresses to a caller-supplied decoded-row
reader, and `ple::step` runs row lookup, PLE projection/gating, and convolution
as one transaction across n-gram history and convolution state. The CPU
reference PLE graph is therefore covered when given verified FP32 rows.
`ple::prefill` applies that path sequentially to a single token-major sequence,
staging all n-gram and convolution state until the whole chunk succeeds; tests
compare it with repeated `ple::step` calls and verify rollback after a late row
decode failure. Batched or prefix-cache PLE prefill remains unimplemented. The
native quantized PLE row adapter implements `EmbeddingRowReader`.
`PleGgufLayer` now binds the candidate's PLE table, block projections, norms,
and convolution weights to the row-backed transactional PLE step. The
full-artifact sampler starts at token ID 1, runs block 0, then applies block 1
PLE and the block-1 GatedDeltaNet/MoE residual paths; the 16 looked-up rows
and all resulting activations are finite. The full-artifact sampler now carries
this path through all 48 blocks, applies the final hyper-connection combine,
and produces finite vocabulary logits on two consecutive stateful decode
steps. This is a CPU smoke run without an independent numeric oracle;
full-model numeric acceptance remains open.
The official converter establishes numeric concatenation order and common row
width. The ggml-org IQ4_NL preset stores PLE rows as Q4_0, while
the separately pinned IQ3_S candidate stores them as IQ4_NL; both headers report
`[160,320001536]` and 90 bytes per row. `agentix-qwen-sys` implements checked
Q4_0 and IQ4_NL row decoding plus metadata-validated on-demand `QuantizedRows`.
Synthetic layout tests pass, and one 90-byte IQ4_NL row from the IQ3_S candidate
decodes to 160 finite values. Original safetensors shard geometry/order are now
verified from header-only reads; source value parity and use in an executable
worker remain open. The
native sys crate implements the pure `agentix-qwen` PLE reader trait; dependency
direction keeps the pure reference crate independent of native code. See `sources.md` and the two
sample-evidence JSON files for artifact-specific provenance and limits.
`ple::step` maps padded positions to EOS before hashing, zeros gated values and
convolution inputs, and advances convolution history with zero.

QSA block grouping follows the visible token indices of the causal/padding mask,
not unconditional absolute groups. Pool raw indexer keys in float32, normalize the
pooled vector and apply RoPE at each group's first visible token position. Sum
positive query/key dot products across indexer query heads and divide by
`sqrt(indexer_head_dim)`. Select complete blocks up to the budget and append every
visible token in the incomplete tail. Intersect this selection with the original
causal mask for main attention. Main projected Q and output gates are interleaved
per head; Q/K norms precede RoPE and cached K/V retain their positions. These are
These behaviors are implemented by the sequential CPU reference paths described
above; batched prefill, native kernels, and production-scale execution remain
future work.

Source anchors in the pinned Transformers file:
`Qwen4ExpTextGatedResidual`, `Qwen4ExpTextGatedDeltaNet`, `Qwen4ExpTextPLELayer`,
`Qwen4ExpTextQSAIndexer`, `Qwen4ExpTextAttention`, `Qwen4ExpTextDecoderLayer`, and
`Qwen4ExpTextModel.forward`. Agentix maps all 1,294 text tensor names in the
pinned safetensors index to GGUF names or PLE metadata, including the 128-to-1
PLE shard aggregation, fused indexer Q/K split, expert gate/up split,
GatedDeltaNet value-head reorder, PLE norm offset, and PLE convolution squeeze.
The resulting 1,224 unique target names equal the pinned config schema and
published split-header inventory. `source::transform_qwen4exp_f32_tensor`
now dispatches the mapped dense float transforms, using source-shape checks
against the pinned configuration for indexer and expert splits, PLE norm
offset/convolution squeeze, and GatedDeltaNet value-head reorders. The pinned
converter specifies that QKV changes only its V rows, the convolution changes
only its V-channel tail, A-log/dt-bias and alpha/beta reorder one value per head,
and output projection reorders columns. This dispatcher does not load every
source dtype or consume PLE shards. Header-only reads now cover all 131 pinned
SafeTensors shards; the joined text header crosswalk records dtype, source shape,
shard, and data offsets for all 1,294 mapped names. The index's complete 1,658
name/shard assignments exactly match the shard headers, and all header-declared
byte extents are consistent with dtype and shape. This does not verify weight
values. The pinned-header integration test checks every text source shape against
the corresponding GGUF schema target after its declared transform, including
indexer/expert splits, convolution squeeze, and all 128 concatenated PLE shards.
The checked source-array reader preserves the
three PLE hash arrays as integers, and the isolated GGUF rewriter emits their
pinned UINT64 metadata keys while preserving tensor payload bytes. Full source
array parity, model-wide tensor conversion, and broader tensor value verification
remain open. Quantization support remains open under T006/T007.
Tokenizer/template artifact identities are pinned, and the three template
fixtures pass with the MiniJinja compatibility callback.
`QwenTokenizer::prepare_prompt` packages rendering, encoding, and context
admission: the measured token count comes from the actual rendered prompt, and
the full requested output bound must fit without truncation. This is a
request-preparation primitive; no generation loop currently consumes it.

## Checkpoint tensor namespace and dense geometry

The pinned checkpoint's `model.safetensors.index.json` was inspected from the
original Qwen revision. Its captured local bytes have SHA256
`99e815241ef03325536b0aaa4441deea45174c17fae31e10f0bb456410c590de`; the index
maps names to shard files but does not contain tensor shapes or quantization
metadata. Header-only reads of all 131 shards now provide shape/dtype metadata
for every indexed entry without reading tensor payloads; the checked-in text
crosswalk joins all 1,294 text entries to Agentix mappings. The names below are
the original Transformers state-dict namespace.
Dimensions are derived from the pinned config and module constructors; dense
weights use PyTorch linear order `[out, in]` unless stated otherwise.

- Root text tensors are `model.language_model.embed_tokens.weight`
  `[248320,2560]`, `lm_head.weight` `[248320,2560]` (untied), and
  `model.language_model.hyper_connection_mixer.{hc_norm.weight,
  input_mix_weight_down.weight,input_mix_weight_up.weight}` for the four-stream
  final mixer (no injection projection).
- Each of 48 decoder layers has `attn_hyper_connection.*` and
  `mlp_hyper_connection.*`: grouped norm length 10240; input mix down `[320,10240]`,
  up `[10240,320]`, and block injection `[4,10240]`.
- The 36 recurrent layers have `linear_attn.{in_proj_qkv,in_proj_z,in_proj_b,
  in_proj_a,out_proj,conv1d,norm,A_log,dt_bias}`. With key width 16×128=2048 and
  value width 48×128=6144, QKV projection is `[10240,2560]`, Z is `[6144,2560]`,
  B/A are `[48,2560]`, output is `[2560,6144]`, depthwise convolution is
  `[10240,1,4]`, and A-log/dt-bias each have 48 values. The per-head output norm
  has 128 values.
- The 12 QSA layers have `self_attn.{q_proj,k_proj,v_proj,o_proj,q_norm,k_norm}`
  and `self_attn.indexer.{index_qk_proj,q_layernorm,k_layernorm}`. Attention
  projections are Q `[12288,2560]`, K/V `[512,2560]`, O `[2560,6144]`; Q/K norms
  have 256 values. The indexer projection is `[640,2560]`, with four query heads,
  one key head, and 128 values per head; its norms have 128 values.
- Every layer has `mlp.gate.weight` `[512,2560]`, stacked expert
  `mlp.experts.gate_up_proj` `[512,1280,2560]` and `down_proj` `[512,2560,640]`,
  plus `mlp.shared_expert.{gate_proj,up_proj,down_proj}` with intermediate width
  640 and `mlp.shared_expert_gate.weight` `[1,2560]`.
- Only layer index 1 (the second layer) has `ple.*`: key projection `[10240,2560]`,
  value projection `[2560,2560]`, grouped key/query/conv norms of length 10240,
  and depthwise convolution `[10240,1,4]`. Its `ple_embedding` contains the
  n-gram embedding split into 128 named safetensors shards plus multiplier,
  per-head vocabulary-size and row-offset buffers. The crosswalk in
  `evidence/qwen3_8_flash_next_source_to_gguf.tsv` records all 128 shards as
  inputs to the single PLE GGUF tensor. Safetensors header-only range reads
  across every shard confirm all 128 tensors are BF16 `[2500012,160]`, each
  contributes 2,500,012 rows at width 160, and their ordered total is exactly
  320,001,536 rows. Header evidence is in
  `evidence/qwen3_8_flash_next_ple_shard_headers.tsv`; no tensor payload bytes
  were downloaded. This resolves PLE row geometry and shard boundaries, but
  not payload values, their numerical order's effect on reference outputs, or
  implementation of quantized PLE weight loading.
- The snapshot also contains `model.visual.*` tensors and a separate `mtp.*`
  namespace (`fc_embedding`, `fc_hidden`, one full-attention/MoE layer and its
  mixer/norms). Vision is outside the first text-only target. MTP is optional and
  must be identified, independently prepared, and validated separately before
  speculative decoding.

This namespace inventory narrows loader validation. The native boundary exposes
bounded row decoding through pinned GGML callbacks for every storage type in
the selected IQ3_S candidate; IQ1_M remains unsupported. Both IQ3_S shard
hashes and the full 1,224-tensor schema have been validated, with one finite
real row sampled from each of the candidate's 13 storage types. Candidate
integrity is therefore established, but full artifact loading and inference
tests remain open.

## Independent GGUF and compute dependency decision

Use `llama-cpp-sys-2` 0.1.154, already pinned in the workspace lockfile, as the
isolated native boundary for its generated `ggml_*` and `gguf_*` bindings. The
crate bundles the upstream llama.cpp/ggml source; the workspace lock checksum
pins the complete published source artifact. `agentix-qwen-sys` will expose a
small checked Rust interface over GGUF metadata and GGML contexts, buffers,
graphs, and backends. The Qwen graph, tensor-name mapping, state machine,
admission, tokenizer and scheduling remain agentix-owned. Do not call llama.cpp's
model loader, model architecture dispatch, server, chat templates or conversion
scripts. Keep the sys crate in a separate Qwen worker process from `agentix-llama`
so the two native stacks never enter one address space.

The inspected 0.1.154 GGML source has CPU quant/dequant traits and CUDA matrix
vector/matrix-vector dispatch entries for IQ2_XS and IQ3_S. Agentix's bounded
GGUF row adapter uses native CPU `to_float` callbacks for the supported types
in its inspected artifacts. The complete two-shard IQ3_S candidate header
inventory has 1,224 schema-matching tensors and uses 13 storage types; both
publisher shard hashes match, and Agentix decodes one real row per storage
type through bounded dense or quantized row adapters. Synthetic quantized rows
are also checked against the same pinned callbacks. This establishes artifact
integrity and row-reader coverage, not end-to-end model loading/decode,
independent quantization parity or target-GPU validation. GGUF parsing still rejects types
outside the explicit mapping. IQ1_M is not selected for the first full-model
artifact.

Pinned source detail: `QK_K` is 256. `block_iq2_xs` stores an fp16 scale, 32
16-bit packed grid/sign words, and eight scale bytes (74 bytes per 256 values,
2.3125 bpw). `block_iq3_s` stores an fp16 scale, 64 quant bytes, eight high-bit
bytes, 32 sign bytes, and four subscale bytes (110 bytes per 256 values,
3.4375 bpw). CPU dequantizers are `dequantize_row_iq2_xs` and
`dequantize_row_iq3_s`; CUDA has IQ2_XS and IQ3_S matrix-matrix and
matrix-vector dispatch in `mmq.cu` and `mmvq.cu`, plus conversion kernels in
`dequantize.cuh`. The native package's `ggml_type_size` and `ggml_blck_size`
traits are now used by the metadata inspector to reject extents that do not
align to the type block and payload byte counts inconsistent with shape/type.
Synthetic tests execute one-block CPU dequantization through pinned GGML
callbacks and verify the type/block geometry used by the reader. One real IQ3_S
expert row has also been read through the bounded GGUF adapter from the pinned
candidate, but has not been compared with an independent numeric oracle; no CUDA
kernel has been compared yet. GGML reports IQ2_XS quantization requires an
importance matrix, while IQ3_S does not; converter provenance/calibration and
the resulting checkpoint tensor map are still unresolved.

CUDA is optional and enabled only by the native Qwen worker's `cuda` feature.
Nix packaging reuses the workspace CUDA 12 toolkit setup and explicitly targets
compute capability 8.6 (`sm_86`, RTX 3090); CPU-only library tests and builds do
not link CUDA. If the registered Nix configuration supplies
`nixpkgs.config.cudaCapabilities`, derive architectures through the shared Nix
helper; target-host builds must include 8.6. A capability mismatch must fail the
CUDA package rather than silently compiling for the current build machine. GPU
results and full format support remain unverified until executed on the target.

The native dependency is independently sourced from the crates.io
`llama-cpp-sys-2` package and its bundled upstream llama.cpp/ggml tree; it is not
obtained from Strata and no Strata source or runtime artifact is used as an
implementation input. Cargo's lock checksum is the source artifact pin. This
does not make Strata-generated tensor packs a required input: the exact original
Qwen-to-GGUF conversion and tensor mapping remain a separate gate before loading
any candidate artifact.

### Current CPU row-backed integration

`agentix-qwen-sys::GatedDeltaNetGgufLayer` is the first reusable actual-GGUF
layer loader: it opens and validates the configured GatedDeltaNet projection,
convolution, and vector tensor geometry, then owns their lazy row readers,
small convolution/gate vectors, and recurrent state. It exposes single-token
and chunk calls. The caller still supplies the layer-normalized/residual-stream
input and performs outer layer/residual operations.

`agentix-qwen-sys::QsaGgufLayer` similarly owns the six block projection
readers, four norm-offset vectors, text RoPE geometry, and per-sequence raw
index-key and attention KV caches. It accepts sequential positions and commits
new caches only after the row-backed decode returns successfully. The
single-sequence API treats every prior token as visible; padding and batching
remain caller/runtime work. The artifact sampler separately runs two positions
with actual block-3 weights and synthetic constant inputs, alongside a long
synthetic-past sparse-budget exercise. Its full-model path also feeds block-3
QSA output from the preceding actual blocks and continues through the remaining
layers. The separate layer checks cover cached-position and sparse-budget cases.

`ResidualMixerGguf` binds the block attention or FFN hyper-connection tensors
to `gated_residual_from_rows`; `ResidualMixerKind::Final` covers the
combine-free output mixer. The full IQ3_S sampler starts from actual
`token_embd.weight` row 1, repeats it across four streams, applies actual PLE
at block 1, and runs every configured attention and MoE layer through block 47.
The reusable `QwenGgufDecoder` owns lazy readers and persistent per-layer
GatedDeltaNet, QSA, PLE n-gram, convolution, and position state. Two successive
`step` calls produce finite 248,320-value logits; the first greedy token is
79104 (logit 11.96474266), and the next is 245228. This demonstrates a
stateful two-token CPU graph execution for the candidate, but no independent
expected logits, tokenizer-backed prompt decode, or numeric acceptance result
is available.

`GatedDeltaNetState::step_from_rows` streams each large linear projection one
decoded GGUF row at a time. Small vectors (`ssm_conv1d.weight`, `ssm_a`,
`ssm_dt.bias`, and `ssm_norm.weight`) are loaded directly. It stages both
convolution and recurrent state and commits them only after output projection
succeeds, matching the dense reference's single-token transaction boundary.
`process_from_rows` also provides atomic token-major prefill by staging state
across the chunk and reusing the same row-backed matrices. Synthetic
equivalence covers one- and two-token output/state against the dense path, and
a late row-read failure confirms whole-chunk rollback. The actual IQ3_S sampler
exercises block 0 single-token decode and two-token prefill using actual weights
with synthetic hidden vectors/initial states. This validates plumbing and
finite execution only. It does not establish independent quantized-value
parity, full graph tensor transforms, CUDA execution, or model output
correctness.
