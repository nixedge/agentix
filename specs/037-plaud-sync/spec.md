# Feature Specification: Plaud recording sync

**Feature Branch**: `037-plaud-sync`
**Created**: 2026-11-17
**Status**: Implemented; physical and deployment acceptance pending
**Input**: “ok, lets make sure the sync is implemented” after charging the owner's NB-100. Owner confirms prior pairing with the official Plaud app.

## User Scenarios & Testing

### US1 — Connect the owner's recorder (P1)
The owner scans, selects a recorder, and explicitly connects with a persistent private binding token. Device-reported protocol evidence determines compatibility, not an advertisement name.

1. Given a supported recorder and matching token, when Connect is selected, then authenticated connection and protocol version are shown.
2. Given an existing official-app binding, when authentication rejects Agentix's token, then the app explains the mismatch; it never automatically clears binding or settings.
3. Given an unsupported protocol, recording-in-progress, missing permissions or a failed connection, when connecting, then show a useful error and close resources.

### US2 — Download recordings into Inbox (P1)
The owner explicitly syncs available recordings into private phone storage, plays them, and shares them using existing Inbox behavior.

1. Given a supported authenticated device, when Sync recordings is selected, then list sessions and download completed recordings sequentially, displaying progress.
2. Given a recording already imported for this device/session/size, when syncing again, then no duplicate Inbox entry or repeated transfer is created.
3. Given an interrupted transfer or process death, when syncing again, then retain partial bytes, resume from the validated contiguous offset, and publish only complete audio.
4. Given a malformed packet, wrong session, invalid offset/length, incomplete listing, mismatched checksum or disk error, when syncing, then do not mark the file complete.
5. Given completed sync, when opening Inbox, then each imported recording has its source, date, duration, playback, sharing and archive actions.

### US3 — Manage an existing binding deliberately (P2)
The owner can supply an existing binding token privately. Any optional switch from the official app must be a separate explicit action with a confirmation describing loss of the previous binding and settings; it never requests recording erasure.

## Requirements

- FR-001: Native Kotlin Android integration; protocol, transfer persistence and audio-container logic remain Android-free in `:core`.
- FR-002: Private persistent per-device token; never log tokens/audio, include them in Git/Nix outputs, or send to third-party cloud services.
- FR-003: Bounded asynchronous GATT operations, callbacks isolated by connection identity, serialized writes, cancellation, timeouts and deterministic cleanup.
- FR-004: Only supported, observed protocol variants may transfer. Unknown variants fail visibly without interpreting arbitrary bytes as recordings.
- FR-005: Validate complete listing and each transfer's identity, bounds, contiguous bytes, completion and available checksum before atomic publication; retain recoverable partial data.
- FR-006: Download is idempotent across retries/restarts and isolated by device identity plus session and size. No remote deletion, firmware commands, time/settings writes or automatic unbinding.
- FR-007: Imports are playable standard audio and work with existing Inbox/Archive/play/share; phone WAV capture remains intact.
- FR-008: Manual sync starts from visible Devices; leaving the app/screen cancels safely and retains partials. No unattended background scan/download claim.
- FR-009: User-visible status distinguishes discovery, connection, authentication, transfer, local import and failure. Local sync does not claim server upload/transcription.
- FR-010: Nix APK build runs meaningful protocol/storage/audio JVM tests and Android lint. Physical Pixel/NB-100 acceptance is a separate required checkpoint.

## Entities

- Recorder identity: BLE address, private token, protocol evidence.
- Remote recording: session ID, raw byte count, scene.
- Partial download: device/session/size identity, contiguous raw audio bytes.
- Imported note: atomic standard audio file, deterministic identity and recording timestamp.

## Success Criteria

- SC-001: A supported recorder's short test note reaches Inbox and plays after explicit sync on the owner's Pixel.
- SC-002: Repeating sync and restarting the app yield exactly one playable import per recording identity.
- SC-003: Disconnect/cancel/process-death recovery preserves a valid partial, resumes without gaps and cannot publish corrupt/incomplete audio.
- SC-004: Automated tests reject malformed/wrong-session packets, demonstrate resumability/idempotency, and validate the generated audio container independently of packet encoding.
- SC-005: Canonical `nix build .#agentix-android` passes; device-only outcomes are not inferred from fixture tests.

## Assumptions and scope

The approved sync target includes the Android Inbox and Agentix server, with the explicit recording API contract below. Task extraction or execution remains outside this increment. Bluetooth-only manual sync is the initial path; WireGuard/backend configuration is not required for local imports. The owner's firmware is unknown until a physical connection. No physical device access is assumed from the current session.

## Approved scope extension — phone-to-server sync

Owner selected **Include phone-to-server sync** after reviewing the proposed file/Sled/LanceDB design.

### US4 — Store phone imports on the Agentix server (P1)

1. Given explicit recording-upload opt-in and the existing enabled HTTPS backend, when Sync to server runs, then each completed Plaud import uploads immutable audio and metadata, with separate per-file phone/server status.
2. Given a lost response, restart or repeated sync, when retrying, then reuse the same recording identity and original destination; the server returns the original durable receipt without duplicates.
3. Given authentication, checksum, source identity conflict, size limit or disk failure, then keep local audio and unresolved upload state and do not claim server storage.
4. Given a stored recording, when local transcription is available, then a durable retry worker transcribes bounded audio chunks, preserves the transcript and atomically queues searchable Note projections in the existing LanceDB pipeline. A transcription failure never loses the original audio or marks it ready.
5. Given daemon restart or a failed projection, then retry durable work without creating duplicate transcript notes.

- FR-011: Content-addressed original Ogg audio on server disk; immutable Sled recording metadata and server identity; full transcripts in Sled and derived transcript chunks in existing LanceDB personal search. No cloud transcription.
- FR-012: Authenticated existing assistant prefix, bounded requests/responses, server-verified SHA256 and Ogg duration/profile, atomic blob publication followed by durable metadata; source-device/session conflicts return409.
- FR-013: Explicit phone upload opt-in; retries bind immutable file/hash/ID to the original endpoint, no redirect or silent destination change. Local recordings are retained after upload.
- FR-014: Upload receipt distinguishes durable server audio from pending/retrying/completed transcription and search projection. Server files+Sled must be backed up together; LanceDB is rebuildable.
- SC-006: Real Sled/filesystem integration tests verify replay/conflicts, corruption, restart, atomic metadata/transcript/projection work. Real LanceDB test verifies transcript search.
- SC-007: Canonical server/client Nix packages and relevant Rust tests/fmt/Clippy pass. Deployment, real server connectivity and physical recorder acceptance remain explicit checkpoints.

These approved requirements supersede the earlier assumption excluding server upload/transcription from this increment.
