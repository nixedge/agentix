# Validation evidence — updated 2026-11-12

Worktree `/home/sam/home/agentix/034-codex-jail-gpu`, base master `fd9fe12`.
Implementation and evidence are maintained on this feature branch. No pushes, merges, deployment, host GPU
configuration changes or edits to features 021/033. Master's unrelated WAV files remain untouched.

## Final automated gates: passed

All Cargo commands ran through the project's Nix development shell:

| Command | Result |
|---|---|
| `nix develop --command cargo fmt --all --check` | Passed |
| `nix develop --command cargo test --locked -p agentix-jails --features gpu-vulkan-smoke` | 34 passed, no failures; includes real-bubblewrap symlinked ICD regression; one subprocess fixture ignored in ordinary enumeration and invoked by its parent test |
| `nix develop --command cargo clippy --locked -p agentix-jails --all-targets --features gpu-vulkan-smoke -- -D warnings` | Passed, including both compute smoke tools |
| `nix develop --command cargo test --workspace` | Passed; existing opt-in installed-model/hardware tests retain their documented ignored status |
| `nix develop --command cargo clippy -- -D warnings` | Passed |
| `nix build .#codex-jail .#gpu-cuda-smoke --no-link --print-out-paths` | Both canonical packages built |
| `nix build .#codex-jail --no-link --print-out-paths` (2026-11-12) | Rebuilt successfully after the symlinked ICD correction |
| `nix build .#codex-jail .#gpu-cuda-smoke .#gpu-vulkan-smoke --no-link --print-out-paths` | All three packages built after adding headless compute validation |
| `nix develop .#gpu-validation --command ...` | Shell initializes with rustc 1.97.1, Vulkan tools and NVRTC 13.1 library path |
| Packaged `codex-jail --help` | Passed |
| Packaged `codex-jail --gpu --no-github-auth` in this device-less jail | Expected early missing-device error verified; no session launched |
| `git diff HEAD --check` | Passed |

The final Cargo verification sequence ran with `set -e` and exited zero. Full local
output after headless Vulkan validation: `/tmp/agentix-034-vulkan-quality.log`. Package output:
`/tmp/agentix-034-vulkan-packages.log`. Shell output:
`/tmp/agentix-034-gpu-shell.log`. Packaged negative-preflight output:
`/tmp/agentix-034-preflight.log`.

Canonical outputs:

- `/nix/store/9ggdbraxksmk9h1gsdp81wk5dcbbngr1-codex-jail`
- `/nix/store/av3z9gv9187zyk8jhyfwvgnx1sh5k8w2-gpu-cuda-smoke`
- `/nix/store/py8g6075djqm9ddjmi9y4lin41rjn8ia-gpu-vulkan-smoke-0.1.0`

The existing kokoro C build emits `_FORTIFY_SOURCE requires compiling with optimization`
build-script warnings; the strict Rust Clippy command nevertheless exits zero. No lint
allowances or relaxed warning flags were added.

## Coverage and review

Synthetic host tests exercise discovery and the actual launcher's argument construction
through a private Rust seam. No production device-root override exists. Coverage includes
default discovery absence, character-device rejection, NVIDIA/DRM selection, denied devices,
unreadable libraries, missing nodes/libraries, driver symlinks, Vulkan ICD validation,
per-GPU sysfs links, host/direnv library-path merging without host secrets, final environment
ordering, broad-directory rejection, sibling worktrees and model paths containing spaces.
The existing real GitHub proxy/server/client and debug-redaction regression cases pass.

Final review confirmed GPU mounts follow home/worktree overlays, GPU variables follow
`--clearenv` and direnv, and the internal wrapper loader variable is not forwarded even
on default launches. Nonstandard library mounts require dedicated directories outside
sensitive host trees; broad `/run`, home and device mounts are rejected, including through
driver symlink targets. No display/audio/management device or new credential access is added.

## Resolved failures

The first strict Clippy run found an existing `expect("serialize")` in the GitHub proxy
client and the same `question_mark` URL-parser warning in both launchers. The first
standalone smoke-package build lacked the C linker. After reporting these failures, the
owner explicitly authorized fixes. The proxy now handles serialization failure, both
parsers use equivalent `?` control flow, and the smoke derivation includes Nix's
`stdenv.cc`. Final checks above pass. Markdown trailing-space findings were also cleaned up.

On 2026-11-12 the owner reported bubblewrap failing to create the symlinked
`/run/opengl-driver/share/vulkan/icd.d/asahi_icd.x86_64.json` destination. Real bubblewrap
reproduced the same error with an ordinary temporary symlink tree when a file below a
read-only directory was rebound. The launcher now omits redundant descendant binds,
including those already covered by `/nix`. The real-bubblewrap regression traverses and
reads the symlinked ICD successfully without GPU devices. A test-fixture invocation of
Nix's multicall coreutils initially failed; explicitly selecting its `cat` subcommand
resolved that fixture issue. Full workspace tests, strict Clippy, formatting and the Nix
package rebuild passed afterward. Logs: `/tmp/agentix-034-redundant-bind-repro.log` and
`/tmp/agentix-034-icd-regression.log`. This fixes the reproduced mount failure, while
the later physical acceptance results are recorded below.

## Physical acceptance: CUDA and headless Vulkan passed

After the owner restarted the jail on 2026-11-12, this session sees `/dev/nvidiactl`,
`/dev/nvidia-uvm`, `/dev/nvidia0`, `/dev/dri/renderD128` and `/run/opengl-driver`.
The driver reports NVIDIA 595.71.05; the GPU identified earlier is an RTX 5090 Laptop GPU.

`nix develop .#gpu-validation --command cargo run -p agentix-jails --example gpu_cuda_smoke`
ran successfully on actual hardware inside the jail and exited zero:

```text
PASS: NVRTC 13.1, compute_120, kernel result 42
```

This establishes NVRTC compilation, PTX loading through the host CUDA driver, GPU kernel
launch, synchronization, device-to-host copy and the checked result. It does not establish
full-model inference correctness. Raw output: `/tmp/agentix-034-physical-cuda.log`.

`nix develop .#gpu-validation --command vulkaninfo --summary` exited **139** with a
segmentation fault. Its output includes a skipped Mesa dzn ICD, absent DISPLAY/XDG runtime
directory, and `vkGetPhysicalDeviceDisplayPropertiesKHR failed with ERROR_UNKNOWN` while
querying `VK_KHR_display`. An NVIDIA-only ICD retry also failed in that display query.
The exact cause of vulkaninfo's crash was not established. All system ICD manifests are selected;
the NVIDIA manifest is `/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json`.
Raw output: `/tmp/agentix-034-physical-vulkan.log`.

The owner approved headless validation after the failure and confirmed the SSH/tmux
context. The new Rust `gpu-vulkan-smoke` tool enables no instance/device extensions or
layers and creates no surfaces; it selects a hardware compute queue, dispatches a
Nix-compiled shader and checks coherent output after barriers/fence completion.
With the jail's original ICD selection it exited zero:

```text
Headless Vulkan: NVIDIA GeForce RTX 5090 Laptop GPU, vendor 0x10de, compute queue 0
PASS: headless Vulkan compute on NVIDIA GeForce RTX 5090 Laptop GPU, shader result 42; no display extensions
```

Raw output: `/tmp/agentix-034-headless-vulkan.log`. Both Nix-built compute packages
then repeated their passing hardware results inside this same jail; raw output:
`/tmp/agentix-034-packaged-physical-compute.log`. This establishes compute access
without DISPLAY, XDG runtime directories, added device mounts or host setup changes.
It does not claim to repair vulkaninfo's display probing or establish full-model
inference correctness in feature 033.

`cargo tree --locked -p agentix-jails --edges normal --prefix none` confirms ash is
absent from ordinary launcher dependencies. It is enabled only for the optional smoke
binary. Cargo.lock adds exactly ash 0.38.0+1.3.281, reusing libloading 0.8.9; no existing
package versions changed. Shader compilation is pinned through Nix.

See [quickstart.md](quickstart.md) for the exact host restart command, CUDA smoke invocation,
headless Vulkan compute and current feature-033 CUDA/Vulkan/CPU reference commands. Feature
033's commands were verified against source only; its own build/model/runtime readiness
and full-model physical correctness remain separate acceptance requirements. MIG partitions,
graphics presentation, quotas and remote GPU access remain outside this profile.
