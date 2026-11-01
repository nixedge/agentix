# Feature Specification: agentix-android foundation

**Feature Branch**: `024-agentix-android`

**Created**: 2026-10-28

**Status**: Implemented foundation, phone recording and Android default assistant; physical-device acceptance pending
**Input**: Create a Kotlin Android app named agentix-android, built with Nix, as an extensible client for the Agentix AI assistant. First device integration is the user's existing Plaud Note NB-100; recordings will eventually reach the user's server through the phone's existing WireGuard tunnel.

## User Scenarios & Testing

### User Story 1 — Install the assistant client (P1)

The owner builds and installs agentix-android without a host Android SDK, Java, or Gradle installation and can navigate between Inbox, Devices, and Settings offline.

**Independent test**: Build an APK through Nix; install it and open all three destinations in airplane mode.

**Acceptance scenarios**:
1. Given only Nix and the checkout, when the documented build runs, then it produces an installable debug APK.
2. Given no configured server or connected recorder, when the app opens, then it presents honest empty states without fabricated tasks, recordings, or connection status.

### User Story 2 — Discover an existing NB-100 (P1)

The owner can explicitly scan for nearby Plaud devices, grant Android permissions, and inspect advertised identity without changing the recorder's binding or recordings.

**Independent test**: Exercise advertisement parsing with fixtures; on a physical phone scan the owner's charged recorder.

**Acceptance scenarios**:
1. Given permissions and enabled Bluetooth, when Scan is selected, then nearby matching recorder advertisements appear without initiating a pairing/reset operation.
2. Given denied permission or disabled/absent Bluetooth, when Scan is selected, then the app explains the condition and offers a retry.
3. Given an unknown or truncated advertisement, when discovered, then the app does not assume protocol compatibility or crash.

### User Story 3 — Configure a private assistant endpoint (P2)

The owner saves a server URL for future assistant integrations. Configuration survives application restart and works independently of recorder discovery.

**Independent test**: Validate URL acceptance/rejection automatically; save a URL, restart the installed app, and verify its value.

**Acceptance scenarios**:
1. Given an HTTPS server URL, when saved, then it persists locally and remains editable.
2. Given an invalid URL, embedded credentials, or plaintext HTTP URL, when saved, then validation rejects it with a useful explanation.
3. Given a saved URL, when viewing the app, then it does not claim a server connection has been established.

## Requirements

- **FR-001**: Name the app and Gradle root project `agentix-android`; use native Kotlin UI.
- **FR-002**: Keep recorder protocol parsing independent of Android and UI code, so assistant features can be added without depending on Plaud.
- **FR-003**: Offer Inbox, Devices, and Settings destinations with truthful status.
- **FR-004**: Discovery must be user initiated, bounded in time, permission aware, and non-mutating with respect to the recorder.
- **FR-005**: Never reset, unbind, delete, update firmware, or automatically connect to the recorder in this foundation.
- **FR-006**: Persist endpoint configuration and explicitly captured phone audio privately; disable Android backup of application data. Do not embed provider keys, server credentials, or a fabricated upload API.
- **FR-007**: Provide a pinned Nix Android development shell and a Nix package producing an APK; capture Gradle dependencies for the sandboxed build.
- **FR-008**: Include meaningful tests for malformed advertisements and endpoint validation, and run Android lint and APK assembly.
- **FR-009**: Clearly distinguish automated build/test evidence from physical-device verification.

### Scope extension — direct phone capture (2026-10-28)

The user also requested phone microphone recording and possible Hey Google integration, and confirmed Gemini is the phone's assistant.

### User Story 4 — Capture a voice note without Plaud (P1)

The owner records a short note with the phone microphone, keeps recording with the screen off, stops from the app or notification, and finds the saved audio in the Inbox for playback or explicit sharing.

**Independent test**: Record and play a note on a physical phone; verify WAV headers and interrupted-file recovery with JVM tests.

**Acceptance scenarios**:
1. Given microphone permission, when Record is tapped in the visible app, then recording starts with an ongoing notification and stop action.
2. Given active recording, when the screen is turned off or another app is opened, then capture continues through a microphone foreground service.
3. Given a stopped recording, when reopening the Inbox, then local audio remains available for playback/sharing without a server connection.
4. Given a process interruption, when restarting, then complete captured PCM frames in a partial file are recovered into a playable WAV; empty files are discarded and unrecoverable files are retained with an error.

### User Story 5 — Open capture from a shortcut (P2)

The owner opens the recording screen from a launcher shortcut or `agentix://record`. Google's optional App Actions metadata maps the recording feature to the same screen; voice invocation remains device/account dependent.

**Acceptance scenarios**:
1. Given an installed app, when Record a note is selected in the launcher shortcut menu or its deep link is opened, then the recording screen appears.
2. Given an external invocation, when the screen appears, then it waits for the owner to start capture; it does not silently acquire the microphone.
3. Given Gemini, when assessing voice support, then documentation distinguishes supported app opening from unverified custom shortcut invocation and does not promise a working Hey Google phrase.

- **FR-010**: Capture 16 kHz, mono, 16-bit PCM WAV in app-private storage, independently of Plaud and backend configuration.
- **FR-011**: Keep audio I/O off the UI thread; explicitly stop/release microphone resources; persist valid audio on clean stop and preserve recoverable PCM after interruption.
- **FR-012**: Provide a microphone foreground service, required runtime permissions, visible recording state, notification Stop action, and no automatic restart after process death.
- **FR-013**: Provide local playback and user-initiated sharing via content URIs; do not automatically upload recordings.
- **FR-014**: Add a recording shortcut/deep link and optional OPEN_APP_FEATURE metadata; do not require Google services for recording or add a hotword listener.
- **SC-005**: WAV-header and interrupted-file recovery fixtures pass; the APK includes the declared microphone service and recording shortcut.
- **SC-006**: Physical screen-off recording, stop, playback, sharing, and Gemini behavior remain explicit hardware acceptance checks.

## Edge Cases

- No BLE hardware, permission refusal, location disabled on older Android, Bluetooth switched off, or the official Plaud app already holding a connection.
- Duplicate scan results and advertisements with missing, short, or unexpected manufacturer data.
- Process recreation and rotation must not create an unbounded scan or erase the saved endpoint.
- A WireGuard tunnel may be unavailable; a saved URL is configuration, not connectivity evidence.

## Key Entities

- **Discovered recorder**: address, advertised name, signal strength, and optional manufacturer project code. A project code is not proof of a hardware model or supported protocol version.
- **Assistant endpoint**: validated HTTPS base URL stored locally, with no implied server API contract.

## Success Criteria

- **SC-001**: `nix build .#agentix-android` produces an APK and runs unit tests and Android lint without host development tools.
- **SC-002**: Truncated manufacturer payloads and invalid endpoint cases pass automated tests.
- **SC-003**: Installed app opens offline and retains saved settings after restart (physical validation recorded separately).
- **SC-004**: A scan stops within 15 seconds or on leaving the Devices screen, and never issues a recorder write command.

## Assumptions and Scope

- This implementation establishes the buildable client, read-only discovery, and direct phone recording. Full BLE authentication, resumable downloads, Opus packaging, durable upload jobs, task extraction, reminders, and assistant chat are subsequent increments, not simulated capabilities.
- Android 8.0/API 26 or newer; personal sideloading first. Release signing and Play Store publication are out of scope.
- WireGuard is managed by the existing phone app. Agentix uses ordinary network requests when an actual server integration is added.
- NB-100 firmware is currently unknown because the charging cable is missing. No claim of compatibility with the owner's unit can be made until tested.

## Android default assistant extension (2026-10-28)

User authorizes Android-only implementation of the custom assistant. Backend execution, MCP transport, model routing, and wake-word engines remain separate increments. Gemini cannot currently invoke the owner's custom tools from the phone; previous optional App Actions metadata does not establish that capability.

### User Story 6 — Invoke Agentix as the phone assistant (P1)

Install the APK locally, select Agentix as the default digital assistant, and invoke it using the phone's configured assistant gesture/button. The same assistant screen is accessible from the app without changing the default assistant.

Acceptance:
1. The APK declares a system-bindable VoiceInteractionService and protected session service with valid metadata; no Play publishing or root is needed.
2. A system assistant invocation opens the visible assistant activity. After microphone permission is already granted and the device is unlocked, this explicit invocation begins one recognition turn. Opening from the app waits for Talk.
3. First use requests microphone permission; denial gives a usable retry/settings path. Granting permission requires a subsequent Talk action. Locked-device launches require unlocking and do not record or speak private content.

### User Story 7 — Dictate an instruction and hear an acknowledgement (P1)

Use native on-device transcription, review the final text, and hear a truthful local acknowledgement. The instruction remains available as a typed `tell(text)` request at the Android integration boundary; no backend request or task completion is fabricated.

Acceptance:
1. On API 31+ with an on-device recognizer and installed language, Talk shows listening/processing state, partial text, and then a final instruction. No silent fallback to cloud recognition.
2. Without a compatible recognizer/language, explain the condition and allow typed instructions. Recognition ends on silence or Stop; Cancel discards in-flight results.
3. An instruction produces an explicit backend-not-connected response, displayed and optionally spoken using an offline TTS voice. No provider API keys or billing are required for this local preview.
4. Leaving the activity, locking the phone, or cancelling stops microphone/TTS and rejects delayed callbacks. Speech is not captured over an existing phone recording; neither transcript nor assistant audio is written to disk or logs.
5. Empty instructions cannot be submitted. Users can edit/retry, toggle spoken responses, and stop speech.

- **FR-016**: Implement default-assistant selection guidance and a visible standalone assistant UI.
- **FR-017**: Keep Android recognition, assistant lifecycle, and TTS adapters separate from Android-free command/turn state and the future backend boundary.
- **FR-018**: Use on-device recognition only, surface missing language/model and permission errors, and clean up resources on cancellation/background/destruction.
- **FR-019**: Disallow overlapping note recording/assistant capture. Invalidate asynchronous results after cancel/new turn/background. Bound recognition waits.
- **FR-020**: Keep assistant data ephemeral, avoid collecting foreground app screenshots/context, and never claim system shell privileges or working wake-word detection.

Success: canonical Nix APK build, core lifecycle/command tests, Android lint, and manifest inspection pass. Physical Pixel assistant-role activation, microphone/TTS/language behavior, lock/background cancellation, and recording contention are documented as pending until a device is attached.

## Owner refinement — speech, navigation and Gemini handoff (2026-11-01)

The owner clarified Jarvis-like means **spoken voice: British and measured**, not rewritten replies. Add a British/measured speech preset with a modestly slower rate/lower pitch, an installed offline voice picker and explicit Preview. Prefer an installed offline British English voice, rank quality deterministically, and show honest fallback/missing-voice status. Voice labels/preview must not invent a gender or claim a character/actor voice. Preserve local speech privacy, stop on focus loss/pause, and save voice/preset preferences without altering reply text, request identities or backend behavior.

Reduce Inbox navigation clutter: retain Inbox/Archive/Record navigation, move Agentix assistant/Settings/Devices into a top-right three-dot menu. Place Open assistant directly above the Inbox feed, opening the existing text instruction box and Talk controls. Preserve role/setup access and notification/record deep links.

Explicit user input `ask Gemini QUESTION` or `Agentix, ask Gemini QUESTION` must hand off only that question to Gemini, leaving conversation continuation in Gemini. Match anchored command syntax, preserve question content and reject an empty/oversized question. Do not route ordinary mentions, saved-note instructions or other requests to Gemini. Keep this separate from backend tell/pending request identity: no implicit cloud fallback, no forwarded records/history, no logging/persistence of delegated questions, no provider keys or Gemini API use. Use registered Android text-sharing/PROCESS_TEXT handlers restricted to installed Gemini/Google components identified as Gemini. If unavailable, retain the draft and explain failure without claiming delivery. Opening an app does not prove a response or completed conversation; physical prompt acceptance is required. Stop microphone/TTS before leaving the assistant.
