# Implementation plan: Isolated workspace builds

Branch027 created from025 f16e9dc after auditing all worktrees/branches/specs and known abandoned numbers through026. Verified host path `/home/sam/home/agentix/027-llama-build-isolation` immediately after creation.

## Evidence and approach

Current `perSystem/packages.nix` includes every manifest and complete Cargo.lock in all dependency sources and final sources. Native arguments include ONNX/OpenSSL/Clang/audio tools for every crate. Artifact chaining from store→assistant-lance→agent further couples caches. Existing partial source filtering therefore does not provide manifest/lock/native-input isolation.

Introduce a Nix workspace-scoping module. Read the committed root/member manifests and lockfile at evaluation time; derive each selected root's transitive locked dependency graph, identify local member/patch paths, and emit stable projected workspace manifests and lock data. Include selected member files only; emit a manifest-only source for Crane dummy/dependency generation without application files. Preserve root workspace inheritance/lints/resolver/profiles and relevant patches. Explicitly pass lock/manifests to Crane to avoid full-source context leaking into dependency hashes. Validate projections with Cargo locked/offline resolution; do not permit network resolution or lock updates in canonical package builds.

Refactor all package/check definitions to use common per-root helpers. Native build profiles are chosen from reachable native dependencies; CUDA only applies to llama and retains configured architecture list. Remove unrelated artifact chaining. Publish individual packages for all20 members while retaining existing aggregate checks and executable wrappers. Do not split or rename Rust crates.

Regression checks exercise the actual helper against mutated synthetic workspace inputs: unrelated source/manifest/lock/member changes, direct/transitive changes, source-only dependency-cache preservation, same-name different-version/source resolution, dev/target/patch closure and native-input scope. Validate all real-member projections with Cargo offline/locked, and build representative pure Rust, jail and llama CPU artifacts plus evaluate CUDA derivations. Full GPU/workspace execution is separately reported.

## Constitution check

Owner approved offline normalization after the initial projected lock failed Cargo's locked check. Normalize using only the scoped vendor set, verify every retained name/version/source/checksum against committed pins, then verify locked/offline resolution again. Read the verified normalized lock at evaluation and regenerate its detached content so unrelated workspace feature edges affect only inexpensive normalization, not compiled artifacts. This introduces deliberate import-from-derivation for lock preparation; preserve hermetic offline behavior and document the evaluation cost.

Nix-only build graph changes preserve Rust library boundaries, routing, storage and runtime state machines. No provider/model/network/service/credential changes. Canonical Nix packages/checks and focused regression tests cover this high-impact cache behavior. No Python automation or extra daemon. Native/compiler changes continue to invalidate affected outputs; do not hide real dependencies with unsafe hash overrides.
