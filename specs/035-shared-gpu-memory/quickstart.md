# Shared GPU memory deployment and verification

Feature worktree: `/home/sam/home/agentix/035-shared-gpu-memory`.
No deployment or kursk physical acceptance has been performed for this feature.
Subsequent owner-authorized, bounded RTX 5090 Laptop results are recorded in
[local-5090/summary.md](results/local-5090/summary.md).
Master's incoming handoff remains unchanged; its snapshot is [request.md](request.md).

## Local CPU checks

Use the existing project shell. Bound compiler and native/CPU model threads:

```sh
export CARGO_BUILD_JOBS=2 CMAKE_BUILD_PARALLEL_LEVEL=2
export AGENTIX_CPU_THREADS=2 RAYON_NUM_THREADS=2 OMP_NUM_THREADS=2
export AGENTIX_GPU_LAYERS=0
nix develop .#workspace-tests --command cargo test \
  -p agentix-gpu -p agentix-infer -p agentix-qwen -p agentix-qwen-worker \
  -p agentix-llama -p agentix-whisper
nix develop .#workspace-tests --command cargo clippy \
  -p agentix-gpu -p agentix-infer -p agentix-qwen -p agentix-qwen-worker \
  -p agentix-llama -p agentix-whisper --all-targets -- -D warnings
nix develop .#workspace-tests --command cargo fmt --all --check
nix build --cores 2 --max-jobs 1 --no-link \
  .#agentix-gpu-coordinator .#agentix-qwen .#agentix-qwen-worker \
  .#agentix-llama .#agentix-whisper .#agentix-daemon
```

The IPC tests use fake observations and real local sockets/child processes, not
the host GPU. Hardware tests remain ignored and require explicit operator
authorization and external thermal/time limits. The vendored pinned library
retains upstream formatting and is
excluded from the repository-wide treefmt pass; Agentix Rust uses cargo fmt.

## Calibrate before enabling admission

For the initial calibration deployment, use matching packages from this branch
and keep shared admission disabled until measurements exist. Apply these
temporary overrides to the existing Colmena node configuration:

```nix
services.agentix = {
  gpu.enable = false;
  qwen = {
    context = 4096;
    prefillChunk = 256;
    extraEnv.STRATA_EXPERT_MB = "0";
  };
};
```

Keep the gateway and model paths, the existing llama GPU placement/context,
and the CUDA Whisper package. No guessed GPU plans are needed in this phase.
The operator must isolate calibration workloads: disable automatic preload of
competing models or stop their workers before measuring each backend, then
restore normal Whisper preloading after calibration to retain low latency.
The module does not serialize unmanaged GPU loads while admission is disabled.
Record the actual llama allocated context and batch rather than assuming they
equal a request's token count. Collect one model at a time with a fresh worker,
including load, maximum supported execution shape, and a short warm request.
Use the existing NVIDIA/node exporters for baseline monitoring; per-process
VRAM sampling is needed to establish model peaks. After measuring all three
backends, remove STRATA_EXPERT_MB, add their measured plans, and enable admission.

Do not deploy guessed byte budgets. On kursk, record the exact model hashes,
Agentix revision, GPU/driver identity, GPU layer placement, context and batch
settings. Measure load and execution peaks with competing workloads isolated
by the operator. Cover full supported prefill/batch shapes, context-dependent
paths and short decode. A sampled peak is a lower bound; include justified
headroom for missed transients, native allocator and driver overhead.

For Qwen, measure its fixed weights/context/scratch peak with expert slots
disabled (`STRATA_EXPERT_MB=0` at startup), then clear that temporary override
for managed deployment. Expert copies receive their own separate lease.
This is one-time calibration, not the normal runtime reclamation mechanism.
Normal reclamation requires neither restart nor whole-model reload.

ModelStore manifests identify llama/Whisper blobs by `sha256:` digests. Profiles
use those hashes, not filenames or file sizes. Qwen profiles use the same two
ordered expected hashes already configured for its immutable split GGUF.
Do not reuse a profile after changing weights, layer placement, hardware,
driver, backend implementation or execution settings without revalidation.

Leave `residentFloorBytes = 0` unless a persistent allocation floor is known
to remain allocated through the lease's entire active lifetime. Never use a
sampled scratch peak as a floor. Zero keeps admission conservative; measuring
and auditing persistent allocations can improve cache utilization later.

## Colmena/NixOS configuration

Deploy the coordinator and **all participating workers** from the same Agentix
revision. Keep the existing gateway, NVIDIA driver, model files and service
settings. Enable the additive module configuration:

```nix
# `agentixPackages` and `measured` come from the deployment repository.
# `measured` must contain the actual calibration results, not these field names
# copied as literal estimates.
services.agentix.gpu = {
  enable = true;
  package = agentixPackages.agentix-gpu-coordinator;
  device = 0;
  safetyBytes = 536870912;
  metricsAddress = "127.0.0.1:9488";
  plans = {
    "qwen3.8-flash-next" = {
      peakBytes = measured.qwenFixedPeakBytes;
      modelSha256 = [
        "4c1eb2ceb4915e1192f4f386021897bde56a97f40a0bb78bb86465e0f7d2aca3"
        "316b46f3a2dbd68c900f43136ab9449f9dcc3725dfd8c794847c204bc161e113"
      ];
      contextTokens = 4096;
      batchTokens = 256;
      priority = 100;
    };
    "hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0" = {
      peakBytes = measured.jinaPeakBytes;
      modelSha256 = [ measured.jinaBlobSha256 ];
      contextTokens = 8192;
      batchTokens = 8192;
      priority = 50;
    };
  };
};
```

Add a profile for every GPU-loaded model, including Whisper. Master's feature
036 enables CUDA for the stock Whisper package under the same configured CUDA
capabilities as llama. Its worker retains the lease through native cleanup and
joins before normal runtime exit. Whisper needs its own measured model-hash
profile. The explicit agentix-whisper-cpu package remains available; CPU backends
register for restart reconciliation but do not request GPU memory. CPU Whisper
results cannot establish GPU Whisper budgets or coexistence acceptance.

The module wires `/run/agentix-gpu/coordinator.sock`, the three worker roles,
device and serialized profiles. The coordinator runs as `agentix`, uses the
host-configured NVIDIA tooling and owns a dedicated systemd runtime directory.
Qwen control uses `/run/agentix/qwen-gpu.sock` (or the configured socketDir).
No management operation is added to the public gateway API. Metrics listen on
localhost by default; configure access through the existing monitoring setup.
Do not mix managed and older workers and assume a shared budget is enforced.
Do not set CUDA_VISIBLE_DEVICES: managed indices are physical driver indices.

Missing/mismatched hashes, context or batch profiles fail before model allocation.
Inference capacity failures return HTTP 503. Oversized context requests retain
their existing validation errors. Coordinator failure never selects another
backend or bypasses admission. Existing models may continue within already
reserved peaks while their snapshots reconcile.

## Metrics

Scrape coordinator `/metrics` alongside the existing NVIDIA exporter at
`http://10.40.33.71:9835/metrics` and node exporter at
`http://ai.lan.disasm.us:9100/metrics`.

- `agentix_gpu_reserved_bytes`, `agentix_gpu_pending_bytes`: per-worker peaks
  and queued demand; loading, active and idle allocations remain charged.
- `agentix_gpu_resident_bytes`: per-worker NVIDIA process memory, omitted when
  unavailable. MiB driver values are conservatively rounded down for admission.
- `agentix_gpu_resident_floor_bytes`: guaranteed persistent floor; not a claim
  that every reserved byte is already allocated.
- `agentix_gpu_cache_reserved_bytes`, `agentix_gpu_reclaimable_bytes`: expert
  budget versus populated persistent cache allocations.
- `agentix_gpu_worker_reconciled`, `agentix_gpu_worker_alive`: role registration
  and ownership state; neither is model readiness.
- `agentix_gpu_worker_info` records PID, start ticks and client session;
  `agentix_gpu_worker_identity_verified` reports current process verification.
- Device total/free, external usage, observation success/age, admission failures,
  reclaim failures and released allocation-byte counters.

Driver queries are bounded and observations older than one second are rejected.
Failed native cache-control operations retry release before another refill;
reservations remain charged until cleanup and reconciliation succeed.
Unknown process attribution is conservatively charged, not fabricated as zero
GPU usage. No RSS/cgroup memory exporter is added here; use existing host metrics
and keep that attribution limit explicit.

## Bounded physical acceptance — operator on kursk

1. Build/evaluate the Colmena node and deploy the matching packages and calibrated
   profiles. Confirm coordinator/worker health and reconciliation. Record Qwen's
   PID and start identity; do not restart it during the sequence below.
2. Sample GPU free/used VRAM, utilization, temperature/power, coordinator leases
   and node MemAvailable before/during/after, at roughly one-second cadence.
3. Warm Qwen with one small request. Issue a bounded embedding request through
   the existing gateway `http://ai.lan.disasm.us:11434`, then one short Whisper
   request. Record whether Whisper actually uses GPU or CPU.
4. Confirm a cache release, a measured increase in device free VRAM **before**
   competing GPU allocations, and successful embedding/transcription. Confirm
   the cache does not immediately refill across pending leases/cooldown.
5. Generate a short Qwen reply afterward from the same process/model instance.
   Use a bounded longer-context case only as needed to verify the calibrated
   context reserve; no sustained benchmark or laptop GPU load.
6. Record results and any explicit capacity errors. A CPU Whisper request does
   not complete the GPU-Whisper acceptance item. If testing fails, preserve
   evidence and ask before changing the allocation/reclamation approach.

An additional ignored fixture test verifies release/refill while a live sequence
holds its context, including invalidation of a warmed decode graph:

```sh
# Only with explicit operator authorization, GPU access and thermal/time guards.
AGENTIX_QWEN_GPU_TEST_BACKEND=cuda nix develop .#qwen --command cargo test \
  -p agentix-qwen --test engine gpu_expert_release_refill_preserves_the_live_session \
  -- --ignored --nocapture
```

Tiny fixture allocations may share CUDA pages with live weights, so the fixture
alone does not prove meaningful device-level free VRAM. Production expert pools
must show the actual release. The extension synchronizes copies, invalidates
graphs, frees expert pool owners and trims unused CUDA allocator pages while
keeping CPU mappings and live context intact.

The owner-authorized local production checks also pass; see
[the local results](results/local-5090/summary.md). They are separately ignored:
`gpu_production_engine_release_refill_continues_session` checks public controls,
and `gpu_production_reclaim_matches_cpu_checkpoint` checks exact state replay.
Both require `AGENTIX_QWEN_PRODUCTION_TEST_MODEL` pointing to the first cached
split GGUF. Use `--release`, `STRATA_FIXED_EXPERTS=1`, two Rayon/OpenMP threads,
and run each test separately with external 90-second/65°C/resource guards.
The fixed-expert setting isolates placement and avoids routing-profile updates.
Cross-placement CPU/GPU rounding is reported independently of state correctness.

Admission uses calibrated upper bounds, not interception of every C++ allocator
call. Wrong profiles or racing unmanaged GPU jobs can still cause a native
allocation failure; keep their limitations distinct from verified lease policy.
