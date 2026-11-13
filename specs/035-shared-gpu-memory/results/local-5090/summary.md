# Bounded local RTX 5090 validation

**Local core GPU acceptance passes**, including public Engine controls and an
exact checkpoint replay with production weights. This does not complete kursk's
three-worker deployment/calibration acceptance.

Owner authorized these bounded checks and the diagnostic reruns. RTX 5090 Laptop,
24,463 MiB VRAM, driver 595.71.05, about 62 GiB system RAM. Existing cached IQ3_S
split weights from feature 033 were reused. Production tests used context 128,
chunk 64, a **512 MiB expert-cache ceiling**, two Rayon/OpenMP threads, and an
optimized build. Guards: **90 seconds, 65°C, 8 GiB minimum available RAM**.
No benchmark loop, driver/service change, GPU reset or deployment was performed.
The owner's niri compositor was left running throughout.

## Production release and continuation: passed

| Check | Duration | Device VRAM returned | Peak sampled temperature | Peak sampled VRAM |
|---|---:|---:|---:|---:|
| Native checkpoint/replay | 7.95 s | 480 MiB | 47°C | 5,235 MiB |
| Public Engine release/refill | 10.08 s | 480 MiB | 48°C | 5,111 MiB |

The Engine test retains one session across release and refill. Expert allocations
change **530,073,600 bytes → 0 → 522,086,400 bytes**. Device free VRAM increases
by **503,316,480 bytes (480 MiB)** before continuation. Both subsequent decode
steps complete without a process restart or whole-model reload. See
[engine.log](engine.log) and [engine-resources.csv](engine-resources.csv).

The checkpoint test warms a decode graph, saves the pre-release recurrent state,
releases the cache, continues, refills and continues again without rewinding that
live path. Only afterward does it restore the original checkpoint for comparison
using the same CPU-expert arithmetic. Live and replayed logits match **exactly**
(relative L2 **0**, required below **1e-5**); position assertions also pass.
See [matched.log](matched.log) and [matched-resources.csv](matched-resources.csv).

This is a small, bounded context test, not calibration of a 4096-token context or
a throughput result. System MemAvailable stayed above **35.1 GiB** throughout all
attempts. No per-worker RSS attribution is claimed. Afterward device memory
returned to **61 MiB**, temperature 45°C and power about 16 W:
[final-after.txt](final-after.txt).

## Fixture: passed

The explicit CUDA test warms a decode graph, releases expert pools, continues
the same live session, refills the pools and continues again. All expected
greedy tokens match. Expert allocation changes **26,112 bytes → 0 → 26,112**.
Device free memory is unchanged, as expected for tiny pools sharing allocator
pages. This proves the exercised control/continuation path, not large VRAM yield.

GPU execution took 0.49 seconds; the surrounding build used one compiler job.
The external guard allowed 60 seconds and stopped at 65°C. Peak sampled GPU
temperature was **37°C**, with **323 MiB** peak sampled device usage.
See [fixture.log](fixture.log) and [fixture-resources.csv](fixture-resources.csv).

## Initial failures and diagnostic correction

The first unoptimized production attempt timed out at **90 seconds** (exit 124),
without reaching a result. Peak sampled temperature was 54°C. Its evidence is
preserved in [production.log](production.log) and
[production-resources.csv](production-resources.csv). The approved release build
took 3m03s with one compiler job; later optimized execution completes in seconds.

An initial optimized comparison returned 480 MiB and continued, but failed an
unvalidated **2% cross-placement logit threshold** at **2.74485%**; peak sampled
temperature was 62°C. See [production-optimized.log](production-optimized.log).
RustingStrata explicitly documents that CPU/GPU expert rounding can change
outputs (`src/expert_cache.rs`, description of `long`). A controlled checkpoint
comparison reproduces about **2.738%** between execution placements while
matched CPU-expert continuation has **zero** error. Thus the mismatched 2% bound
was replaced by the tighter matched-state check, not a looser parity threshold.
Cross-placement drift remains reported separately. Original failure logs remain.

After the timeout, utilization remained 100% despite baseline VRAM. The jail
could not see the host process; the owner identified niri in the host table.
That process was not stopped, and no reset was attempted. Its presence does not
establish attribution of all observed GPU activity. The approved optimized
attempt retained the same thermal guards; activity later returned near idle.
See [post-timeout-gpu.txt](post-timeout-gpu.txt).

## Additional checks and remaining scope

Hardware tests are ignored by default. Supplemental Qwen CPU checks pass:
**12 passed, 4 ignored**, strict all-target Clippy and formatting. Canonical Nix
Qwen library/worker builds pass again with these test-only additions;
paths are recorded in [nix-build-paths.txt](nix-build-paths.txt).
Production runtime code was unchanged by this validation work.

Kursk still needs calibrated model/context/batch profiles and a physical
embedding/Whisper/Qwen coexistence sequence. These local tests exercise the cache
release/refill and state path, not three real coordinated worker processes.
