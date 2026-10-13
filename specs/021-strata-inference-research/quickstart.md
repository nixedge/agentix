# CPU foundation validation

Benchmark protocol: [`benchmarks.md`](benchmarks.md). It defines the run matrix
and records still needed; benchmark execution still needs a persistent
generation worker, target host, and finalized coding-task evaluator.

Run from the persistent `021-strata-inference-research` worktree:

```sh
nix develop path:. --command cargo test -p agentix-qwen
nix develop path:. --command cargo test -p agentix-qwen-sys
nix develop path:. --command cargo fmt -p agentix-qwen -p agentix-qwen-sys --check
nix develop path:. --command cargo clippy -p agentix-qwen -p agentix-qwen-sys --all-targets -- -D warnings
nix build path:.#agentix-qwen
nix build path:.#agentix-qwen-sys
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
nix develop path:. --command cargo run -p agentix-qwen --example build_benchmark_prompt_fixtures
nix develop path:. --command cargo test -p agentix-qwen --test benchmark_prompts
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
these CPU foundations. T010/T030's complete model fixture and native dependency
strategy remain open; this document covers only T033's foundation package.

## Qwen worker package

`nix build path:.#agentix-qwen-worker` builds the isolated worker. The wrapper
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
    };
  };
}
```

Place `iq3s-00001.gguf` and `iq3s-00002.gguf` in `/var/lib/agentix/qwen/`;
the module creates that directory for the `agentix` service account. The worker
package supplies the pinned config, tokenizer and chat template. Verify the
shards before activation:

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

This is a CPU smoke deployment. The `cuda` Nix feature currently confirms a
CUDA-enabled native package build; the decoder does not dispatch inference
operators to CUDA. A successful package build therefore does not establish GPU
inference, target-device correctness, or acceptable latency. Real chat prefill
is also not batched, so the first request may be very slow.

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
prompt generation worker.
Reproduce the
actual-artifact validation with:

```sh
nix develop path:. --command cargo run -p agentix-qwen-sys --example validate_gguf_sample_rows -- \
  agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00001.gguf \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00002.gguf
```

This checks header schema, one finite row from each observed storage type, and
the listed row-backed operator paths; it does not verify each tensor's numeric
values or run full-model inference. Synthetic GGUF unit fixtures continue to
cover malformed metadata and reader errors.

The pinned tokenizer artifact contains 248,077 token IDs, while the model's
input/output embedding vocabulary is padded to 248,320 rows. Use the tokenizer
to encode and decode text; use the configured model vocabulary for tensor
geometry and logits. Do not treat these counts as interchangeable. The pinned
tokenizer test checks both values and exercises special-token IDs near the
upper end of the tokenizer vocabulary.

### Normal dependency tree audit

These exact commands were run in the Nix development shell:

```sh
nix develop path:. --command cargo tree -p agentix-qwen --edges normal
nix develop path:. --command cargo tree -p agentix-qwen-sys --edges normal
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
package builds with its CUDA feature and passes its current native boundary
tests; this verifies packaging and compilation, not execution on a CUDA device.
Its Nix source set retains both `agentix-qwen-sys` and its pure `agentix-qwen` trait
dependency so the package build sees the same interface as the workspace. This
documents the current package boundary, not the final T010 fixture/dependency
strategy or a substitute for T016 review.
