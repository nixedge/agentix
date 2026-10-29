# agentix-android

A native Kotlin/Jetpack Compose client for the Agentix assistant, with optional Plaud Note NB-100 integration. It provides a selectable Android voice assistant, on-device instruction transcription, offline spoken replies, opt-in HTTPS assistant commands with durable retries, local phone note recording and read-only Bluetooth discovery.

Plaud recording downloads, audio uploads, recorded-note transcription, task execution and reminder delivery are subsequent work. The backend can store notes/tasks, manage lists and mark tasks complete. The Devices screen does not pair, connect, reset, modify firmware, or delete recordings. Saving a server address does not contact the server.

## Build with Nix

From the **repository root**, not this subdirectory:

```bash
nix build .#agentix-android
```

The APK is `result/share/agentix-android/agentix-android-debug.apk`. The derivation runs `:core:test` and `:app:lintDebug`; their reports are under `result/share/agentix-android/reports/`.

The flake pins Gradle 8.14.4, JDK 17, Android API 35, and build tools 35.0.0. The Android shell is separate from the Rust/CUDA development shell. Nix accepts the Android SDK license for this environment; SDK binaries are unfree upstream distributions. No host Android SDK or Java installation is required.

For interactive development:

```bash
nix develop .#android
cd agentix-android
gradle :app:assembleDebug :core:test :app:lintDebug
```

Use the Nix-provided `gradle` command; there is no independent Gradle wrapper download. Interactive builds may fetch dependencies from Google/Maven/Gradle repositories. Canonical Nix builds use the checked-in dependency snapshot instead.

## Install and try

Enable USB debugging on an Android 8.0+ phone, connect it, and authorize the computer. From the repository root:

```bash
nix develop .#android --command adb devices
nix develop .#android --command adb install -r result/share/agentix-android/agentix-android-debug.apk
```

Application ID: `us.disasm.agentix`. Display name: `agentix-android`.

The Nix build uses an ephemeral debug signing key. A later build may require uninstalling an earlier installation if Android reports a signature mismatch; uninstalling erases local settings and recordings. Establish a stable owner-controlled signing key before using the app for durable recording storage. Release signing and store publication are separate work.

On Devices, wake the Plaud, disconnect any official-app connection, and tap Scan. Grant permission, then tap Scan again to begin discovery. Scanning stops after 15 seconds, when leaving Devices, or when the activity is backgrounded. Android versions before 12 also require Location services for BLE scanning. A displayed project code is advertisement metadata, not a verified model/firmware result.

On Settings, configure the HTTPS base address (default `https://ai.lan.disasm.us`), optionally enter the runtime owner token, explicitly enable the assistant backend and Save. The route is `POST /v1/assistant/tell`. Configuration is private, excluded from backup and never logged; a saved address does not establish connectivity. The phone's existing WireGuard tunnel routes traffic; the app does not manage the VPN. Normal TLS certificate validation applies and redirects are rejected. The server must be deployed/configured separately; the supplied address returned502 during development, so live phone use has not been accepted.
Leave Owner token blank when the server relies on WireGuard-only access. A blank token omits Authorization entirely; a supplied token must match the server and contain32–512 visible ASCII characters. The server still enforces bearer authentication whenever configured.

## Record directly on the phone

Open Record and tap Start recording. Grant microphone permission, then tap Start recording again. Allow notifications if you want the Stop action in the notification drawer. Once started from the visible app, recording continues through a microphone foreground service when switching apps or turning off the screen. Stop from Record or the ongoing notification to save.

The Inbox lists local recordings with date, duration, and size. Play them locally or use Share audio to explicitly send a recording to an app of your choice. Files remain in private application storage and are excluded from backup; uninstalling the app removes them. Nothing is automatically sent to Google or the configured server.

Audio is 16 kHz, mono, 16-bit PCM WAV (roughly 1.9 MB/minute). Completed recordings are finalized atomically from `.wav.part` files. If the process is interrupted, the next Inbox load attempts to recover complete captured PCM frames, marks recovered notes, and preserves files it cannot repair. The service does not restart automatically after process death.

## Recording shortcut

Long-press the app icon and select **Record a note**, or open `agentix://record`. These open the visible recording screen and wait for your Record action; external intents never silently start the microphone.

To test the entry point from a computer:

```bash
nix develop .#android --command adb shell am start -a android.intent.action.VIEW -d agentix://record -p us.disasm.agentix
```

The APK also declares optional `actions.intent.OPEN_APP_FEATURE` metadata for its recording feature. This does not give Gemini access to custom Agentix commands. The owner confirmed that their phone's Gemini cannot invoke our custom tools; the assistant below operates independently of Gemini.

References: [Gemini Device assistance](https://support.google.com/gemini/answer/15235441), [App Actions setup](https://developer.android.com/develop/devices/assistant/get-started), [OPEN_APP_FEATURE](https://developer.android.com/reference/app-actions/built-in-intents/common/open-app-feature).

## Use Agentix as your Android assistant

Open the **Agentix Assistant** tab and select **Choose default assistant**. On the Pixel, choose Agentix under **Settings → Apps → Default apps → Digital assistant app → Default digital assistant app** (wording varies by OS). This is a user choice; the APK does not change the default silently. No Play Store publication, root, Google developer registration, or API key is needed.

Configure the phone's power-button or assistant gesture as desired. Invoking Agentix through the system opens its assistant screen and begins one speech turn when microphone permission is already granted and the phone is unlocked. First use requests permission and waits for a subsequent **Talk** action. **Open assistant** inside the app always waits for Talk. Choosing Agentix replaces the current default assistant; it does not inherit “Hey Google.” No custom wake-word listener is implemented in this version.

Talk uses Android's explicit **on-device** recognizer, available on Android 12+ where a compatible service and language model are installed. It never silently switches to a cloud recognizer. The device's current language is used. Missing recognition/language support and microphone conflicts produce visible errors; typed instructions remain available on older or unsupported devices. **Finish speaking** requests a final transcript; **Cancel**, losing focus, or leaving the activity cancels capture and rejects delayed results. A turn times out after 45 seconds, or 8 seconds after Finish speaking. Note recording and assistant recognition cannot acquire the microphone simultaneously.

Type an instruction and select **Send instruction**, or speak an instruction; final recognized speech uses the same tell boundary. With the backend disabled, no network call is made and the app reports not connected. With it enabled, the client durably stores an immutable request/conversation ID, instruction, IANA timezone and endpoint before sending it. HTTP/authentication/transport/malformed-response uncertainty retains that envelope. Use **Retry pending instruction** to check the same request; another instruction or endpoint is blocked until it resolves. The optional owner token may be added, removed or corrected for the same endpoint. **Stop waiting** stops the UI wait; a server change may still complete. There are no automatic retries. A definitive identity-matched reply is persisted atomically with removal of the pending envelope and recovered when the activity reopens. Explicitly repeat a resolved instruction only when you intend a new action.

**Speak replies offline** uses an installed TTS voice matching the device language that does not require a network connection. If no offline voice is available, the reply remains visible and the app explains the condition. Stop speaking cancels output. The app never downloads voice data automatically. Partial transcripts and drafts stay in activity memory. An enabled backend retains the pending final instruction and latest definitive reply in private backup-excluded preferences for recovery. They are not logged or saved as audio recordings. Uninstalling loses local state; do not uninstall to resolve a pending operation.

## Structure and extension points

- `core/`: Android-free advertisement parsing, endpoint validation, WAV encoding/recovery, assistant command/turn and durable retry state, and JVM tests.
- `app/.../assistant/`: default voice service/session, native recognition bridge, assistant activity/controller, offline TTS, and setup UI.
- `app/.../recording/`: microphone service, private audio store, playback, sharing, and Compose capture/Inbox screens.
- `app/.../device/`: Android BLE discovery. Add the eventual protocol transport here while keeping packet parsing in core.
- `app/.../settings/`: Android-private configuration persistence.
- `app/.../MainActivity.kt`: Compose shell and screens. Future assistant features should have their own packages and share only required services.
- `nix/`: APK derivation and hash-pinned Gradle dependency snapshot.
- `../specs/024-agentix-android/`: specification, plan, tasks, and hardware acceptance checklist.

## Update Gradle dependencies

After deliberately changing dependency versions, run from the repository root:

```bash
nix build .#agentix-android.mitmCache.updateScript --out-link result-android-update
./result-android-update
git add agentix-android/nix/deps.json
nix build .#agentix-android
```

The update script uses nixpkgs's Gradle dependency recorder, temporary storage, and a local recording proxy to generate `nix/deps.json`. Normal package builds replay only the pinned artifacts. Inspect the dependency diff before committing. Do not disable sandboxing or substitute a developer's mutable Gradle cache.

## Validation boundaries

Build status and physical-device results are recorded in `../specs/024-agentix-android/quickstart.md`. A successful APK build does not prove the owner's firmware works with the reverse-engineered protocol.

Next device milestone: inspect protocol version, then download one recording without deletion. Binding changes require a deliberate owner-approved step. The reference implementation's tested device was V95/protocol 10; the owner's unit has not yet been charged or inspected.
