# Feature Specification: Opt-in GPU access for codex-jail

**Feature Branch**: `034-codex-jail-gpu`

**Created**: 2026-11-11

**Status**: Implemented; automated gates and physical CUDA/headless Vulkan compute passed

**Input**: Repository-family `GPU-JAIL-PROMPT.txt`; owner explicitly authorizes the narrow permission expansion and implementation.

## User Scenarios & Testing

### User Story 1 — Run local GPU compute (Priority: P1)

As the owner, I can explicitly grant the GPU on the machine launching codex-jail to a session for CUDA/Vulkan inference development.

**Independent Test**: Inspect the real launcher's assembled arguments, then separately initialize a physical backend and execute a small compute operation inside a real jail.

1. Given accessible NVIDIA compute devices and driver libraries, when I launch with `--gpu`, then the jail receives only the required device nodes and read-only driver files and can initialize CUDA.
2. Given an accessible DRM render device and Vulkan driver, when I launch with `--gpu`, then headless Vulkan compute can use it without display or audio access.
3. Given a launch without `--gpu`, then GPU discovery, mounts and environment changes are absent.

### User Story 2 — Diagnose unavailable access (Priority: P1)

As the owner, I receive an actionable error before session startup if requested GPU access cannot work.

**Independent Test**: Simulate missing devices, denied permissions and missing driver libraries independently using an internal discovery seam.

1. Given no supported device nodes, when I request `--gpu`, then startup fails with a message to launch on the GPU host and check host device setup.
2. Given inaccessible devices or absent libraries, then the error distinguishes permissions from missing drivers; no CPU fallback or automatic host setup occurs.

### User Story 3 — Keep existing development workflows (Priority: P2)

As a feature-033 developer, I combine GPU access with sibling worktrees and read-only models and use a reproducible compatible runtime environment.

**Independent Test**: Assemble `--gpu --all-worktrees --ro PATH` through the launcher fixture and inspect final environment ordering; follow documented hardware validation commands.

1. Given host and direnv library settings, then GPU setup preserves unrelated paths and survives final environment initialization.
2. Given `--all-worktrees` and `--ro`, then existing worktree/model mounts, credential policy, proxy behavior and debug redaction remain intact.

## Requirements

- **FR-001**: Add documented `--gpu` to codex-jail only; disabled by default.
- **FR-002**: Discover devices on the launching host with bounded reads of relevant directories; use deliberate device bindings for NVIDIA control/UVM/GPU nodes and DRM render nodes. Inspect requirements for optional capability nodes and driver information.
- **FR-003**: Never bind all of `/dev` or `/run`, expose display/audio sockets, change permissions/groups, or load kernel modules automatically.
- **FR-004**: Mount actual host driver directories including `/run/opengl-driver` when present read-only, retaining Nix symlink targets; discover CUDA/Vulkan libraries and ICD manifests.
- **FR-005**: Preserve relevant existing host/direnv runtime paths without forwarding arbitrary host variables or erasing GPU settings at final environment setup.
- **FR-006**: Keep NVRTC/toolkits in Nix environments; establish cudarc 0.19 `cuda-13010` requirements from source and document feature-033 coordination without editing that worktree.
- **FR-007**: Fail early for no supported accessible backend; distinguish absent devices, permissions, libraries and driver configuration. Do not claim a CPU fallback fulfills GPU access.
- **FR-008**: Preserve namespace, credentials, SSH/GitHub proxy and default-launch behavior; compose with `--ro` and `--all-worktrees`. Local GPU access does not imply remote access.
- **FR-009**: Regression tests exercise actual launcher argument assembly with an internal Rust discovery seam, never a production environment override for fake host devices. Keep hardware acceptance separate.
- **FR-010**: Document exact mounts, usage, limitations and host restart/compute validation commands; do not invent an owner model path.
- **FR-011**: Run focused tests, formatting, strict Clippy, workspace checks and canonical Nix package build; record blockers and physical acceptance honestly.

## Edge Cases

- Only DRM or only NVIDIA available; multiple GPUs; inaccessible nodes; device disappearance after discovery.
- Absent UVM node; optional profiling/MIG nodes; unavailable driver paths or broken symlinks.
- Existing library paths, ICD overrides, direnv-provided runtime libraries and spaces in mounted paths.
- Nested jail without host devices; GPU kernel module visible through proc but devices absent.

## Success Criteria

- **SC-001**: Every default-launch regression passes with no new GPU access.
- **SC-002**: Device, driver, environment and composition cases pass deterministic launcher tests.
- **SC-003**: Supported hardware initializes a backend and returns correct results for a small compute operation; CUDA validation includes NVRTC compilation and kernel execution. Pending hardware remains explicitly pending.
- **SC-004**: All required local quality gates pass, or exact failures are recorded with incomplete tasks left open.

## Assumptions and Scope

- Existing Linux/NixOS bubblewrap launcher and host GPU configuration are reused.
- The owner controls resource scheduling; this does not add GPU quotas or remote forwarding.
- No changes to features 021/033, active master, deployment or running host configuration.
