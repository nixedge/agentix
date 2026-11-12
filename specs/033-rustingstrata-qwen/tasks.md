# Tasks: RustingStrata Qwen replacement

- [x] T001 Inspect master, feature 021, upstream APIs/license, allocate and verify sibling worktree.
- [x] T002 Write specification, plan, requirements checklist and superseding decision before implementation.
- [x] T003 [US1] Add pinned RustingStrata dependency and network-free library wrapper with persistent model lifetime and reset/cancellation.
- [x] T004 [US2] Port tokenizer/protocol/tool handling and replace worker execution with library sessions.
- [x] T005 [US2] Wire gateway and NixOS Qwen service while preserving master behavior.
- [x] T006 [US1] Add Nix runtime/package integration, lockfile and attribution.
- [x] T007 [US1] Create small deterministic GGUF fixture and exercise actual library CPU execution/reset/context handling.
- [x] T008 [US2] Verify HTTP streaming/nonstreaming, tools, repeated requests, admission and cancellation.
- [x] T009 Run focused formatting/tests/strict Clippy, workspace tests and Nix package build; record evidence.
- [x] T010 [US3] Prepare reproducible target GPU correctness/benchmark commands.
- [x] T011 [US3] Run production GPU correctness/benchmark commands on the available RTX 5090 Laptop GPU.
- [x] T012 Record acceptance result and retirement decision based on physical GPU tests.

T009: focused checks, full-workspace tests, canonical Nix packages and NixOS
evaluation pass. Existing daemon all-target Clippy tests contain 25 unwrap_used
findings (binary Clippy passes). Physical CUDA acceptance passes on the available
RTX 5090 Laptop GPU; see results/2026-11-12/summary.md and decision.md. A separate
RTX 3090 deployment/same-host comparison is not claimed.
