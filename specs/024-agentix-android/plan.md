# Implementation Plan: agentix-android foundation

**Branch**: `024-agentix-android` | **Date**: 2026-10-28 | **Spec**: [spec.md](spec.md)

## Summary

Add `agentix-android/` to the Agentix monorepo: a Kotlin/Compose application with an offline assistant shell, private endpoint configuration, read-only Plaud discovery, and direct phone recording. Separate Android-free domain/protocol/audio-file logic into `:core`. Do not invent backend endpoints or ship destructive device commands.

## Technical Context

- Kotlin 2.2.0; Compose compiler uses the same Kotlin version.
- Android Gradle Plugin 8.11.1; Nix-pinned Gradle 8.14.4 and JDK 17.
- Android compile/target API 35, build tools 35.0.0, minimum API 26.
- Compose BOM 2025.05.01; Activity Compose 1.10.1. No dependency injection framework or native Rust bridge.
- Storage: Android private SharedPreferences for one endpoint value, and private WAV files for phone recordings. No database in this increment; durable upload-job storage belongs with the later backend integration.
- Tests: JUnit 4 in `:core`; Android lint for platform integration; APK assembly through Nix. Physical phone and recorder tests remain pending.
- Scale: one personal Android client, three destinations, one optional device family.

## Constitution Check

| Principle | Application |
|---|---|
| Library first / layered architecture | Android-independent logic is in `:core`; UI and Android transport are in `:app`. No daemon internals or inference implementation on the phone. |
| Local-first intelligence | No provider calls; future requests go to the configured Agentix backend. |
| Reproducible environments | Dedicated `devShells.android`, pinned SDK/JDK/Gradle, Gradle dependency snapshot, canonical `packages.agentix-android`. |
| Isolation | No changes to jail mounts, credentials, or execution policy. |
| Testing / code quality | Core unit tests, Android lint, APK build. Rust files are unchanged; workspace Cargo gates remain pre-merge requirements. |
| Agent state machine | No agent loop added. Scan lifecycle is explicit and bounded. |

## Project Structure

```text
agentix-android/
  settings.gradle.kts
  build.gradle.kts
  gradle.properties
  core/src/{main,test}/kotlin/us/disasm/agentix/core/
  app/src/main/{AndroidManifest.xml,kotlin/us/disasm/agentix/,res/}
  nix/{default.nix,deps.json}
  README.md
  AGENTS.md
perSystem/android.nix
specs/024-agentix-android/
```

## Design

The application has Inbox, Record, Devices, and Settings tabs. Inbox lists real phone recordings or an honest empty state. Devices requests the Android-version-appropriate permissions, scans at most 15 seconds, deduplicates by Bluetooth address, and stops when its screen leaves the foreground. Discovery does not connect or issue GATT writes. Project code is informational and does not establish firmware compatibility.

Settings validates an HTTPS URI without embedded credentials, query, or fragment and saves it privately. It reports that connectivity has not yet been checked. No default private host, authentication flow, or server API is assumed.

The Nix module composes only the necessary Android SDK components, selects JDK 17, patches the AAPT2 path to Nix's executable, and provides a separate lightweight Android shell. The APK derivation uses nixpkgs's Gradle MITM dependency snapshot, so sandboxed builds do not depend on a developer's Gradle cache. Build output is a debug APK for sideloading, not a release-signed artifact.

## Complexity Tracking

| Exception | Justification |
|---|---|
| Native Kotlin deliverable rather than a Rust crate | Explicit user choice for Android. Rust-only library requirements apply to existing inference/routing/agent mechanics; these remain on the server. Kotlin modules preserve the intended separation without JNI overhead. |

Design review: the app opens no listening port and implements no inference, cloud routing, agent execution, or recorder mutation. No constitution amendment required.

## Direct phone recording extension

User-requested extension: add a Record destination, AudioRecord-based microphone foreground service, and app-private WAV storage. Use 16 kHz mono PCM for speech and simple recovery: reserve a 44-byte WAV header, append PCM to `.wav.part`, and write the final header/rename on stop. On restart, recover complete PCM frames in orphaned partial files; never recover a currently active file. Audio work and directory operations run off the main thread. No automatic service restart after process death.

Keep WAV header encoding and file recovery in `:core`, test with real temporary files on the JVM. Android service/notification, MediaPlayer playback, FileProvider sharing, and permission UI live under `app/.../recording/`. Explicit dependencies: kotlinx-coroutines-android 1.10.2 and androidx.core:core 1.16.0; refresh the Nix dependency snapshot.

Expose `agentix://record` plus a static Record a note shortcut. Include optional `actions.intent.OPEN_APP_FEATURE` metadata, with an inline recording-feature inventory. All entry points open the visible Record screen and require the owner's Record action before microphone access. Gemini's documented ability to open apps does not establish arbitrary App Actions support for a sideloaded app; record voice invocation as unverified. No Google SDK, Play publication, wake-word service, or default-assistant role is required.

## Android default assistant extension

Reuse feature 024 and its pinned SDK/dependencies. No new library or backend code is needed. Add `app/.../assistant/` with a lightweight VoiceInteractionService, a protected VoiceInteractionSessionService, and a UI-disabled session that launches a non-exported Compose AssistantActivity using startAssistantActivity (API 26+, matching minSdk). Do not request assist structure/screenshots or implement hotword capture. Service metadata sets supportsAssist=true and disables lockscreen launch.

The activity handles runtime microphone permission and recognition only while resumed, focused, and unlocked. System session invocation may start one turn when permission is already granted; app navigation opens the same screen with explicit Talk. A main-thread controller owns the SpeechRecognizer, coroutine scope, and offline TextToSpeech adapter. Use createOnDeviceSpeechRecognizer on API 31+ only, never createSpeechRecognizer as a cloud fallback. Handle missing model/language, microphone conflict, timeout, and recognizer errors. Stop capture and speech on pause/destruction; late results cannot revive a cancelled turn. No transcript/audio persistence or sensitive logging.

Android-free `core/.../assistant/` defines TellCommand, an AssistantClient boundary, a disconnected implementation, and a turn reducer with identity tokens and explicit phases. Cover duplicate/stale callbacks, cancellation, invalid transitions, and blank input with JVM tests. A disconnected reply is an honest local preview, not a successful command. No HTTP/MCP or Gemini API call in this increment.

Native TTS chooses an installed voice with isNetworkConnectionRequired=false. Display text if unavailable; never start a voice-data install automatically. Persist only the speak-replies preference. Main app gains Assistant navigation with role status, settings link, and Open assistant. Existing recording behavior remains unchanged except the assistant checks its active phase before capturing.

Constitution recheck: no Rust/agent loop/backend changes, no listener, no provider credentials; local-first transcription/TTS, Nix canonical build and Android-free lifecycle tests preserve the existing Kotlin exception. Hands-free wake-word work requires a separate device-backed design and is not implied by the assistant role.

Metadata detail verified in AOSP android-15.0.0_r1 VoiceInteractionServiceInfo: both sessionService and recognitionService must be non-null for assistant eligibility. Supply a real RecognitionService bridge to createOnDeviceSpeechRecognizer, with native callback forwarding, bounded lifetime, recorder contention checks and teardown. This bridge does not implement a model or proxy to itself; the on-device recognizer is the OS-configured provider, distinct from the default recognition service. Protect the bridge with BIND_SPEECH_RECOGNITION_SERVICE.

## Voice/navigation/delegation refinement

Implement pure offline voice ranking/profile and anchored Gemini command parsing in :core, with fixture tests for offline-only selection, British preference, deterministic quality fallback, missing saved voice, punctuation/content preservation and non-triggering mentions. Extend OfflineSpeaker with persisted profile/voice identity, available installed voices, selected label and honest fallback notice; apply pitch/rate and setVoice only to engine-returned voices. Preview is explicit and follows existing controller lifecycle cleanup. Do not change backend wording or reply storage.

Reuse the existing assistant conversation activity for the Inbox Open assistant link. Add a native vector three-dot icon and overflow menu using existing Compose dependencies. Keep saved destination IDs and deep-link behavior, reduce main chips to Inbox/Archive/Record. Voice controls live in the conversation UI.

Gemini handoff uses standard ACTION_SEND/EXTRA_TEXT and ACTION_PROCESS_TEXT/EXTRA_PROCESS_TEXT_READONLY with the question only. Resolve exported enabled handlers in Gemini package com.google.android.apps.bard or Gemini-labelled handlers in Google package com.google.android.googlequicksearchbox, and launch the observed component. Add exact package-visibility queries, not broad package enumeration. No guessed private activity, prompt URL, clipboard injection, accessibility automation, app install or provider API. No backend tell is sent for an explicit local delegation. Missing handler/empty question reports a local error and retains the draft. Existing unresolved backend instructions remain immutable and guarded. The platform handler must be verified on the physical phone; no assumption that a generic Google share handler is Gemini.

Official references: developer.android.com/reference/android/speech/tts/Voice (quality/locale/network features); TextToSpeech#setVoice/setPitch/setSpeechRate; developer.android.com/develop/ui/compose/sharing/send. Source index has no Android platform repository and its package fetcher does not support Android; use official docs. Google support documents Gemini conversation entry but no general prompt URL was found. Keep app-only changes in the current028worktree, reusing024assistant foundation artifacts; no new dependency/backend/schema/permission changes except package visibility.
