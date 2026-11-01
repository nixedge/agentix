# Hey Agentix: build and phone acceptance

Run from `/home/sam/home/agentix/030-android-wake-word`. This branch starts at028 commit2413c5c, retaining reminders/alarms/snooze, unified Inbox/Archive, British voice controls, simpler navigation and explicit Gemini handoff.

```bash
nix build .#agentix-android -o result-wake-android
nix build .#checks.x86_64-linux.agentix-wake-model --no-link
```

The APK is unsigned in the derivation. Sign outside the store using the existing host key and a fresh output path:

```bash
nix run .#agentix-android-sign -- \
  result-wake-android/share/agentix-android/agentix-android-debug-unsigned.apk \
  /home/sam/home/agentix/.android-state/apks/agentix-android-030-wake-word.apk \
  /home/sam/home/agentix/.android-state/signing/debug.keystore \
  agentix-debug \
  /home/sam/home/agentix/.android-state/signing/keystore-password
nix develop .#android --command adb install -r \
  /home/sam/home/agentix/.android-state/apks/agentix-android-030-wake-word.apk
```

No backend deploy, credentials, new default-role grant, uninstall or data reset is needed for an existing Agentix default-assistant installation. Grant microphone/notifications if missing. App installation and permission changes are owner actions, not implied by automated build evidence.

## Automated verification — 2026-11-01

- Canonical Android Nix build passed all54JVM tests, including five new gate/admission tests. Lint0errors/18warnings/8hints; remaining warnings cover existing SDK/dependency/style issues and optional preference helpers. Assembly passed.
- `checks.x86_64-linux.agentix-wake-model` passed real native inference: synthetic Hey Agentix positive, three synthetic near-phrase negatives and two ordinary-speech negatives. This check runs the pinned2.0.1 native engine and bundled quantized model, not a mocked detector.
- Initial test compilation failed because new tests imported kotlin.test instead of the project's existing JUnit dependency; imports corrected and canonical checks rerun. Initial synthetic wake missed at boost1.5/threshold0.35; scoring tuned within the same detector, then positive/negative fixtures rerun under final boost3/threshold0.25/two trailing blanks.
- Packaged manifest confirms the new service is non-exported and microphone-type. Model/token/keyword/license assets and native libraries are present. Every packaged arm64-v8a/x86_64 ELF LOAD segment supports16KiB alignment. Signing wrapper now uses and verifies16KiB ZIP alignment, and verifies the signature after signing.
- Final unsigned build output: `/nix/store/gqk7hy7z1147qi48gxz3l0vq3hak19c4-agentix-android-0.1.0`. Signed/verified same-key artifact: `/home/sam/home/agentix/.android-state/apks/agentix-android-030-wake-word.apk`,140977012bytes (includes bundled model/runtime/ABIs), SHA256 `5ebb8ed3f5b5af87b740610b9f3edca71b5616ac28929bd8ccfac98453fe8855`.
- Alejandra checks for changed Nix files and Git whitespace checks passed. No Gradle Maven lock/dependency snapshot, Cargo manifest/lock, Rust source or backend contract changed; unrelated workspace/model builds were not rerun. Local application build remains reproducible without a network/runtime model fetch.

## Use it

1. Open three-dot menu → Agentix Assistant. Confirm Agentix is selected as default.
2. Press **Start listening** under Hey Agentix. If asked, grant permissions and press Start again. Keep the notification channel enabled. A microphone foreground notification must appear.
3. Say **Hey Agentix**, pause for the assistant screen/listening status, then say a command. This first version separates wake phrase and command; immediate speech appended to the phrase is not buffered/forwarded. Use **ask Gemini …** for an explicit question handoff.
4. Locked phone: the wake invocation can show an Unlock screen. Authenticate normally; press Talk after unlocking if it does not begin a turn automatically. No conversation history is shown or command captured while locked.
5. Wake listening pauses for recognition, backend processing, spoken replies, recording and calls. It rearms when idle, including when the assistant screen remains open. Failed/silenced microphone access can stop it; inspect setup and use Start again.
6. **Stop listening** in setup or notification releases the listener. After reboot/force-stop/process death, Start again explicitly. A remembered enabled preference alone does not mean the microphone is running.

The detector runs locally with a bundled model, bounded memory, one inference thread and no audio file/network output. It is an opt-in software listener and uses more battery than Google's built-in privileged detector. It does not restore Hey Google. Voice transcription/backend behavior after activation is unchanged.

## Physical acceptance — pending

- [ ] Install same-key update and verify recordings, reminders, archive metadata and pending request identity persist.
- [ ] Ten Hey Agentix trials at normal distance: record successes (target≥9), duplicate launches (target0) and latency. Include different speech rates and noisy/quiet conditions.
- [ ] Fifteen minutes ordinary speech/music with no wake phrase: record false activations (target0).
- [ ] Background app, turn screen off, test unlocked and locked phone; confirm visible activation, authentication requirement, hidden history and no command before unlock.
- [ ] Verify microphone notification, both Stop actions, default-role removal, permission revocation, mic privacy switch, call/audio contention and explicit restart after failures.
- [ ] Cancel while detector is releasing; start/stop note recording; preview speech and listen to a reply containing Hey Agentix; confirm no competing capture, self-trigger or stale launch. A second wake should work after reply even if the screen stays open.
- [ ] Force-stop/reboot and verify no silent listener restart; Start from setup restores it.
- [ ] Wake, then ask Gemini a question; verify received question/continued Gemini conversation and unchanged Agentix default role.
- [ ] Measure an hour with listener enabled versus disabled under similar conditions; record battery/CPU/temperature. No battery or all-day reliability claim from desktop fixtures.
- [ ] Regression: reminder alarm/Snooze/Dismiss/Archive and recording/playback/sharing remain functional.

Build/native fixture evidence is separate from physical recognition, locked/background UI and battery acceptance. The agent has not installed this APK, enabled listening, changed permissions/defaults or captured microphone audio on the phone.
