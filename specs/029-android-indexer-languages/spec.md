# Feature 029: Android and Kotlin code indexing

**Status**: Implemented and validated locally; pending deployment
**Branch**: `029-android-indexer-languages` (previously reserved, now based on master)

## User stories and acceptance

### US1 — Find Android implementation (P1)
As a code-search user, I can index an Android repository with the ordinary code ingest command and find Kotlin and Java implementation without extra include patterns.

- Given `.kt`, `.kts`, and `.java` sources, when code ingestion completes, then their chunks retain repository-relative paths, source line ranges, and the `kotlin` or `java` language.
- Given an annotated Compose function or class member, when indexed, then symbol extraction retains the declaration and permits function/method filtering.
- Given an unchanged repository, when ingestion repeats, then previously indexed Android files do not trigger new embedding requests.

### US2 — Find Android configuration without generated noise (P2)
As a code-search user, I can find manifest, resources, and build configuration alongside implementation.

- Given Android manifest/resource XML, Groovy Gradle scripts, and properties files, when indexed, then their text is searchable with languages `xml`, `groovy`, and `properties` respectively. Kotlin Gradle scripts use `kotlin`.
- Given `.gradle`, `.kotlin`, `.idea`, `build`, and existing excluded directories, when walking a repository, then files inside those directories are excluded. Git ignore rules remain effective.
- Given a Kotlin script without extractable declarations, then bounded line chunking still indexes its content.

## Requirements and scope

- FR001: Discover `.kt` and `.kts` by default and reuse existing Kotlin symbol parsing.
- FR002: Discover `.xml`, `.gradle`, and `.properties` by default, using line chunks for these formats.
- FR003: Preserve Java support, existing language support, ignore behavior, and chunk limits.
- FR004: Verify ingestion, persisted metadata, and filtered full-text retrieval against actual LanceDB with a deterministic embedding HTTP fixture.
- FR005: Document re-ingesting existing repositories and filtering Kotlin searches. No Android APK update, schema migration, new parser dependency, server route, or deployment is part of this change.

## Success criteria

The regression fixture's supported source/configuration files all appear in persisted chunks and filtered keyword search; excluded files appear nowhere. A second unchanged run sends zero additional embedding requests. Existing indexer tests and the canonical Nix package build pass.

## Assumptions and edge cases

Android indexing includes source plus text configuration, not APKs, AARs, models, images, or generated outputs. Filename extension matching stays case insensitive. Java already works; Kotlin discovery is the confirmed defect. Existing forced Git revision ingestion is needed to refresh revisions already recorded complete.
