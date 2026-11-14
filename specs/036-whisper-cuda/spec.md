# 036 — Restore Whisper CUDA packaging

Base: master `bc91122`. Requested: fix Whisper GPU support independently of shared GPU memory work.

## User story
P1: As the operator, I need the packaged Whisper worker to use NVIDIA GPU acceleration for speech latency when CUDA capabilities are configured.

Acceptance: Given configured CUDA capabilities, building the stock Whisper package enables the pinned whisper-rs CUDA backend and supplies its build/runtime libraries. Given no CUDA capabilities, a CPU package remains available. Existing socket transcription and persistent worker state continue to work.

## Requirements
- Forward the crate CUDA feature to whisper-rs; remove the ineffective local build script.
- Apply the same configured CUDA architectures and native dependency wiring used by llama to Whisper.
- Preserve isolated per-crate builds, pinned dependencies, CPU support, and existing service options.
- Document deployment and verify native CUDA linkage, not just Cargo feature selection.
- No changes from feature 035, model downloads beyond the existing pinned tiny fixture, deployment, GPU resets, or display-process interruption.

## Success criteria
CPU crate tests and canonical CUDA package build pass. The packaged worker socket smoke passes. A short physical GPU fixture check proves CUDA use where available, bounded to 30 seconds and 65°C. Required formatting, Clippy, workspace tests and lock checks pass before merge; limitations must be recorded.

## Approved shutdown follow-up

The owner approved fixing the GPU cleanup race after hardware acceptance exposed it. Closing the worker channel must precede waiting for thread completion; native state/context destruction must finish before normal Tokio process teardown. Joining must not block an async runtime worker. Validate cleanup with deterministic worker-lifetime tests and reject CUDA teardown errors even if the process reports success.
