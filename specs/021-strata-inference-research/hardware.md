# Target host inventory

**Updated**: 2026-10-13
**Evidence**: User-provided target hardware identification; the agent-container probe below was read-only but is not the target. No target inspection or benchmarks performed.

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

The 2026-10-13 user-provided systemd journal identifies the deployment host as
`kursk` and confirms that `agentix-qwen.service` is installed and attempting to
start. Its failure is currently explained by the absent first IQ3_S shard at
`/var/lib/agentix/qwen/iq3s-00001.gguf`; the journal does not provide hardware,
memory, storage-capacity or driver inventory. The required IQ3_S download is
83,617,662,656 bytes across both shards, so available target storage must be
checked before fetching. This is artifact-size arithmetic, not evidence that
the target has sufficient free space.

T004 remains incomplete until these are recorded. Candidate memory budgets and performance measurements are pending; this inventory does not establish that a particular model/runtime configuration fits or meets parity goals.

### Follow-up container probe — 2026-10-13

A Nix-provided `pciutils` probe identifies PCI device `0000:c5:00.0` as
`NVIDIA GeForce RTX 5090 Max-Q / Mobile`; `/proc/driver/nvidia/gpus` identifies
the same device as `NVIDIA GeForce RTX 5090 Laptop GPU` with firmware/driver
`595.71.05`. `/proc/cpuinfo` reports AMD Ryzen AI 9 HX 370 w/ Radeon 890M and
24 logical processor entries. `/proc/meminfo` reported 64,928,800 kB total,
35,018,100 kB available, and 67,108,860 kB swap, all free. Available RAM
changed from the earlier sample, so it is a time-specific observation.
`nvidia-smi` is absent; the container omits `/sys/devices/system/cpu/possible`
and `/sys/dev/block`, so `lscpu` could not determine CPU topology and `lsblk`
could not enumerate storage. This probe still does not expose VRAM capacity or
free space, and remains evidence about the agent container only.

## Agent session environment (not the target)

On 2026-10-13, the agent container reported Linux kernel `6.18.46` on host
`iviron`, an AMD Ryzen AI 9 HX 370 (12 cores / 24 threads), and
`MemTotal=64,928,800 kB` (`MemAvailable` about 31.9 GB during the probe).
`/proc/driver/nvidia/gpus` identifies an NVIDIA GeForce RTX 5090 Laptop GPU
with driver `595.71.05`. Swap was 64 GiB, entirely free. The workspace volume
reported 1.5 TiB total and 551 GiB available. `nvidia-smi`, `lscpu`, and `free`
were unavailable, `/sys/class/drm` was not exposed, and the proc GPU record did
not include VRAM capacity or free memory. These observations do not match the
user-identified RTX 3090 / Ryzen 9 9950X / 128 GB target and cannot establish
the target's compatibility or memory budget. T004 remains open pending direct
target inspection.
