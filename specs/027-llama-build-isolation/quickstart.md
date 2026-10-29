# Isolated workspace builds checkpoint

Every Cargo workspace member now has a Nix package with a computed dependency closure. Existing deployment packages, wrappers and aggregate checks remain available. `agentix-llama-cpu` provides a canonical CPU build; `agentix-llama` retains configured CUDA capabilities.

```bash
nix build .#workspace-locks
nix build .#agentix-api .#agentix-jails .#assistant-core .#agentix-llama-cpu
```

These commands passed on2026-10-29. The lock check validates all20 real members with locked/offline/all-features Cargo metadata. Assistant-core ran26 passing tests. API, jail binaries and native llama CPU builds succeeded. Nix syntax, formatting and whitespace checks pass. No fullworkspace/GPU tests or deployment were performed. Cache-invalidation mutation regression coverage and CUDA derivation evaluation are still open in tasks.md.

The root Cargo.lock remains authoritative. Nix selects reachable locked identities and manifests, runs Cargo normalization offline using only vendored pinned packages, rejects any retained package whose name/version/source/checksum differs, and then verifies locked resolution. Final compilation also uses `--locked`. Sources, dependency artifacts, relevant patches and native arguments are scoped per crate; no artifact chain through unrelated members is used.

Lock normalization intentionally uses import-from-derivation. Evaluation may build small source/vendor/normalization derivations, especially after manifest or lock changes. Verified normalized contents are detached and re-emitted so the preparation derivation's input address does not itself become a compiler cache dependency. Real compiler, relevant dependency, patch, CUDA target or native library changes still require rebuilds. The first build after this refactor also has new cache identities.

Kursk's exact old/new deployment derivations have not been compared. The original broad manifest/lock/native dependency coupling was verified locally; no claim is made that every observed deployment rebuild had that cause.
