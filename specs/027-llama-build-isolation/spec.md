# Feature Specification: Isolated workspace builds

**Feature Branch**: `027-llama-build-isolation`
**Created**: 2026-10-29
**Status**: Specified
**Input**: Avoid rebuilding llama on unrelated flake updates; isolate every workspace crate except for actual dependencies.

## User scenarios and testing

### US1 — Unrelated changes preserve caches (P1)

Given an existing per-crate build, when an unrelated workspace crate's source, manifest or exclusive lockfile dependency changes, its source and dependency derivations stay identical. This includes adding another unrelated workspace member. Independently test by comparing actual derivation paths across controlled fixture changes for every workspace member.

### US2 — Real dependency changes rebuild dependents (P1)

Given a shared path or registry dependency, changing its source or pinned dependency identity invalidates affected builds. Source-only changes invalidate final builds while preserving external dependency artifacts. Independently test both affected and unaffected members, including transitive paths, build/dev dependencies and target-specific dependencies.

### US3 — Reproducible normal builds (P1)

Existing public package/check names and service behavior remain compatible. All members have canonical isolated build attributes. Cargo uses pinned dependency versions/checksums offline; it cannot silently resolve fresh versions. Correct CPU/CUDA configuration, patches, build scripts and required native libraries remain effective. Compiler or relevant external dependency updates may still rebuild affected packages.

## Functional requirements

- FR-001: Compute workspace membership and transitive dependency closure from Cargo metadata/manifests and the committed lockfile rather than maintaining package-specific handwritten lists.
- FR-002: Each crate gets only relevant source/manifests, a scoped lockfile and relevant patches/configuration; unused workspace manifests and lock entries must not influence its derivations.
- FR-003: Dependency artifacts exclude application sources. Reuse stays within the same scoped dependency graph and feature/build configuration; no artifact chain through unrelated packages.
- FR-004: Native build inputs/environment reflect the package's dependencies; unrelated ONNX/audio/CUDA dependencies do not invalidate pure Rust packages.
- FR-005: Resolve Cargo dependency identities unambiguously, preserve versions/checksums/sources, include normal/build/dev/target edges, and reject unsupported or inconsistent metadata rather than guess.
- FR-006: Keep existing package wrappers, executable names, test selection, CUDA capabilities and runtime service interfaces compatible. Add isolated packages for library members that lack them.
- FR-007: Provide canonical Nix regression checks for cache stability and dependency sensitivity across all workspace members, plus Cargo locked/offline acceptance and representative package builds.
- FR-008: Document actual evidence and limitations. A flake revision bump alone must not force unrelated rebuilds; compiler, relevant native dependency or target changes legitimately do.

## Success criteria

Every workspace member passes cache-isolation regression checks. Unrelated source/manifest/lock additions do not change its derivations; relevant changes do. All isolated workspaces pass locked/offline Cargo validation and representative pure-Rust/native builds succeed. Existing deployment package names remain available. No production deployment or model behavior changes are part of this feature.

## Scope and edge cases

Transitive dependencies count as actual dependencies; reverse dependencies do not. Crate-level isolation does not split binaries inside one crate. Dev/target/optional dependency coverage is conservative and reproducible. Shared compiler/Nixpkgs changes remain valid rebuild triggers. Independently reproduce a broad cache dependency locally; Kursk's exact old/new derivations are not yet available, so do not claim its specific deployment cause is proven.
