# Offline Kokoro voices

Branch031 starts at030commit27becc6 and includes the reminders, Inbox/Archive, Gemini handoff and wake-word changes. This increment needs an app update only; no backend deployment/configuration change.

## Build and use

```bash
nix build .#agentix-android -o result-kokoro-android
```

Sign outside the store with the existing persistent key using `.#agentix-android-sign`; choose a fresh output filename because the wrapper refuses overwrites. See `agentix-android/README.md` for the signing command. Install with `adb install -r`, preserving app data and signing identity.

Open Inbox → Open assistant → Voice. Choose **Kokoro · bm_george** or **Kokoro · bm_daniel**, then **Preview voice**. Subsequent spoken replies use that choice, stored separately from the existing Android profile/installed voice settings. Choose **Android voices** to return to those settings. Existing installations retain Android as their initial provider.

Kokoro runs on the phone with a bundled model and a measured0.95 rate. No API key, server speech request or runtime download is used. First Preview verifies/copies the approximately295MB model to private backup-excluded storage and initializes the local CPU engine; subsequent calls can reuse it while the assistant activity remains alive. Native cache can consume additional space. It requires an arm64 phone; unsupported devices retain Android voice support. Errors are visible rather than silently selecting another voice.

**Stop speaking**, switching provider/voice, disabling spoken replies or leaving the assistant cancels pending synthesis/output. The worker checks generation identity before producing playback and holds wake detection until actual audio cleanup. Backend mutation/request identity is independent of speech cancellation.

## Verification evidence

- The initial canonical Nix build passed59core JVM tests, including five new preference/voice-ID/Unicode/chunk-bound/cancellation tests. Android assembly and lint passed with0errors,19warnings and8hints.
- Google Maven tasks-audio1.2.0 and its dependency snapshot are pinned. Model revision33ad69fec6c4d9eb10a547aaf9a8c63e7c446a56 and SHA256 e7b931ee5d4b7104012a838c9eee240d19ea69cb3ca269041a3274f83dbb7fa1 are fixed by Nix; copy verifies size/hash before use. The package includes relevant model/runtime/lexicon license notices.
- The owner urgently requested signing before leaving. Signed/verified same-key APK `/home/sam/home/agentix/.android-state/apks/agentix-android-031-kokoro-voices.apk` was provided. SHA256 `fff72e520a097d1ecc7c5b712b60705eb9c83ba37a4fae4fc2f8e2044c842d45`,591403953bytes. It contains the tested production code; additional model/lexicon attribution assets and the smoke-test source were added afterward for this commit. Signing verifies signature and16KiB ZIP alignment.
- `smoke/KokoroNativeSmoke.java` is a fixed-sentence model/JNI fixture, not application code or a passing phone test. It compiles/dexes with the project's JDK/Android tools. A temporary adb harness stopped before native execution because adb consumed the enclosing Bash script's stdin; future heredoc-based runs must use `adb shell -n`. No model/audio output was generated and no app was installed. Native synthesis, audible quality and phone latency are unverified.
- No Rust, Cargo lock, backend contract or credentials changed. Git whitespace/Nix formatting checks passed. Physical acceptance remains open regardless of JVM/lint/build results.

## Phone acceptance — pending

- [ ] Preview both exact voices in airplane mode; confirm audible output, persistence after reopening and subsequent reply voice. Check normal text, names, numbers and Unicode.
- [ ] Measure cold model setup, warm synthesis latency and memory. Compare perceived voice/delivery by listening; these are stock voices rather than a custom actor voice.
- [ ] Stop during first copy, initialization, synthesis and playback. Switch voices repeatedly; background/lock during speech; deny/lose audio focus. Confirm no delayed speech restarts.
- [ ] Verify wake detection pauses through speech and rearms afterward, including a reply containing the wake phrase. Check recognition/note recording contention.
- [ ] Confirm Android profiles/voice choice, recordings, Inbox/Archive, alarm/Snooze/Dismiss, Gemini handoff and pending backend request identities remain intact.
- [ ] If using the JNI smoke fixture, generate both fixed-sentence samples and verify non-empty finite24kHz output before claiming SC-002. Keep this separate from the app's actual playback/lifecycle tests.
