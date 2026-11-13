# Implementation Plan: Shared GPU memory

**Branch**: `035-shared-gpu-memory` | **Date**: 2026-11-12 | **Spec**: [spec.md](spec.md)

## Summary

Create an independent pure-Rust GPU admission library and a separate local
coordinator binary. Integrate workers before native allocation and keep leases
with the allocation owners until destruction. Extend the pinned RustingStrata
engine to release expert buffers at synchronized boundaries and invalidate
captured graphs. Treat configured peak budgets as uncalibrated until measured.

## Technical Context

Rust 2021, Nix/fenix, Tokio/serde/Axum, Unix sockets. Runtime device observations
come from the host NVIDIA driver tooling; tests inject observations. No new C++
dependencies in coordinator, router or agentix-infer. Storage is ephemeral;
coordinator recovery must reconcile with live workers and measured device usage.

## Constitution Check

- Library-first: admission/policy/types in a standalone Rust library; only the
  coordinator binary binds its local management listener. This adds an explicit
  dedicated infrastructure binary, analogous to the existing worker processes;
  it does not add a public inference API or listener in a library.
- Nix supplies packages and services; lockfile and isolated-workspace closure
  must include any locally patched RustingStrata source with original notices.
- Blocking device/native work stays off Tokio; no raw pointer lifetime tricks.
- Unit/IPC/inference-fixture tests cover transitions and allocation lifetime;
  physical release remains a separate opt-in gate with no laptop GPU activity.
- Do not change routing/cloud policy or public inference request schemas.

## Source Audit and Design Gates

See [research.md](research.md). Complete the IPC/ownership contract before
implementation. The deployed pin lacks cache resizing and graph invalidation;
the extension must change actual buffer ownership, not only cache placement or
the environment variable used at startup. CPU weight mappings remain alive.
Loading a replacement full model is not an acceptable reclaim implementation.

## Planned Structure

`agentix-gpu/`: pure policy, lease protocol/client, metrics, coordinator binary.
`agentix-infer/`: allocation admission, pool accounting and load cancellation.
`agentix-llama/`, `agentix-whisper/`: conservative peak plans and native owner
lifetimes. `agentix-qwen/`: safe control commands and runtime expert release.
`agentix-qwen-worker/`: coordinator registration/control integration.
`flake/nixosModules/agentix.nix`, `perSystem/packages.nix`: opt-in service wiring.

## Validation

Bound CPU build concurrency. Test fake multi-process ownership, delayed reclaim,
active streams, failed loads, cancellation, crashes, telemetry failures and cache
refill cooldown. Run focused Clippy/format, workspace tests and canonical Nix
packages. Do not claim GPU cache release from CPU tests. Prepare a short kursk
acceptance run with before/during/after metrics and same-process Qwen generation;
deployment remains operator-owned.
