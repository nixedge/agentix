# Research

## Host observations (2026-11-11)

The current jail exposes no `/dev/nvidia*`, `/dev/dri`, or `/run/opengl-driver`.
`/proc/driver/nvidia/version` reports open kernel module 595.71.05;
`/proc/driver/nvidia/gpus/0000:c5:00.0/information` identifies an RTX 5090 Laptop GPU, device minor 0.
Kernel information does not demonstrate usable hardware access from this session.

The visible capability files are `mig/config` (minor 1), `mig/monitor` (minor 2), and `fabric-imex-mgmt`.
These are management capabilities, not ordinary whole-GPU compute requirements. Do not mount the entire `nvidia-caps` directory or grant these controls. MIG partition access is outside this initial whole-GPU profile; document that limitation. Likewise omit modesetting, UVM tools/profiling and NVSwitch/Fabric administration devices.

NVIDIA documents ordinary GPU/control/UVM nodes and the distinct partition-access versus management capabilities in its [device-node guide](https://docs.nvidia.com/datacenter/tesla/mig-user-guide/620/device-nodes-and-capabilities.html). Whole-GPU compute uses individual `/dev/nvidiaN`, `/dev/nvidiactl`, `/dev/nvidia-uvm`; no display session is needed. DRM uses render nodes, excluding primary card/modesetting nodes. Physical compute remains required to validate the selected profile.

## cudarc and feature 033

Feature 033's current lockfile pins cudarc **0.19.10**. Agentic-nix fetched and indexed this exact crate; inspected `Cargo.toml`, `build.rs`, `src/lib.rs` and NVRTC loader search results.

- `build.rs::SUPPORTED_CUDA_VERSIONS` maps `cuda-13010` to **13.1** and sets `CUDA_MAJOR_VERSION=13`, `CUDA_MINOR_VERSION=1`.
- `src/lib.rs::get_lib_name_candidates` tries unversioned names, the selected major, and older compatibility names. NVRTC's loader uses these candidates. Loading an older library is not proof all selected symbols or generated PTX are supported.
- Therefore feature 033 should supply Nix-managed **CUDA 13.1 NVRTC and its matching builtins**, plus any runtime/cuBLAS libraries its selected features actually use. The default agentix CUDA 12 build shell is not established as sufficient for this integration. Avoid changing its existing llama build toolkit globally.
- The host driver supplies `libcuda.so.1`; toolkit driver stubs must never replace it at runtime. Use host driver directories first, retaining unrelated development library directories afterward.
- Driver/PTX compatibility must be established with NVRTC compilation, module load, kernel execution and result comparison, rather than `nvidia-smi` alone. CUDA [13.1 release notes](https://docs.nvidia.com/cuda/archive/13.1.0/cuda-toolkit-release-notes/index.html) provide the upstream compatibility context.

Source links: [cudarc 0.19.10 build.rs](https://docs.rs/crate/cudarc/0.19.10/source/build.rs), [library naming](https://docs.rs/crate/cudarc/0.19.10/source/src/lib.rs), [NVRTC loader](https://docs.rs/crate/cudarc/0.19.10/source/src/nvrtc/sys/mod.rs).

## Design decisions

Use a private host filesystem trait for deterministic discovery tests; production has one actual-host implementation. No environment-controlled fake device root.
Validate character-device type and read/write openability before binding. Directory enumeration is single-level and name-filtered, never recursive and never applied to `/nix/store`.
Use a Nix-provided Vulkan loader path through the package wrapper while the host supplies actual ICD/driver files. Preserve selected environment paths after final direnv application. No NVRTC binary vendoring or global CUDA prerequisite.

## 2026-11-12: symlinked ICD launch failure

The owner reported `bwrap: Can't create file at /run/opengl-driver/share/vulkan/icd.d/asahi_icd.x86_64.json: No such file or directory`.
The launcher mounted the driver directory read-only and then rebound the individual
ICD below it. A temporary symlinked ICD tree reproduced the same error using real
bubblewrap with the redundant bind. Omitting child mounts covered by a parent
directory fixes that reproduction without changing access. Keep ICD paths in the
Vulkan environment; only remove unnecessary bind operations. The new namespace
regression uses ordinary temporary files, not GPUs, and runs through the Nix shell.

## Headless physical acceptance

After the owner restarted with GPU access, CUDA NVRTC 13.1 compiled and executed
the smoke kernel (`compute_120`, checked result 42). `vulkaninfo --summary` crashed
in its `VK_KHR_display` property query both with all system ICDs and with just
the NVIDIA ICD. Display/presentation probes are unsuitable as a required test
for this SSH/tmux compute jail; no display permission expansion is needed.

The headless Rust smoke uses ash **0.38.0+1.3.281**, matching feature 033's pinned
version. The package indexer rejected its build-metadata version and a normalized
bare-version fetch did not exist; Cargo metadata for the current feature-033
checkout resolved the exact already-pinned source in the local registry. Inspected
`ash/src/{entry,device,util}.rs` directly for loader/instance lifetime, pipeline
creation, memory mapping, barriers, queue submission, fences and SPIR-V alignment.
No sibling source/lockfile was edited. The new optional dependency reuses the
workspace's libloading 0.8.9; other locked versions remain unchanged.

The smoke creates a Vulkan 1.0 instance/device with no extensions/layers and selects
a hardware compute queue. A four-byte coherent host-visible buffer starts at zero;
the shader writes 42. Host-to-shader and shader-to-host memory barriers order access,
and a completed fence precedes the checked host read. This follows the Khronos
[synchronization specification](https://docs.vulkan.org/spec/latest/chapters/synchronization.html).
Nix compiles the GLSL source with glslc for Vulkan 1.0 and embeds the resulting SPIR-V.
It ran successfully on the NVIDIA RTX 5090 Laptop GPU with the jail's original ICD
selection, without DISPLAY/XDG runtime directories or new mounts.
