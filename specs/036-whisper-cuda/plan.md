# Implementation plan

Use existing Rust 2021 whisper-rs 0.14.4 / whisper-rs-sys 0.13.1 and Nix CUDA 12 toolchain. Verify upstream feature/build behavior with agentic-nix. Change only Whisper Cargo/build wiring and the Nix package CUDA predicate, including driver-library fixup. Keep CPU builds enabled by an empty cudaCapabilities configuration.

Constitution: native code remains isolated in agentix-whisper, Nix remains canonical, no API/routing/sandbox changes, no new dependencies or model formats. Existing pinned tiny Whisper fixture supplies hardware acceptance. No new data model or API contracts are required.

Validation: Cargo features and CPU tests/Clippy/fmt; canonical Nix CUDA package, startup smoke and workspace lock check; inspect native artifact backend; optional bounded fixture transcription on physical GPU. Limit compilation parallelism to two jobs to respect laptop thermal/load constraints. Record commands/results and deployment requirements in validation.md.

## Worker cleanup

Retain the native thread JoinHandle. On worker drop, close its sender first, then join through Tokio's blocking pool when a runtime is available, or join directly outside a runtime. Tokio runtime shutdown waits for the blocking join task. Explicitly drop native state before context before the native thread returns. Keep the existing persistent worker and inference path. Test runtime responsiveness and native cleanup completion, then rebuild the canonical CUDA package and repeat the bounded physical fixture with strict log checks.
