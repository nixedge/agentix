# Source audit

Base: Agentix `bc91122`; RustingStrata `9dd6340d25c7c6770f7918134a27984cf20d7ce3`.

- `agentix-infer/src/engine.rs::load_model`: calls backend.load before pool
  admission; cancellation can leave a loading sentinel. Same-model loading has
  a check/register race and Notify wakeup race.
- `agentix-infer/src/pool.rs`: idle entries alone count toward both VRAM and
  model limits. Active guards and simultaneous loads are excluded. A completion
  stream can outlive its guard.
- `agentix-llama/src/lib.rs`: model-file-size estimate omits context/scratch;
  native owner runs on a detached thread and startup does not acknowledge
  successful context creation. Thread shutdown must precede lease release.
- `agentix-whisper/src/lib.rs`: weights and reusable state are owned on a
  separate thread. Admission must cover both and their lifetime.
- `agentix-qwen/src/engine.rs`: persistent thread owns Gguf/Model/GpuModel in
  stack order; session mutex spans a request. Management commands need their own
  channel access to run between engine operations without discarding context.
- Pinned RustingStrata `src/expert_cache.rs`: size classes own monolithic Buf
  pools. `adapt` only swaps expert contents; it does not free allocations.
  Per-layer GPU tables use -1 for CPU-backed experts.
- Pinned `src/gpu_model.rs`: cache is public, GPU backend is private. No runtime
  resizing API. Context/scratch precede cache allocation; startup expert budget
  is free memory minus margin, optionally capped by STRATA_EXPERT_MB.
- Pinned `src/backend.rs`: Buf frees its owner on drop; Backend exposes sync and
  graph capture but no invalidation method. CUDA stores captured graphs holding
  buffer addresses; freeing pools without invalidating these is unsafe.
- The agentic-nix inventory contains Agentix bc91122, cudarc 0.19.10 and the
  exporter, but not RustingStrata. Exact pinned source is available directly in
  Cargo's checkout; audit uses that source without running GPU-powered ingestion.
- The isolated Nix workspace explicitly supports path patches and includes
  their complete sources in dependency/build closures, allowing a documented,
  reproducible local extension of the pinned library.

## Implemented decisions

- Bounded newline JSON over local Unix sockets; management binds only in the
  coordinator/worker binaries. Role, process incarnation and client session
  identify ownership. Clients serialize snapshots/grants; releases bypass that
  lock and use tombstones to prevent stale snapshot resurrection.
- Native threads keep local and shared leases until weights and contexts drop.
  This also addresses load cancellation and detached thread shutdown. Llama
  initialization now acknowledges context creation instead of returning a handle
  whose native thread may already have failed.
- Managed profiles bind ordered model hashes and context/batch limits. Zero
  resident floors are conservative. Only audited persistent allocations may
  offset future-growth reservations, not a transient driver sample.
- cudarc 0.19.10 was checked through agentic-nix: CudaSlice uses async allocation
  where supported; `driver::result::device::get_mem_pool` and
  `driver::result::mem_pool::trim_to` provide the explicit unused-page release.
  The extension synchronizes both streams and preserves live buffers.
- Existing `whisper-rs` 0.14.4 defaults use_gpu to its compiled `_gpu` feature.
  Master's feature 036 now enables CUDA in the stock package and joins native
  worker cleanup before runtime exit. The shared lease remains in the native
  owner until state/context destruction; CPU-only builds still request no GPU
  allocation. GPU Whisper needs a measured profile and coexistence acceptance.
- Llama managed placement uses one physical device and disables split placement.
  CPU layer settings also explicitly disable KV/op offload.

Remaining operator gate: calibrated production peaks and bounded physical
reclamation/coexistence acceptance on kursk. No hardware evidence is inferred
from fake-observation IPC tests or CPU inference fixtures.
