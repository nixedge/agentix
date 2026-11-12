# Implementation Plan: RustingStrata Qwen replacement

Branch: `033-rustingstrata-qwen`; base master `fd9fe12`.

## Technical context
Rust 2021 Agentix wrapper plus Rust 2024 upstream `rusting_strata` 1.2.1, pinned Git revision `9dd6340d25c7c6770f7918134a27984cf20d7ce3`. Axum Unix-socket worker, existing gateway, Nix/crane isolated workspace packaging. GPU kernels are owned by RustingStrata; Agentix introduces no FFI.

## Constitution check
Library-first: network-free `agentix-qwen` wraps dependency and tokenizer; worker owns HTTP. Local inference only. Nix controls toolchain/runtime and dependency pinning. All inference on a dedicated thread, no Tokio blocking. Existing 021 custom engine is not imported. Upstream transitive unsafe/native implementation is isolated behind safe library calls, as with existing inference dependencies. Small GGUF fixture tests are mandatory; full model is a target acceptance gate. No jail permission changes implicit in this feature.

## Design
- Import only tokenizer/context admission and worker protocol/tool parsing from 021. Remove custom row store, compute graphs, reference execution, converter and sys crate from the new dependency graph.
- Persistent thread creates GGUF -> Model -> GpuModel in stack lifetime order, avoiding self references, leaked allocations or unsafe lifetime extension. Synchronous command/reply channels expose reset, prefill and step; request sessions serialize ownership. CPU reference supports fixture execution. Shutdown disconnects channels and joins the thread.
- GPU allocation happens once before readiness. Each request resets all library sequence state. Cancellation is checked between prefill chunks/decode steps. Upstream model-specific limitations are documented rather than papered over.
- Preserve HTTP validation and tools; adapt decoding to the library. Use direct library prefill, not a host projection loop.
- Add gateway Qwen socket/model configuration and NixOS service without replacing current assistant or other worker changes on master.
- Pin dependency in Cargo.lock and vendor with existing crane Git support. Add packages/checks and required NVRTC/Vulkan runtime paths. Retain upstream attribution in installed notices.

## Research and evidence
Upstream checkout: `/tmp/agentix-rustingstrata-033` at the pinned revision. `GpuModel::load`, `prefill_with`, `step`, `reset` and backend::open are public. Gguf::open discovers split shards using standard `-00001-of-NNNNN.gguf` naming. Model/GpuModel borrow weights, so ownership must remain on one thread. CUDA is dynamically loaded and kernels compile via NVRTC. Parallel slots and MTP are unavailable for hyper-connection models; ordinary decode remains the initial path.

## Validation
Nix dev shell; meaningful library fixture and HTTP tests; focused Clippy/format; canonical Nix package build; workspace tests. Produce exact target commands and keep target acceptance pending unless hardware and artifact are available.
