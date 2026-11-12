# Tasks: codex-jail GPU access

Input: [spec.md](spec.md), [plan.md](plan.md). Execute in order; no subagent delegation.

## Setup and research

- [x] T001 Allocate max+1 and verify sibling host worktree; create specification and requirements checklist.
- [x] T002 Inspect launcher, fixture pattern, Nix wrapper and constitution; write implementation plan.
- [x] T003 Establish host NVIDIA/DRM/capability requirements and cudarc 0.19.10 loader/runtime compatibility in `research.md`.

## US1 — Explicit local GPU compute

- [x] T004 Implement bounded device/driver discovery and validated mount/environment plan in `agentix-jails/src/codex_jail/gpu.rs` with private synthetic-inventory seam.
- [x] T005 Add opt-in parsing and integrate early discovery, device binds and final environment setup in `agentix-jails/src/codex_jail/main.rs`.
- [x] T006 Cover NVIDIA/DRM device selection, read-only drivers, ICDs, optional nodes and default absence with launcher argument fixtures.

## US2 — Actionable failures

- [x] T007 Add separate missing-device, permissions and missing-library diagnostics and regression coverage; no host mutation or fallback.

## US3 — Development workflow

- [x] T008 Test environment merge/order and `--all-worktrees --ro` composition through actual launcher assembly; preserve proxy and redaction tests.
- [x] T009 Document exact security expansion in `ARCHITECTURE.md`, usage in `README.md`, and feature-033 runtime/compute acceptance commands in `quickstart.md`; adjust Nix runtime paths only as required by established sources.

## Validation

- [x] T010 Run focused tests and strict focused Clippy through Nix; record evidence in `validation.md`.
- [x] T011 Run workspace formatting, strict Clippy, workspace tests and canonical `nix build .#codex-jail`; record exact failures and ask before alternative implementation.
- [x] T012 On a GPU host launch the real jail and prove backend initialization and correct compute including CUDA NVRTC/kernel execution. CUDA (`compute_120`) and headless Vulkan shader dispatch both returned 42 on the RTX 5090 Laptop GPU on 2026-11-12; Nix-built packages repeated both successes.
- [x] T013 Review final diff against requirements, mark completed tasks accurately, and provide exact restart command without inventing the model directory.
- [x] T014 Fix redundant child binds beneath read-only driver trees; verify a symlinked ICD with real bubblewrap, then rerun quality gates and the canonical package build.
- [x] T015 Add a Nix-packaged, optional Rust headless Vulkan compute smoke binary and shader; execute on physical hardware without display extensions or additional jail permissions, then rerun quality gates.
