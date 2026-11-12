# Launch and acceptance

## Build without deploying

From `/home/sam/home/agentix/034-codex-jail-gpu`:

```bash
nix develop --command cargo test -p agentix-jails --bin codex-jail --test codex_jail
nix develop --command cargo clippy -p agentix-jails --all-targets -- -D warnings
nix build .#codex-jail
```

## Restart from the GPU host

Exit the current jail and run on the machine whose GPU should be used:

```bash
cd /home/sam/home/agentix/034-codex-jail-gpu
nix run .#codex-jail -- --all-worktrees --gpu
```

Equivalent installed-launcher syntax, once the owner chooses to install this version:

```bash
codex-jail --all-worktrees --gpu
```

No host configuration or installation is performed by this feature. Models
within the mounted repository family are already visible. If needed, add
`--ro /path/to/models` for a real model directory outside it; neither smoke test
requires model files. `--gpu` is local
to the launching machine, and does not forward SSH or credentials.

## Inside the newly launched jail

The checks below require an idle GPU authorized for this work. Default fixture
tests neither allocate GPU memory nor establish physical GPU acceptance.

```bash
cd /home/sam/home/agentix/034-codex-jail-gpu
# Bounded inspection of exactly the intended NVIDIA nodes on this host:
ls -l /dev/nvidiactl /dev/nvidia-uvm /dev/nvidia0
# Actual NVRTC 13.1 compilation + CUDA driver/PTX initialization + kernel + checked result:
nix develop .#gpu-validation --command cargo run -p agentix-jails --example gpu_cuda_smoke
# Actual headless Vulkan shader dispatch, synchronization and checked result:
nix develop .#gpu-validation --command cargo run -p agentix-jails \
  --features gpu-vulkan-smoke --bin gpu-vulkan-smoke
```

CUDA success prints `PASS: NVRTC 13.1, compute_<actual capability>, kernel result 42`.
The smoke test uses device ordinal 0 in the visible CUDA set. Missing libraries,
unsupported PTX, denied device access or an incorrect result cause failure.
On failure report the exact output before changing implementation or host setup.
The test process releases resources on exit, including after an early error.

Vulkan success identifies the hardware GPU and prints `shader result 42; no display
extensions`. This tool creates no surfaces, window-system/display extensions or
swapchains, and refuses software devices. It uses a Nix-compiled SPIR-V shader,
a compute queue, a shader-to-host memory barrier and a completed fence before
checking the output. SSH/tmux sessions do not need DISPLAY or XDG_RUNTIME_DIR.

The packaged equivalent is:

```bash
nix develop .#gpu-validation --command nix run .#gpu-vulkan-smoke
```

`vulkaninfo --summary` is not a required headless acceptance check: this build
queries `VK_KHR_display` properties and crashes in the current headless jail.
That display probe is distinct from compute. Both CUDA and the headless Vulkan
smoke tool have returned the correct result on the RTX 5090 Laptop GPU here.

For separate full-model inference acceptance, feature 033 supplies `agentix-qwen-bench` with
`AGENTIX_QWEN_COMPUTE=cpu|cuda|vulkan` (verified in its source, not executed here).
For an owner-selected compatible model and an idle GPU, set `MODEL_FIRST_SHARD`,
`AGENTIX_QWEN_TOKENIZER` and `AGENTIX_QWEN_CHAT_TEMPLATE` to existing files visible
inside the jail, then run:

```bash
: "${MODEL_FIRST_SHARD:?Set to the existing first GGUF shard}"
: "${AGENTIX_QWEN_TOKENIZER:?Set to the existing tokenizer JSON}"
: "${AGENTIX_QWEN_CHAT_TEMPLATE:?Set to the existing chat template}"
export AGENTIX_QWEN_TOKENIZER AGENTIX_QWEN_CHAT_TEMPLATE
export AGENTIX_QWEN_BENCH_TOKENS=4 AGENTIX_QWEN_BENCH_RUNS=0
export AGENTIX_QWEN_CONTEXT=128
AGENTIX_QWEN_COMPUTE=cuda nix develop .#gpu-validation --command cargo run \
  --manifest-path /home/sam/home/agentix/033-rustingstrata-qwen/Cargo.toml \
  -p agentix-qwen --bin agentix-qwen-bench -- "$MODEL_FIRST_SHARD"
AGENTIX_QWEN_COMPUTE=vulkan nix develop .#gpu-validation --command cargo run \
  --manifest-path /home/sam/home/agentix/033-rustingstrata-qwen/Cargo.toml \
  -p agentix-qwen --bin agentix-qwen-bench -- "$MODEL_FIRST_SHARD"
AGENTIX_QWEN_COMPUTE=cpu nix develop .#gpu-validation --command cargo run \
  --manifest-path /home/sam/home/agentix/033-rustingstrata-qwen/Cargo.toml \
  -p agentix-qwen --bin agentix-qwen-bench -- "$MODEL_FIRST_SHARD"
```

Require the reported backend to match,
successful prefill/decode, sensible output and comparison of generated token IDs
with the CPU reference; investigate disagreement rather than accepting a timing
result alone. These commands run inference and may consume substantial model/GPU
memory. They depend on feature 033's own build being ready and are documented
coordination, not evidence that its integration passes. No files in 033 were edited.

## Feature 033 coordination

Feature 033 pins cudarc 0.19.10 and `cuda-13010` selects 13.1. Use the
`gpu-validation` shell to establish the runtime baseline. It supplies matching
NVRTC/builtins from this flake's pinned nixpkgs (`cudaPackages_13_1.cuda_nvrtc`,
evaluated version 13.1.115), the Vulkan loader and host driver library path.
The existing CUDA 12 shell remains unchanged for llama builds. If feature 033
needs cuBLAS/cudart or other selected cudarc components, its own reproducible
environment must supply compatible 13.1 packages; the launcher does not guess
or install these and no changes are made to the feature-033 worktree.

Missing-device errors: launch outside the existing jail on the actual GPU host
and check its pre-existing driver/device setup. Permission errors: the owner must
check host ACL/group/cgroup policy. Missing-driver errors: check the host's
`/run/opengl-driver/lib`, selected `LD_LIBRARY_PATH` and ICD manifests. Do not
put CUDA toolkit stub libraries in runtime search paths. Multi-GPU sharing,
MIG partitions and remote GPU access are not supplied by this profile.
