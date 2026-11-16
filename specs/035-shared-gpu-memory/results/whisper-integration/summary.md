# Automatic GPU handoff acceptance — passed

Validated on the RTX 5090 Laptop GPU, 2026-11-14, using real coordinator and
worker packages listed in packages.json. Branch035 is rebased onto signed master
f427067 (CUDA Whisper); the follow-up commit includes these fixes and results.

## Observed behavior

1. Production Qwen shards loaded with context128, prefill64, two CPU/Rayon/OpenMP
   threads and a512MiB expert-cache cap. Qwen PID126959 answered the first request.
2. A competing real Jina indexing-model request triggered automatic cache reclaim.
   Qwen cache reservation fell from530,073,600bytes to zero. NVIDIA observed free
   VRAM rise17,979→18,459MiB before Jina allocation, while Qwen residency fell
   5,042→4,562MiB: **480MiB actually returned to the device**.
3. The real jina-code-embeddings-1.5b Q8_0 GGUF produced a1536-dimensional embedding
   for a short code snippet. Driver-observed llama residency was2388MiB; CUDA
   compilation and all-layer offload were enabled. Jina's worker shut down cleanly.
4. Pinned tiny.en Whisper completed a3-second synthetic-audio transcription on
   CUDA, with581,959,680bytes observed worker residency. Empty transcript is
   expected for this non-speech fixture; this verifies allocation/execution and
   cleanup, not speech accuracy. Whisper remained resident during the next step.
5. The unchanged Qwen PID returned “OK” after reclamation, using CPU experts with
   its live model retained. Whisper then shut down cleanly; no native CUDA errors
   or panics appeared. Owned test workers were stopped; niri was untouched.

See handoff.log, qwen-before.json, qwen-after.json, jina-embedding.json,
handoff-transcript.json, worker logs, metrics snapshots and smi-observations.log.
reproduce.sh preserves the exact local acceptance driver and model/package paths.

## Bounds and resources

Owner approved120seconds overall,30-second HTTP deadlines,65°C cutoff, minimum
8GiB MemAvailable, two CPU/Rayon/OpenMP threads and512MiB expert cap. Successful
run sampled85seconds including shard verification, maximum56°C, peak total GPU
memory7351MiB, peak62.23W, minimum MemAvailable25,780,236KiB (24.59GiB).
After cleanup:50°C,0% utilization,958MiB total GPU memory (baseline),15.86W.
Resource samples and summary are retained. No production inference, deployment,
SSH, GPU reset or compositor termination was performed.

## Limits

This proves automatic expert-cache reduction, real Jina allocation, CUDA tiny
Whisper execution and same-process Qwen continuation. It does not implement full
Qwen eviction. Fixed allocations that still cannot fit result in bounded503.

To trigger admission pressure without filling the laptop GPU, the test deliberately
sets Jina's admission ceiling just128MiB above current headroom, then releases that
inflated lease before the independent Whisper check. Thus this is sequential
handoff acceptance, not simultaneous three-model capacity calibration. Qwen uses
STRATA_FIXED_EXPERTS=1 to bound the resource test, and one-token requests; neither
answer quality nor production throughput is measured. Production Whisper model
sizes and kursk context/batch/model peaks still require calibration. Admission is
opt-in and must use matching worker packages and valid measured plans.

## Integration corrections and verification

Real Jina imports exposed a capability heuristic that relied on source filenames
lost after blob hashing. Metadata parsing now considers GGUF general.name while
preserving explicit capability precedence; actual binary-parser regressions cover
hashed blob paths. Llama also discarded its native thread handle, allowing CUDA
process teardown to race cleanup. It now retains the handle before initialization
await, closes dispatch and joins through the runtime blocking pool at drop.
Lifetime regressions verify cleanup and async responsiveness.

Full workspace:367passed,13ignored, zero failures; focused strict all-target
Clippy and cargo formatting pass (llama-fix-cargo.log). Canonical coordinator,
Qwen worker, CUDA llama, CUDA Whisper, CPU Whisper, daemon, Whisper socket smoke
and workspace-lock Nix builds pass (llama-fix-nix.log). NixOS module evaluation
has no failed assertions (module.json). git diff --check passes.

## Preserved failed attempts

- first-handoff-attempt:90s cap,15s HTTP timeout hit first Qwen prefill before
  reclamation. Owner subsequently approved the current120s/30s/one-token bounds.
- second-handoff-attempt: actual Jina misclassified after blob hashing; fixed
  metadata parsing, without replacing its model or inference backend.
- third-handoff-attempt: automatic480MiB release occurred, but the test supplied
  context256 while embedding allocation requires8192. Corrected test shape to
 8192context/batch; production shape checks remain enforced.
- fourth-handoff-attempt: automatic480MiB release and real Jina embedding passed,
  then detached llama native cleanup caused a CUDA abort. Fixed thread ownership;
  the final run passed the same checks and clean shutdown.
