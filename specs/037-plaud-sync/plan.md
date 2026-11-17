# Implementation Plan: Plaud recording sync

**Branch**: `037-plaud-sync` | **Date**: 2026-11-17 | **Spec**: [spec.md](spec.md)

## Summary

Implement explicit foreground BLE sync from the owner's NB-100 into the existing Android Inbox. Pure Kotlin core owns restricted wire commands, strict protocol parsing, durable partial downloads and standard Ogg Opus packaging. Android owns serialized GATT, runtime permission/lifecycle handling, private binding settings and Compose controls. Existing WAV capture remains supported. The approved extension adds server storage and local transcription through existing assistant crates and the agent host; SHA256 reuses the pinned sha2 dependency and FFmpeg is Nix-provided.

## Technical Context

- Kotlin 2.2 / JDK17; Android API26+, Compose, existing coroutines dependency.
- Private files under `filesDir/plaud-partials` and `filesDir/recordings`; private preferences for device tokens.
- JUnit core tests and canonical Nix APK/lint build.
- Only protocol10's plaintext, mono 80-byte/20ms Opus profile initially supported. No claim of firmware-wide compatibility. Missing or unsupported evidence fails visibly.
- GATT operations serialized with finite deadlines; bounded notification channels; no callback disk work. Cancelling/leaving Devices closes connection and retains complete partial records.

## Constitution Check

Before and after design: pass. Existing native Android exception established by024; `:core` stays platform-free. Recording API types remain in assistant-core, durable files/metadata in assistant-sled, projection in assistant-lance, and network/worker assembly in agentix-agent. No cloud inference, third-party uploads, unrelated dependency upgrades, listener, database, credentials in artifacts or sandbox changes. Nix build is canonical; physical outcomes remain distinct. Run relevant assistant Rust tests and production Clippy/fmt, real storage/LanceDB/API coverage, canonical server packages and Android tests/lint.

## Structure

- `agentix-android/core/src/main/kotlin/us/disasm/agentix/core/plaud/`: protocol, sync engine, storage and Ogg container.
- `agentix-android/core/src/test/kotlin/us/disasm/agentix/core/plaud/`: packet/recovery/import integration tests.
- `agentix-android/app/src/main/kotlin/us/disasm/agentix/device/`: GATT transport, sync controller and device UI.
- Existing recording model/cards and Inbox updated to support Ogg Opus alongside WAV.
- `specs/037-plaud-sync/`: spec, research, contracts, model, tasks, quickstart and verification.

## Design decisions

Owner-requested remote diagnostics: add a read-only inspection mode to the existing GATT transport, ending after service discovery and standard Device Information (0x180A) reads. Read firmware (0x2A26), hardware (0x2A27), software (0x2A28), model (0x2A24) and manufacturer (0x2A29) sequentially. Match read callback UUIDs, bound values and deadlines, abort on timeout rather than issuing another overlapping read. No subscription, handshake or binding command in inspection mode. Preserve the bounded report on failure and expose Copy device report in Devices. Include app/Android build context and GATT UUID/property inventory; omit addresses/tokens/audio. Extend the existing spec rather than allocate another feature.

Download raw voice records with their session/offset headers retained in partials for checksum and resume validation. Enforce exact size, contiguous offsets, supported frame geometry and matching session. Build Ogg Opus to a temporary file, fsync and atomically rename to a deterministic device/session/size filename. Only published files enter Inbox. On retry, validate existing partial records before computing resume position. Existing completed files avoid transfer; stale temporary output can be rebuilt from the retained partial. Restrict send command constructors to the required read/download operations and checksum acknowledgement. No device deletion or binding-clear command is included in ordinary sync.

Existing binding: allow private token entry; authentication mismatch is actionable and never triggers a reset. A binding migration requires a separate concrete owner decision and hardware checkpoint; do not silently introduce CMD5.

## Risks and acceptance

Public protocol descriptions disagree on some transfer details and do not provide owner's firmware captures. Pin researched sources, use conservative validation, and keep hardware acceptance open. The first charged-device test may reveal a different layout requiring an explicit revision. ADB currently reports no attached devices. No software fixture can prove the owner's token, firmware, audio or BLE resume behavior.

## Binding setup refinement

Owner confirmed the existing official-app binding. Provide a separate **Use this Plaud with Agentix** confirmation, describing settings/binding loss and preserved recordings. Only after that on-device confirmation, probe handshake protocol10 and status1 before sending exactly `01 05 00 00`; persist the new token first. Ordinary sync still cannot reset/unbind. A post-command disconnect is reported as an uncertain switch outcome and the owner must explicitly try Sync next; no automatic reconnect or repeated clear. This makes setup possible without requiring token extraction from the vendor app. No binding command has been sent from this session.

## Approved server extension

Reuse `agentix-assistant-core` for typed recording metadata/receipts and `agentix-assistant-sled` for content-addressed file storage plus durable recording/transcription state in the existing assistant DB. Add no crate or listener. `agentix-agent` adds authenticated PUT/GET recording endpoints under `/v1/assistant/recordings`, routes upload bodies separately from16KiB instruction JSON, and owns a bounded local transcription worker. It decodes original Ogg through Nix-pinned ffmpeg into30-second16kHz WAV chunks (<1MiB), compatible with the current gateway/Whisper default body limits and Symphonia's lack of Opus decoding. Child processes are async, time-bounded and killed on cancellation. Whisper requests remain local and fail durably for retry; no cloud fallback.

Full transcript and immutable metadata live in Sled. Atomic completion creates deterministic UTF8-bounded Note chunks and projection jobs, reusing existing real LanceDB FTS projection/rebuild behavior. No promise of embeddings: the current personal projection is lexical. Original audio remains immutable and independent of transcription success.

Android adds a private durable upload journal and explicit opt-in, HTTPS raw-file PUT with bounded JSON receipts, immutable recording IDs/hashes, endpoint pinning while unresolved and per-file status in Inbox. Manual server sync can run without an awake recorder and resumes the same upload after a lost response. Backend Settings alone never upload audio.

Initial upload bound64MiB and4hours; full transcript bound2MiB. Blob/metadata conflict checks precede mutations. Nix wraps agentix-agent with the pinned ffmpeg path. Relevant crates, real storage/search integration, API tests, Clippy/fmt, server Nix build and Android canonical build are required after implementation.
