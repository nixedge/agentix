# CPU foundation validation

Benchmark protocol: [`benchmarks.md`](benchmarks.md). It defines the run matrix
and records still needed; benchmark execution still needs a persistent
generation worker, target host, and finalized coding-task evaluator.

Run from the persistent `021-strata-inference-research` worktree:

```sh
nix develop --command cargo test -p agentix-qwen
nix develop --command cargo test -p agentix-qwen-sys
nix develop --command cargo fmt -p agentix-qwen -p agentix-qwen-sys --check
nix develop --command cargo clippy -p agentix-qwen -p agentix-qwen-sys --all-targets -- -D warnings
nix build .#agentix-qwen
nix build .#agentix-qwen-sys
```

These commands validate CPU reference operators, artifact identity, tokenizer
rendering, GGUF split completeness/uniqueness and type/shape byte counts, n-gram
state, encoded-row storage and memory arithmetic. The CPU Nix package runs its tests and installs
the Rust library plus provenance. The sys package runs GGUF architecture/split,
Q4_0/IQ4_NL row-decoder, and on-demand row-reader tests against the pinned
GGML/GGUF dependency. The sys package includes an artifact sampler that runs a
single token through all 48 blocks and the output projection; it does not
provide a reusable multi-token generation worker or GPU inference path.
The fixed integer and analytical vectors in `agentix-qwen/tests` are part of the
source; they do not download model weights or constitute a full-model fixture.

Regenerate and verify the exact benchmark prompt fixtures with the Nix-pinned
tokenizer and template:

```sh
nix develop --command cargo run -p agentix-qwen --example build_benchmark_prompt_fixtures
nix develop --command cargo test -p agentix-qwen --test benchmark_prompts
```

The checked-in artifact records rendered prompts, exact token IDs and hashes
for 4,096, 32,768 and 65,536-token inputs. `QwenGgufDecoder::generate_prepared_greedy`
connects an admitted `PreparedPrompt` to the persistent full-model decoder and
existing greedy sampler. The artifact check exercises it with token ID 1 as a
minimal prepared prompt for two full-model steps; no benchmark result or
full-artifact run from a rendered chat prompt exists yet.

The target host is only accessible to the user. GPU build, inventory, model
verification and benchmark commands will be added once their executable paths
exist and have been checked locally. No target-host action is needed to validate
these CPU foundations. The compact synthetic fixture below checks decoder
control flow; T010/T030's production-model reference and final dependency/package
strategy remain open. This document covers the current foundation package.

## Qwen worker package

`nix build .#agentix-qwen-worker` builds the isolated worker. The wrapper
uses the pinned tokenizer/template and text-config fixtures and expects the two
IQ3_S shards in `AGENTIX_QWEN_MODEL_DIR` as `iq3s-00001.gguf` and
`iq3s-00002.gguf` by default. Startup hashes every shard against the pinned
publisher SHA256 values before schema/layer validation and socket creation;
set `AGENTIX_QWEN_GGUF_SHARDS` and
`AGENTIX_QWEN_EXPECTED_SHARD_SHA256` together when names or artifacts change.
The worker serves `/health`, `/v1/models` and greedy text
`/v1/chat/completions` over `AGENTIX_QWEN_SOCKET`. The daemon routes only the
exact `AGENTIX_QWEN_MODEL` name to it. Text SSE is bounded; tools, reasoning
channels, non-greedy sampling and non-chat endpoints are explicitly unsupported.

The NixOS module exposes `services.agentix.qwen`; the deployment still needs
the user to place the verified GGUF shards in its configured model directory.
The CPU reference sampler is the available local end-to-end model evidence; a
full model request through the worker has not been run because generation is
slow on this host and the real chat prefill is not batched yet.

### Compact full-decoder fixture

Run the deterministic four-block synthetic GGUF through the complete stateful
CPU decoder with:

```sh
nix develop --command cargo test -p agentix-qwen-sys \
  compact_qwen4exp_gguf_runs_the_full_stateful_decoder
```

The test generates the fixture using the pinned `llama-cpp-sys-2` GGUF writer,
validates its complete schema, and asserts the exact greedy output for all-zero
weights. The generated artifact SHA-256 is
`29ddcc30858e34f94259607a6695da7d9a216b70651ff0d89bf5e9b8221259d0`. This
checks full-decoder wiring and state traversal; it is not original-checkpoint
numeric parity or a worker deployment test.

### NixOS smoke deployment

Import the module from the Agentix flake (replace `inputs.agentix` if the flake
input has a different name) and enable both the gateway and worker. Both package
options are required by the module:

```nix
{ inputs, pkgs, ... }:
{
  imports = [ inputs.agentix.nixosModules.agentix ];

  services.agentix = {
    daemon.enable = true;
    daemon.package = inputs.agentix.packages.${pkgs.system}.agentix-daemon;

    qwen = {
      enable = true;
      package = inputs.agentix.packages.${pkgs.system}.agentix-qwen-worker;
      modelDir = "/var/lib/agentix/qwen";
      # Defaults: the verified IQ3_S shard names and publisher hashes.
      # context = 32768;
      # Optional CUDA probe: requires a CUDA-enabled package and a working driver.
      # This routes only the final vocabulary projection to CUDA; all Qwen block
      # operators remain CPU-side and streamed weights are uploaded each token.
      # extraEnv.AGENTIX_QWEN_CUDA_OUTPUT_DEVICE = "0";
    };
  };
}
```

`AGENTIX_QWEN_CUDA_OUTPUT_DEVICE` is an opt-in correctness probe, not a
performance setting. The worker validates that the selected CUDA device exists
before binding its socket. CUDA output projection has not been executed on the
RTX 3090 target; leave the option unset until testing on that host.

The model shards are hosted under the `IQ3_S/` subdirectory with longer
publisher filenames. Fetch the pinned revision directly into the worker's
expected names (the module creates the destination for the `agentix` account):

```sh
sudo install -d -o agentix -g agentix -m 0750 /var/lib/agentix/qwen
cd /var/lib/agentix/qwen
sudo -u agentix curl -fL --retry 5 \
  'https://huggingface.co/ISTA-DASLab/Qwen3.8-Flash-Next-GSQ-RCO-GGUF/resolve/ed59f92082b1e93c0e96d60a8b11aab089b52f09/IQ3_S/Qwen3.8-Flash-Next-GSQ-RCO-IQ3_S-00001-of-00002.gguf' \
  -o iq3s-00001.gguf
sudo -u agentix curl -fL --retry 5 \
  'https://huggingface.co/ISTA-DASLab/Qwen3.8-Flash-Next-GSQ-RCO-GGUF/resolve/ed59f92082b1e93c0e96d60a8b11aab089b52f09/IQ3_S/Qwen3.8-Flash-Next-GSQ-RCO-IQ3_S-00002-of-00002.gguf' \
  -o iq3s-00002.gguf
```

Both shards total about 83.6 GB. The worker package supplies the pinned
config, tokenizer and chat template. Verify the shard digests before activation:

```sh
sha256sum /var/lib/agentix/qwen/iq3s-00001.gguf /var/lib/agentix/qwen/iq3s-00002.gguf
# Expected, in shard order:
# 4c1eb2ceb4915e1192f4f386021897bde56a97f40a0bb78bb86465e0f7d2aca3
# 316b46f3a2dbd68c900f43136ab9449f9dcc3725dfd8c794847c204bc161e113
```

After `nixos-rebuild switch`, inspect worker readiness and send a small greedy
chat request through the daemon:

```sh
systemctl status agentix-qwen agentix-daemon
curl -sS http://127.0.0.1:11434/v1/chat/completions \\
  -H 'content-type: application/json' \\
  -d '{"model":"qwen3.8-flash-next","messages":[{"role":"user","content":"Reply with OK."}],"temperature":0,"max_tokens":4}'
```

This is a CPU smoke deployment by default. The optional
`AGENTIX_QWEN_CUDA_OUTPUT_DEVICE` setting routes only the final vocabulary
projection through CUDA; all attention, recurrent, PLE, residual-mixer, and MoE
operators remain on CPU. GGUF output weights are streamed and uploaded for each
token, so this path is a correctness probe rather than an expected speedup. A
successful package build does not establish GPU execution or target-device
correctness. Real chat prefill is also not batched, so the first request may be
very slow.

For a direct output-projection comparison on the GPU host, run the pinned
Q6_K `output.weight` through the CPU row reference and CUDA matvec:

```sh
nix develop --command cargo run -p agentix-qwen-sys --features cuda \
  --example compare_cuda_gguf_matvec -- \
  agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json \
  /var/lib/agentix/qwen/iq3s-00001.gguf \
  /var/lib/agentix/qwen/iq3s-00002.gguf 0
```

The example reports max absolute/relative logit error and top-1 agreement. It
does not choose a numerical acceptance threshold; record its output before
freezing the operator tolerance.

## Current dependency and Nix boundary

`agentix-qwen` is the pure Rust reference/identity/tokenizer crate. Its locked
runtime dependencies are MiniJinja, tokenizers, safetensors, serde_json, and
sha2. The separate `agentix-qwen-sys` package is the only Qwen crate that depends on the
locked `llama-cpp-sys-2` 0.1.154 GGML/GGUF bindings. `cargo tree` confirms the
pure crate does not reach those native bindings; neither tree contains Strata.
The CUDA feature is opt-in on the sys crate. Existing Nix package logic derives
CUDA architecture flags from `pkgs.config.cudaCapabilities`; for the target
RTX 3090, the expected capability is 8.6. CPU package builds do not establish
that the CUDA configuration compiles or runs.

The tokenizer and chat template are fetched from the pinned Qwen revision with
fixed Nix hashes and are exercised by the checked-in template tests. The full
IQ3_S candidate used for research is not a small Nix test fixture. Its complete
shards are cached outside the worktree at
`/home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/`;
both publisher SHA256 values and its full tensor schema were verified. This
is the runtime artifact consumed by `agentix-qwen-worker`; a packaged build
does not include or distribute the large model shards.

The split-aware `QwenGgufTensorStore` validates a complete shard set against
the configured 1,224-tensor schema, then opens a named dense or quantized
tensor as a lazy bounded row reader. Rank-one/rank-two matrices can be opened
with `open_matrix_rows` and consumed by the pure Rust
`reference::linear_from_rows`, which reads one weight row at a time. This
provides a CPU reference path for a single linear operation. Rank-three expert
stacks can expose one `[in,out]` group through `open_group_matrix_rows`, and
`reference::moe_block_from_rows` composes streamed routing, selected experts,
and shared expert matrices. `qsa_attention_step_from_rows` also runs the main
QSA attention operator with streamed projections and KV cache updates;
`qsa_decode_step_from_rows` composes that attention with streamed indexer rows
and returns updated caches. The IQ3_S artifact check runs one block-17 MoE
token, block-0 GatedDeltaNet single-token/two-token prefill, block-3 QSA
two-position cached decode, and a separate block-3 QSA decode with synthetic
past caches. Its full-model sample composes block 3 from propagated actual
activations. `GatedDeltaNetGgufLayer`
and `QsaGgufLayer` validate and own their
actual GGUF readers and per-layer state; `MoeGgufLayer` validates shared and
expert tensor geometry and opens only routed expert groups. GatedDeltaNet
matrix projections are streamed; its convolution and scalar parameters are
small loaded vectors. `ResidualMixerGguf` streams block hyper-connection
matrices, and `PleGgufLayer` binds actual PLE rows and projections to the
transactional n-gram and convolution state. The artifact sampler now carries
actual token ID 1 through all 48 configured blocks, including block 1 PLE,
then the final stream combine and full vocabulary projection. It emits finite
logits and greedy token IDs for two consecutive positions. This is a stateful
CPU reference smoke run, not an independent parity result or a tokenizer-backed
prompt generation worker. Set `AGENTIX_QWEN_CHAT_SMOKE=1` when running the
sampler inside `nix develop` to render the actual `Reply with OK.` user prompt
through the pinned chat template and tokenizer before full-model decode; the
sampler reports the prompt source and admitted token count. This opt-in path has
not yet been run against the full artifact.
Reproduce the
actual-artifact validation with:

```sh
nix develop --command env AGENTIX_QWEN_CHAT_SMOKE=1 cargo run -p agentix-qwen-sys --example validate_gguf_sample_rows -- \
  agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00001.gguf \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00002.gguf
```

This checks header schema, one finite row from each observed storage type, and
the listed row-backed operator paths, followed by a two-token stateful full
decoder run. With the opt-in chat flag above, the decoder input is rendered by
the pinned tokenizer/template. Neither mode verifies every tensor's numeric
values or independent full-model parity. Synthetic GGUF unit fixtures continue
to cover malformed metadata and reader errors.

### Original source-to-IQ3_S row comparison

Raw BF16 row 1 from the pinned original checkpoint is captured for both
`model.language_model.embed_tokens.weight` → `token_embd.weight` and
`lm_head.weight` → `output.weight`. Their hashes and source offsets are in
`evidence/qwen3_8_flash_next_token_embd_row1_source.json` and
`evidence/qwen3_8_flash_next_lm_head_row1_source.json`. Both rows were compared
with the pinned IQ3_S shards using:

```sh
nix develop --command cargo run -p agentix-qwen-sys --example compare_source_bf16_gguf_row -- \
  specs/021-strata-inference-research/evidence/qwen3_8_flash_next_token_embd_row1_source.bf16 \
  agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json \
  token_embd.weight 1 \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00001.gguf \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00002.gguf

nix develop --command cargo run -p agentix-qwen-sys --example compare_source_bf16_gguf_row -- \
  specs/021-strata-inference-research/evidence/qwen3_8_flash_next_lm_head_row1_source.bf16 \
  agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json \
  output.weight 1 \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00001.gguf \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00002.gguf
```

The row metrics are recorded in
`evidence/qwen3_8_flash_next_source_to_iq3s_row_comparisons.json`. The embedding
row comparison reports max absolute error `7.0190e-4`, RMSE `2.2747e-4`,
relative RMSE `9.1546e-2`, and cosine similarity `0.995814`. The output row
comparison reports max absolute error `1.7242e-3`, RMSE `3.9618e-4`, relative
RMSE `2.0096e-2`, and cosine similarity `0.999798`. These two sampled rows
provide source-to-candidate evidence only; they do not establish whole-tensor
or full-model numerical parity.

The pinned tokenizer artifact contains 248,077 token IDs, while the model's
input/output embedding vocabulary is padded to 248,320 rows. Use the tokenizer
to encode and decode text; use the configured model vocabulary for tensor
geometry and logits. Do not treat these counts as interchangeable. The pinned
tokenizer test checks both values and exercises special-token IDs near the
upper end of the tokenizer vocabulary.

### Normal dependency tree audit

These exact commands were run in the Nix development shell:

```sh
nix develop --command cargo tree -p agentix-qwen --edges normal
nix develop --command cargo tree -p agentix-qwen-sys --edges normal
```

The first tree's direct dependencies are `minijinja 2.24.0`, `safetensors
0.8.0`, `serde_json 1.0.151`, `sha2 0.10.9`, and `tokenizers 0.22.2`. It has
no `llama-cpp-sys-2` dependency: `agentix-qwen` can parse pinned text
configuration, load indexed SafeTensors BF16 PLE rows, and execute CPU
reference operations without the native crate, but it does not parse GGUF
files. The second tree directly depends on `agentix-qwen` and
`llama-cpp-sys-2 0.1.154`; `agentix-qwen-sys` owns GGUF parsing, split-file
validation, and quantized row decoding through the GGML/GGUF bindings. Neither
normal dependency tree contains Strata. These results establish the current
crate boundary; they do not establish a full model loader or runtime.

The CPU package can be reproduced with the commands above. The configured sys
package compiles with its CUDA feature, but its current Nix check phase cannot
load the test executable in this no-driver sandbox because `libcuda.so.1` is
absent. The separate CPU sys test suite passes. This verifies CUDA compilation,
not CUDA test execution or device behavior. The Nix source set retains both
`agentix-qwen-sys` and its pure `agentix-qwen` trait dependency so the package
build sees the same interface as the workspace. The package boundary review is
recorded in `decision.md`; target-driver execution and final T010 packaging
acceptance remain open.
