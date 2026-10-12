# Target host inventory

**Updated**: 2026-10-12
**Evidence**: User-provided hardware identification; no host inspection or benchmarks performed.

| Component | Confirmed specification |
|---|---|
| GPU | NVIDIA GeForce RTX 3090 |
| GPU memory | 24 GB VRAM |
| CPU | AMD Ryzen 9 9950X 16-Core Processor |
| System memory | 128 GB RAM |

## Research implications

Evaluate the independent agentix Qwen backend's NVIDIA/CUDA path, CPU participation and host-to-device transfers on this exact combination. Strata is an optional external benchmark only. Do not transfer upstream results from other hosts or infer throughput from memory capacity.

## Remaining inventory

- Accessible host identity and operating system.
- NVIDIA driver, supported CUDA runtime, and pinned build environment.
- Actual total/free RAM and VRAM in bytes, including service/display reservations.
- RAM configuration and measured bandwidth; active PCIe link width and speed.
- Storage type, free space, and model location.
- Competing inference, embedding, voice, and other workloads.
- Power/thermal settings and swap configuration relevant to reproducibility.

T004 remains incomplete until these are recorded. Candidate memory budgets and performance measurements are pending; this inventory does not establish that a particular model/runtime configuration fits or meets parity goals.
