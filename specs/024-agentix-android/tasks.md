# Tasks: agentix-android foundation

## Preparation

- [x] T001 Allocate and verify sibling worktree 024 after inspecting existing branches, worktrees, and specs.
- [x] T002 Write specification, requirements checklist, plan, research, and data model.

## US1 — Build and install the client

- [x] T003 Create pinned Gradle Kotlin/Compose project and independent `:core` module under `agentix-android/`.
- [x] T004 Add `perSystem/android.nix`, SDK/JDK/Gradle dev shell, and APK derivation in `agentix-android/nix/default.nix`.
- [x] T005 Implement Inbox, Devices, and Settings navigation and honest empty states.

## US2 — Non-mutating recorder discovery

- [x] T006 Implement manufacturer advertisement parser and malformed-payload tests in `core/`.
- [x] T007 Implement bounded, permission-aware Android BLE scan and lifecycle cleanup in `app/`.
- [ ] T008 Verify discovery on a physical phone with the user's charged NB-100; record firmware and protocol version before implementing authentication. Requires hardware; do not mark complete from fixtures.

## US3 — Private assistant configuration

- [x] T009 Implement HTTPS endpoint validation and boundary tests in `core/`.
- [x] T010 Implement persisted endpoint configuration and backup exclusion in `app/`.
- [ ] T011 Verify offline navigation, rotation/background scan cleanup, and settings persistence on a phone. Requires a connected device/emulator.

## Build validation and handoff

- [x] T012 Generate and pin `nix/deps.json` for Gradle dependencies.
- [x] T013 Run core tests, Android lint, and APK assembly using `nix build .#agentix-android`; inspect APK identity (initial foundation; extension revalidation is T020).
- [x] T014 Document Nix commands, debug signing, extension boundaries, build evidence, and pending hardware tests in README/quickstart/AGENTS.

## Subsequent increments (not foundation acceptance)

Define separate spec/tasks for protocol-version inspection, owner-approved binding migration, resumable recording downloads with checksums, Opus/Ogg packaging, durable uploads and authentication to the selected backend API, then task extraction/reminders and assistant chat. No placeholder “Sync” action may pretend these work.

## User-requested extension — phone recording and Gemini entry points

- [x] T015 Extend spec/plan/research for microphone capture and confirmed Gemini usage before implementation.
- [x] T016 Add tested WAV encoding and partial-file recovery in `core/.../audio/`.
- [x] T017 Add private recording store and microphone foreground service with notification Stop, resource cleanup, and no automatic restart.
- [x] T018 Add Record UI, local Inbox playback/sharing, runtime permissions, and explicit error states.
- [x] T019 Add static recording shortcut, deep-link handling, and optional OPEN_APP_FEATURE metadata; document Gemini limitations.
- [x] T020 Refresh Gradle dependency snapshot and rerun sandboxed Nix build, unit tests, lint, and APK manifest inspection for the extended scope.
- [ ] T021 Verify screen-off recording, recovery, playback/sharing, permissions, shortcuts, and Gemini launching on the owner's phone. Hardware dependent.

## User-requested extension — Android default voice assistant

- [x] T022 Extend spec/plan/data model/research for Android-only assistant scope before implementation.
- [x] T023 Implement tested Android-free tell command boundary and cancellable turn state.
- [x] T024 Register VoiceInteractionService/session metadata and non-exported assistant activity; add default assistant settings/navigation.
- [x] T025 Implement native on-device recognition, permission/language/error UI, cancellation/timeouts and recording contention guard.
- [x] T026 Implement offline TTS, reply preference and honest disconnected preview; keep voice data ephemeral.
- [x] T027 Build through Nix, run lifecycle/command JVM tests and lint; inspect packaged manifest; document installation and Pixel acceptance.
- [ ] T028 Test default role, configured system gesture, language availability, microphone denial, lock/background/rotation cleanup, cancellation races, offline TTS and recording contention on the Pixel 10. Requires an attached physical phone.
