# Agentix development handoff: shared GPU memory and expert reclamation

Implement this in Agentix. The network repository handles configuration/builds;
Sam deploys Kursk and retains SSH keys.

## Current deployment and evidence

- Kursk: RTX 3090, 24 GiB VRAM; NVIDIA driver 595.71.05.
- Agentix pin: `bc91122e1d2a5f11839d2d3635134699cbcede68`.
- RustingStrata pin: `9dd6340d25c7c6770f7918134a27984cf20d7ce3`.
- Qwen: `qwen3.8-flash-next`, IQ3_S split GGUF, CUDA device 0,
  context 4096, prefill chunk 256, `vramMargin = 536870912` (512 MiB).
- Llama: all GPU layers, `vramLimitBytes = 23000000000`; indexing uses
  `hf.co/jinaai/jina-code-embeddings-1.5b-GGUF:Q8_0`.
- Whisper preloads `ggerganov/whisper.cpp:ggml-large-v3-turbo.bin`.
- With Qwen running, embedding/model loading failed with `NullResult` despite
  llama, Qwen and gateway systemd services being active. GPU pressure is strongly
  implicated, but that error alone does not prove an out-of-memory condition.
- After Sam stopped Qwen, the failed exporter-source ingestion and code search
  succeeded; approximately 19.1 GiB VRAM was free.
- After Qwen restarted: 23.07 GiB used, 506 MiB free, about 450 MiB driver-reserved.
  These are device totals; per-worker GPU attribution is unavailable.

Monitoring works: GPU `http://10.40.33.71:9835/metrics`, node exporter
`http://ai.lan.disasm.us:9100/metrics`. GPU collection exit code is 0;
Prometheus job `nvidia-gpu` is up and scrapes every 10s. Direct HTTP sampling
can run about once per second. Worker RSS/cgroup memory remains unavailable.

## Requested behavior

Coordinate memory across Qwen, llama/indexing and Whisper. Reserve room for
context/KV state, scratch buffers, batch growth, loading peaks and a device safety
margin before admitting allocations. Expert caching gets the remaining budget.

When another worker needs VRAM, reclaim Qwen's cached GPU expert copies while
retaining CPU-backed weights. Qwen should continue using CPU experts; normal
reclamation must not require stopping/restarting Qwen or reloading the whole model.
Free allocations only at safe CUDA/engine boundaries; acknowledge actual released
bytes before admitting competing allocations. Never discard live context or
in-flight buffers. If non-reclaimable memory cannot fit, queue with a bounded wait
or return an explicit capacity error. Prevent immediate cache refill and thrashing.

## Relevant implementation gaps in the deployed pin

- `agentix-infer/src/engine.rs::load_model` calls `backend.load` before the pool
  checks its budget: allocation admission currently happens too late.
- `agentix-infer/src/pool.rs` accounts for idle models, excluding active guards
  and concurrent loads; it does not coordinate the separate worker processes.
- `agentix-llama/src/lib.rs` reports model file size as its VRAM estimate, omitting
  context/KV and scratch. Its embedding context ceiling is currently 8192 tokens.
- Qwen owns a separate RustingStrata engine and has no runtime reclaim/unload API.
  Verify the pinned engine's expert-cache ownership/resizing support and extend
  it if needed; lowering a logical budget must actually release GPU allocations.

Use a shared allocation/lease coordinator covering resident, active and pending
allocations. Reconcile with measured device usage, including external processes.
Add priorities, cancellation/crash handling, refill hysteresis and per-worker
allocated/reserved/reclaimable metrics. Determine budgets from measured peaks.

Acceptance: with Qwen warm, bounded embedding and Whisper requests can obtain
memory, GPU expert reclamation measurably increases free VRAM, context growth
keeps its reserve, and Qwen still generates afterward without a restart. Test
concurrent requests, failed loads, cancellation and worker crashes. Use Spec Kit
in Agentix before implementation. No long stress tests or laptop GPU load.
