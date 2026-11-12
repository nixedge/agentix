# Implementation Plan: Opt-in GPU access

**Branch**: `034-codex-jail-gpu` | **Date**: 2026-11-11 | **Spec**: [spec.md](spec.md)

## Summary

Extend only codex-jail with `--gpu`. Discover supported devices before proxy or jail startup, construct a validated internal GPU access plan, and append individual `--dev-bind` mounts after the fresh `/dev`. Mount driver directories read-only and apply selected GPU environment variables after direnv and `--clearenv` processing.

## Technical Context

- Rust 2021, existing anyhow/clap/serde_json/tempfile dependencies; Linux/NixOS bubblewrap.
- No new crate, persistent storage, listener or credentials.
- Small private `gpu.rs` module for bounded host discovery, validation and command assembly.
- A standalone Rust CUDA acceptance example uses the Driver/NVRTC C ABI through dynamic loading without adding launcher dependencies. Its Nix package supplies build tooling; the separate validation shell supplies NVRTC 13.1.
- Discovery seam is Rust-only: tests supply a synthetic inventory; production always reads actual host paths. No production GPU fixture environment variables.
- Device access must be checked with real user credentials; no modprobe, chmod or group changes.
- Nix `/nix` mount preserves read-only driver symlink targets. Driver lookup includes `/run/opengl-driver/lib`, selected existing library paths and bounded conventional library directories; Vulkan manifests use explicit existing overrides or bounded standard ICD directories.
- CUDA toolkits/NVRTC remain Nix-managed. Research cudarc 0.19.10's loader and `cuda-13010` before choosing runtime documentation/environment; do not presume CUDA 12 compatibility.
- Validate argument construction with existing fake-bwrap fixture pattern and synthetic discovery within the actual launcher path; real GPU compute is a separate acceptance gate.
- Read-only directory mounts cover their descendants; omit redundant child mounts and mounts already covered by `/nix`. Test NixOS-style symlinked ICD traversal using real bubblewrap without GPU devices.
- Headless Vulkan acceptance uses a separate optional `gpu-vulkan-smoke` Rust binary with ash 0.38 (the same version pinned by feature 033), a Nix-compiled SPIR-V compute shader, a compute-only queue, host-coherent output and a checked result. It enables no instance/device extensions, layers, surfaces or presentation. Ordinary launcher builds leave this feature disabled. No jail permissions change for SSH/tmux validation.

## Constitution Check

- I/V/VII: existing standalone launcher; inference/routing/state machines untouched.
- II: no cloud or inference routing changes.
- III: development and validation through existing Nix shell; canonical `codex-jail` package build required.
- IV: explicit owner authorization in input prompt; minimal opt-in devices and read-only drivers documented in ARCHITECTURE.md. No default permission expansion, whole-device-directory mount, display/socket or credential forwarding.
- VI/VIII: focused launcher regression, strict Clippy, formatting, workspace tests and Nix build required. Physical compute recorded separately and never inferred from mocks.
- Post-design review: no new workspace crate or constitutional exception. ash is an optional dependency for the separate headless acceptance binary, absent from default launcher dependency trees. Security-sensitive changes remain subject to review before merge; no merge/deployment authorized here.

## Project Structure

```text
agentix-jails/src/codex_jail/{main.rs,gpu.rs}
agentix-jails/tests/codex_jail.rs
perSystem/packages.nix
README.md
ARCHITECTURE.md
specs/034-codex-jail-gpu/{spec,plan,research,tasks,quickstart,validation}.md
```

## Execution and Dependencies

Research actual driver/cudarc requirements first. Implement discovery and validation, then launcher integration, then regression fixtures and documentation. Run focused checks before the canonical package/workspace gates. On failure explain and ask before implementing a different approach. If physical hardware is unavailable, retain exact host acceptance commands and leave that gate open.

## Complexity Tracking

No constitutional violations. The discovery seam avoids privileged or environment-controlled fake devices in production. No persistent data model or network contract is needed; CLI contract is the opt-in flag plus documented mounts/errors.
