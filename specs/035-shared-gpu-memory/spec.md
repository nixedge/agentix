# Feature Specification: Shared GPU memory and expert reclamation

**Feature Branch**: `035-shared-gpu-memory`
**Created**: 2026-11-12
**Status**: Implemented; CPU/package checks and local GPU core acceptance pass; kursk coexistence/calibration pending
**Input**: Owner's [master handoff](request.md).

## User Scenarios & Testing

### US1 — Use embeddings and speech while Qwen is warm (P1)

As the operator, I can index code or transcribe speech without stopping Qwen.

1. Given Qwen has GPU expert copies, when another worker requests memory, then
   cached copies are reclaimed at an engine boundary and released bytes are
   acknowledged before that worker allocates.
2. Given reclamation, when Qwen generates again, then CPU-backed experts remain
   usable without restarting or reloading the whole model.
3. Given another worker's active allocation, when Qwen considers refilling its
   cache, then the competing reservation and refill hysteresis prevent it.

Independent test: CPU-backed coordinator simulations plus an opt-in bounded
physical sequence: warm Qwen, embed, transcribe, generate again, observing free
VRAM and process identity throughout. No laptop GPU load or stress tests.

### US2 — Admit memory safely under concurrent load (P1)

1. Given active, idle and pending allocations, when another load arrives, then
   all are charged before native allocation begins, including context, scratch,
   batch growth, loading peaks and a shared device safety reserve.
2. Given insufficient non-reclaimable capacity, when a bounded wait expires,
   then the caller receives an explicit capacity error, not a native allocator
   failure or implicit CPU/backend fallback.
3. Given competing requests, then explicit priority and FIFO ordering among
   equal priorities govern admission without evicting live context.

Independent test: competing real IPC clients with fake device probes/reclaimers
verify no overcommit or admission before release acknowledgement.

### US3 — Recover safely and observe ownership (P2)

1. Given cancellation or a failed load, then unused reservations are released;
   allocations still alive remain charged until their destructors finish.
2. Given a crashed worker, then stale ownership is reconciled with process
   liveness and measured device usage; a timeout alone never proves VRAM freed.
3. Given unrelated GPU usage or stale/unavailable telemetry, then admission
   conservatively accounts for it or fails closed.
4. Metrics identify resident/allocated, reserved, reclaimable and pending bytes
   per worker, actual released bytes, waits, failures and external device usage.

## Functional Requirements

- FR-001: One shared per-device allocation/lease coordinator across separate
  Qwen, llama/indexing and Whisper processes; policy remains a Rust library.
- FR-002: Admission precedes native allocation. Context and scratch reserves
  cannot be lent to expert caching. Loading peaks and future growth are charged.
- FR-003: Count active models, concurrent loads and idle models; keep leases
  alive through actual native allocation destruction, including streaming work.
- FR-004: Reclaim actual Qwen GPU expert allocations while keeping CPU weights,
  KV/recurrent state and in-flight buffers. Synchronize streams and invalidate
  pointer-bearing graphs before changing allocation ownership.
- FR-005: Reclamation acknowledgements follow successful release and device
  synchronization; never credit a requested logical decrease as released memory.
- FR-006: Bound waits; enforce priority, cancellation, crash handling and refill
  hysteresis. A busy worker can defer reclaim until a safe boundary.
- FR-007: Reconcile reservations and allocation reports with fresh device
  measurements, conservatively accounting for external users and driver reserve.
- FR-008: Expose coordinator and per-worker metrics. Unavailable measurements
  are explicit, not zero-valued substitutes for successful collection.
- FR-009: Supply additive Nix packages/options and deployment instructions.
  Standalone CPU and uncoordinated configurations retain compatibility; enabling
  shared management requires all GPU workers to participate or be accounted as
  external usage. No silent unmanaged fallback on coordinator failure.
- FR-010: Derive deployable budgets from measured peaks with the actual model,
  context and batch configuration. Do not label configured guesses as calibrated.

## Key Entities

- Device observation: UUID/index, total/free bytes, monotonic sample age.
- Worker: process incarnation, priority and reclaim endpoint; reconnect is not
  permission to forget live allocations.
- Lease: allocation owner and lifetime, reserved peak, resident allocation,
  reclaimable cache, pending/active/releasing state and request deadline.
- Reclaim acknowledgement: actual remaining cache/resident bytes and release
  completion; associated with a specific request/worker incarnation.

## Edge Cases

Simultaneous first loads; active guard dropped before stream completion;
cancelled load while blocking work continues; failed context creation; delayed
CUDA frees; cache graph pointers after shrink; CPU-only model; all experts on
CPU; non-reclaimable working set larger than device; stale telemetry; coordinator
restart with live workers; worker crash during release; refill immediately
followed by new competing demand; malicious/invalid/overflowing byte counts.

## Success Criteria

- SC-001: Deterministic admission tests never exceed available capacity or admit
  a competing load before actual reclaim acknowledgement.
- SC-002: Failure/cancellation/crash tests retain charges while allocations may
  remain alive and release them after confirmed cleanup.
- SC-003: A bounded physical test on kursk admits embedding and Whisper work
  with Qwen warm, measures reclaimed VRAM, preserves reserved context capacity,
  and generates afterward from the same Qwen process/model instance.
- SC-004: Strict focused lint, formatting, relevant/workspace tests and canonical
  Nix builds pass, with pre-existing failures and hardware gaps stated precisely.

## Assumptions and Limits

Kursk has a 24 GiB RTX 3090 and 128 GB RAM. GPU metrics are available at
`http://10.40.33.71:9835/metrics`, node metrics at
`http://ai.lan.disasm.us:9100/metrics`. External allocations can race any userspace
admission decision; failures remain explicit and measured usage is conservative.
Sam owns deployment/SSH. No deployment, driver change or long stress run is
authorized. After implementation, the owner explicitly authorized bounded local
RTX 5090 Laptop validation; retain thermal/time guards. Kursk acceptance remains
separate from CPU simulation and local hardware results. The old feature-021
engine stays historical.
