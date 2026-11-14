# Validation

Base: master `bc91122e1d2a5f11839d2d3635134699cbcede68`.
Worktree: `/home/sam/home/agentix/036-whisper-cuda`.
Final acceptance: 2026-11-14.

## Passed after cleanup fix

- Full CPU workspace tests: **336 passed, 9 ignored, zero failed** (`results/cleanup-workspace.log`). The pinned Nix bubblewrap package supplies the jail tests. Hardware tests remain explicitly opt-in.
- Whisper tests and strict all-target Clippy (`cleanup-cargo.log`). New tests verify channel closure and joining without a runtime, plus async responsiveness and runtime shutdown waiting for pending cleanup.
- `cargo fmt --all --check` and pinned Nix formatting (`cleanup-nix-format.log`).
- Canonical CUDA package: `/nix/store/lw7mg81263dcx5w0qvwnkikn2pk582v2-agentix-whisper-0.1.0`.
- Canonical CPU package: `/nix/store/1q8b7clipzmj7vpsr2v1vajyr7xh5ffz-agentix-whisper-0.1.0`.
- CPU socket startup: `/nix/store/fhcydqkk21m21qmn1chi6f5fw88l5bp3-whisper-runtime`.
- Workspace lock check: `/nix/store/75ij982xqcc4x8khs6slm4p02crhgrzk-workspace-locks`.
- Native CUDA binary links libcublas.so.12, libcudart.so.12, and host-provided libcuda.so.1; its wrapper supplies /run/opengl-driver/lib. Architectures 86 and 120 compiled. Builds limited to two cores (`cleanup-nix.log`).
- Physical RTX 5090 Laptop: pinned tiny.en model registered through `/api/pull`; three seconds of synthetic audio transcribed over the packaged worker's Unix socket, followed by `/control/shutdown` and a successful process wait. **CUDA0 selected, 77.11 MB GPU model buffer, under one second for the whole worker run, no CUDA errors or cleanup panics** (`gpu-worker.log`, `gpu-smoke.log`). The script explicitly rejects native CUDA errors even if the exit status is zero.
- Corrected GPU smoke: maximum sampled temperature **44°C**, sampled total VRAM up to 973 MiB and power 15.33 W (`gpu-resources.csv`). After exit: 45°C, 958 MiB total VRAM, matching the first smoke sample (`gpu-after.csv`). These are device-wide samples, not per-process peak measurements; the short run can finish between samples. The earlier cleanup check reached 50°C, below the 65°C cutoff.

## Historical failures and resolution

- Initial telemetry deadline expired before launching the worker. The initial read-only query now has ten seconds; in-run monitoring remains bounded. No GPU work occurred in the original timeout.
- One-second synthetic audio became 990 ms after preprocessing and was rejected. The corrected fixture is three seconds. The short-audio evidence remains under `gpu-short-audio-*`.
- Original GPU runs transcribed but logged native cudaFree/cuMemUnmap teardown errors despite process status zero. They are preserved in `before-cleanup/` and `gpu-short-audio-worker.log`. The owner approved retaining the native JoinHandle, closing the channel before joining, and waiting via Tokio's blocking pool. Explicit state destruction precedes context destruction. Clean final logs confirm the race no longer reproduces.
- The first cleanup smoke exited cleanly without CUDA errors but the stricter harness rejected an unrelated preload warning: registering an absolute fixture path returns a model name which the preload warmup lookup does not normalize. Evidence is in `cleanup-first-smoke/`. The corrected harness registers the local fixture through the public model API before requesting transcription. Production preload code is unchanged.
- Initial Clippy found six existing unwrap/expect calls in the ignored integration test. Converting that test to return errors resolved the failures. The standalone formatter was absent from the minimal shell; the repository's pinned `nix fmt` supplies it.

## Scope and limits

Niri remained running; no GPU reset, driver change, production model workload or deployment occurred. GPU runs retained a 30-second timeout and 65°C cutoff. The synthetic tone returned empty text, as expected: this validates CUDA execution and cleanup, not speech recognition accuracy or production throughput. This branch contains no shared GPU coordinator changes from feature 035; its eventual Whisper budgets must be calibrated with CUDA enabled.

All local acceptance checks are complete. No merge, push, deployment or remote CI execution performed. Master and feature 035 remain unchanged.
