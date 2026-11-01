# Feature Specification: Hey Agentix

**Branch**: `030-android-wake-word` · **Created**: 2026-11-01 · **Status**: implementation

The owner needs hands-free invocation after selecting Agentix as the Android default assistant. Confirmed wake phrase: **Hey Agentix**. Default-assistant selection alone does not supply a wake-word detector.

## User scenarios and testing

### US1 — Enable local hands-free invocation (P1)

As the phone owner, I explicitly enable Hey Agentix and invoke my assistant without a button.

- Given Agentix is default and microphone/notification permissions are granted, when I enable the listener from the visible app, then a persistent notification and microphone indicator disclose local listening.
- Given it is listening, when I say Hey Agentix and pause for the assistant screen, then keyword capture releases its microphone before one existing on-device command turn starts.
- Given airplane mode, detection and local transcription remain available with installed recognition data; backend commands still require backend connectivity.
- Given the phone is locked, detection may open the assistant, but commands require unlocking. Wake-word detection never bypasses authentication.

### US2 — Control listening and microphone contention (P1)

- Given the listener is active, when I disable it in setup or press Stop in its notification, then capture stops and its notification is removed.
- Given recognition, assistant speech or note recording, keyword capture pauses before competing capture starts, and resumes only when those holds have released.
- Given default-assistant removal, permission loss, audio failure or service destruction, capture is released and the UI describes stopped/unavailable rather than claiming active listening.
- Given reboot, force-stop or process death, the listener does not silently restart; opening setup exposes Start listening again. Preference and runtime status remain distinct.

### US3 — Understand the operating limits (P2)

The setup explains continuous software listening uses more battery than Google's privileged DSP integration. Only the wake phrase is recognized by this listener; audio is processed locally in memory, not saved or sent. Existing typed commands, button invocation, reminders, snooze, recording, voice profiles and explicit Gemini handoff remain available.

## Functional requirements

- FR-001: Default disabled; only an explicit visible Start action enables a microphone foreground service. Permissions/default-role are prerequisites, never silently granted.
- FR-002: Bundle reproducibly hash-pinned offline detector/runtime/model; no keys, downloads at runtime, cloud recognition fallback, hidden API reflection or DSP promises.
- FR-003: Detect Hey Agentix with bounded audio buffers and single-threaded inference off the main thread. Debounce/revalidate delayed detections and reject detections after Stop, role change or generation change.
- FR-004: Release capture before launching the protected default-assistant session. Do not auto-dispatch a command from keyword audio; wait for the visible existing recognition turn. Lock state gates command recognition.
- FR-005: Coordinate microphone ownership asynchronously; disabling/pausing cannot block the main thread or overlap keyword capture with command/note capture. Pause during visible assistant interaction to avoid TTS self-triggering.
- FR-006: Provide persistent truthful states (starting/listening/paused/stopped/error), notification Stop, explicit restart, and cleanup of native streams, recorders and bounded wake locks. No automatic background/boot restart.
- FR-007: Preserve backend pending identities, credentials, recordings, reminder/archive journals and existing notification channels. No server API or backend deployment change.

## Edge cases

Missing native ABI/model, unsupported microphone format, permission revocation, microphone privacy switch, phone call/other audio capture, unavailable default service, locked/screen-off phone, stale detection during pause, Stop during model initialization, repeated enable taps, recognition cancellation, concurrent holds, process teardown, voice-trigger false positives and no-network command failure.

## Success criteria

- SC-001: Canonical Nix APK build runs JVM policy tests and Android lint successfully; assets/native runtime are packaged and hash pinned.
- SC-002: On owner Pixel, at least 9/10 spoken wake phrases at normal conversational distance open exactly one assistant turn; record latency and actual conditions. Zero false triggers in a 15-minute ordinary speech sample. Do not claim these from unit tests.
- SC-003: Phone acceptance verifies screen-off/locked gating, Stop and permission denial, recording/recognition/TTS arbitration, Gemini handoff, and battery impact over a measured hour. Hardware results remain explicit pending evidence.
