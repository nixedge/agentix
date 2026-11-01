# Research and decisions

## British speech and explicit Gemini handoff — 2026-11-01

Use engine-returned installed offline voices only. Prefer en-GB for the British profile, with deterministic quality/latency ranking and an explicit voice picker/preview. Persist voice identity together with engine identity. Android voice metadata does not provide a standard gender field, so do not infer one or promise an actor's voice. Apply rate/pitch 0.95 for a measured delivery and show an honest fallback when no British voice is installed. References: [Voice metadata](https://developer.android.com/reference/android/speech/tts/Voice), [TextToSpeech selection and controls](https://developer.android.com/reference/android/speech/tts/TextToSpeech).

Explicit `ask Gemini …` routes locally to a registered Google/Gemini text-sharing component. Use ACTION_SEND/EXTRA_TEXT, with registered PROCESS_TEXT as fallback, exact package visibility and exported/enabled/permission checks; do not guess private activity names or a prompt URL. Discovery on the owner's phone found an exported BardEntryPointActivity for text/plain sharing. No live prompt handoff has been attempted. Android's documented [text sharing contract](https://developer.android.com/training/sharing/send) supports transmitting text, but discovery and successful activity launch do not prove Gemini consumed it or submitted it. UI therefore says the owner may need to press Send.

Gemini supports continued typed/voice conversations within its app. Google's assistant and voice activation depend on Google's configured assistant integration; this explicit handoff does not reproduce its wake-word support while Agentix is default. See [Gemini mobile assistant guidance](https://support.google.com/gemini/answer/14554984?hl=en). No Gemini API key, cloud recognizer fallback or forwarded Agentix context is introduced. External Android source was absent from the agentic-nix inventory; these primary platform references were consulted instead.

## Native Android client

The user selected Kotlin after considering Rust/JNI and Elm/WebView. Keep UI and platform integration native; no cross-language bridge in this foundation. Name the application `agentix-android`, not a Plaud-specific name.

## Nix Android build

The repository's flake.lock pins nixpkgs revision `ac6b2166e7a9375683b8e98f860f273222337b16`. Its Gradle package is 8.14.4 and offers `gradle.fetchDeps` plus the Gradle setup hook. Use `mitmCache`/`deps.json` for hermetic dependency retrieval rather than disabling sandboxing or relying on a mutable Maven cache.

AGP 8.11 supports JDK 17, Gradle >=8.13, build tools 35.0.0, and API <=36: https://developer.android.com/build/releases/agp-8-11-0-release-notes

Kotlin 2.2.0 and matching Compose compiler avoid a separate compiler compatibility matrix: https://kotlinlang.org/docs/whatsnew22.html

## NB-100 discovery

Upstream documents service 0x1910 and manufacturer ID 0x0059. Manufacturer project code is a little-endian uint16 at payload offsets 1–2. These offsets are for manufacturer payload, excluding the Bluetooth company ID supplied separately by Android. Multiple project codes generate Plaud names; they do not prove a physical model or protocol compatibility.

Source: https://github.com/Kurikara-dev/plaud-note-nb100-re/blob/main/docs/02-gatt-profile.md (consulted 2026-10-28).

That project's tested unit was V95/protocol 10. Initial binding replacement is mutating; recording deletion is also exposed by the protocol. Exclude both from the foundation. No universal firmware support claim.

Android permissions vary by OS: SCAN/CONNECT on Android 12+, fine location on earlier supported versions. BLE is optional so future assistant features can run on devices without it. https://developer.android.com/develop/connectivity/bluetooth/bt-permissions

## Assistant boundary

The existing `agentix-agent` includes a home-assistant HTTP surface, but the conversation did not select an upload/authentication/task API. Store a URL without issuing requests; define those contracts alongside the later server feature. WireGuard stays external to this app.

## Phone capture and Gemini (scope extension)

Use AudioRecord with PCM WAV rather than a compressed container: complete PCM frames can be recovered after a process interruption by repairing the header, and speech at 16 kHz/16-bit mono uses roughly 1.9 MB/minute. Foreground recording must start from the visible activity with microphone permission. A microphone-type foreground service permits continued capture after backgrounding; it is not an unrestricted background microphone trigger.

Sources consulted 2026-10-28:
- https://developer.android.com/media/platform/mediarecorder (foreground/microphone constraints)
- https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start (while-in-use permissions and visible-activity start)
- https://developer.android.com/reference/app-actions/built-in-intents/common/open-app-feature (capability and inline inventory)
- https://developer.android.com/develop/devices/assistant/get-started (Assistant integration and deployment boundaries)
- https://support.google.com/gemini/answer/15235441 (Gemini Device assistance can open apps; does not establish this sideloaded app's custom voice action)

Decision: offer native recording and a launcher/deep-link entry independently of Google, with optional App Actions declarations. Do not claim “Hey Google, record a note in Agentix” works until verified on the owner's Gemini setup. No background hotword capture or automatic microphone start from external intents.

## Custom Android assistant

The owner confirmed Pixel 10 and authorized our own Android assistant instead of depending on Gemini custom tools. Sideloading is sufficient for the assistant implementation; system default selection is user-controlled. BIND_VOICE_INTERACTION is a permission required on the service, not granted execution power. A session can launch an assistant activity on API 26+, with setUiEnabled(false) preventing an extra session window. Keep microphone permission handling in the visible activity.

SpeechRecognizer.createOnDeviceSpeechRecognizer is available API 31+ and must be queried for availability; no default cloud recognizer fallback. The service being present does not establish that a requested language model is installed. Native TTS supports checking voice network requirements. A wake-word detector is separate from transcription and default-assistant qualification; low-power DSP access cannot be assumed for a Pixel sideloaded APK.

Primary references (2026-10-28):
- https://developer.android.com/reference/android/service/voice/VoiceInteractionService
- https://developer.android.com/reference/android/service/voice/VoiceInteractionSession#startAssistantActivity(android.content.Intent)
- https://developer.android.com/reference/android/speech/SpeechRecognizer
- https://developer.android.com/reference/android/speech/tts/Voice#isNetworkConnectionRequired()
- https://source.android.com/docs/core/audio/sound-trigger

Eligibility source: AOSP `platform_frameworks_base`, tag `android-15.0.0_r1`, `core/java/android/service/voice/VoiceInteractionServiceInfo.java` checks BIND_VOICE_INTERACTION and rejects metadata without both sessionService and recognitionService. The native recognition bridge satisfies this requirement without claiming custom hardware wake-word access. Indexed Android framework source was unavailable in the session's agentic-nix inventory; this exact upstream file was inspected directly.
