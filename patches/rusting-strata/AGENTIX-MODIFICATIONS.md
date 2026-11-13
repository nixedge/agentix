# Agentix modifications to RustingStrata

Uses RustingStrata by Vasyl Trefilov. Original source revision:
`9dd6340d25c7c6770f7918134a27984cf20d7ce3` (1.2.1).
Original license and copyright are preserved in LICENSE.md. This modified
RustingStrata source remains under that same license.

Feature 035 adds explicit expert-cache allocation ceilings, synchronized
release of actual expert pool owners while retaining CPU weights and live
context, runtime cache refill, and captured CUDA/Vulkan graph invalidation
before changing buffer addresses. Other inference algorithms are unchanged.
CUDA release also trims unused async allocator pages after synchronizing both
streams, so freed pool owners can yield memory to other processes.
Agentix's own admission and service integration are separate library code.

This source copy is a Cargo path patch of the pinned dependency, included by
the existing Nix isolated-workspace patch mechanism for reproducible builds.
No edit of Cargo's shared Git cache is required or performed.
