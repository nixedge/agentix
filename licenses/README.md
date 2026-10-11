# Upstream components

Agentix links unmodified OpenAI Codex PTY, file-search and execution-policy libraries at commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d` (Apache-2.0). See `codex-LICENSE` and `codex-NOTICE`. Its patch-core extraction has its own provenance and notices in `vendor/codex-apply-patch`.

Codex file-search links unmodified nucleo/nucleo-matcher at commit `4253de9faabb4e5c6d81d946a5e35a90f87347ee` (MPL-2.0). Corresponding source: https://github.com/helix-editor/nucleo/tree/4253de9faabb4e5c6d81d946a5e35a90f87347ee . See `nucleo-LICENSE`. No nucleo source modifications are made here.

Exact resolved Linux dependency versions and declared licenses are recorded in `specs/020-ax-coding-agent/upstream-dependencies.txt`; Cargo.lock remains authoritative. This records provenance, not a legal opinion.
