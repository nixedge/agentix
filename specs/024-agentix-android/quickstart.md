# Quickstart and validation

## Current voice, navigation and Gemini update — 2026-11-01

Implemented in sibling worktree `028-reminders`, retaining the unified Inbox/Archive and snooze work. This increment is app-only: no backend deployment or configuration change.

Signed with the existing persistent host key outside the Nix store:

```bash
nix develop .#android --command adb install -r /home/sam/home/agentix/.android-state/apks/agentix-android-028-voice-gemini.apk
```

APK SHA256: `5c535fbafa4e4bc1f15ef7d9237394cae0189ccb10fbae0a54c2625a99b845ed`.

Canonical `nix build .#agentix-android -o result-reminders-android` passed: all 49 core JVM tests, including six voice/handoff tests, seven Inbox tests and sixteen reminder tests. Android assembly and lint passed (0 errors, 16 warnings, 8 hints). Signing wrapper verified the APK signature. No Gradle dependencies, Rust sources or backend contracts changed. Git whitespace checks passed.

Phone acceptance still required:

1. Open Inbox. Use **Open assistant** immediately above it to reach the text field and Talk control. Verify the three-dot menu contains Agentix assistant, Settings and Devices; Inbox, Archive and Record remain directly accessible.
2. In the assistant, select **British, measured**, choose an installed offline voice and press **Preview voice**. Rate and pitch are slightly lowered; the voice is supplied by Android's engine. Install British offline voice data using **Android speech settings** if the app reports a fallback. Confirm the choice survives reopening. This changes speech only, not reply wording.
3. Type `agentix ask gemini Explain why the sky is blue`, then send. Gemini should open with that question; press Send there if necessary and continue the conversation in Gemini. Repeat with Talk and `ask Gemini …`. Return to Agentix and verify its default assistant selection is unchanged.
4. Try `ask Gemini` without a question. It should ask for the question locally. Without an available Gemini text handler, the app should report failure and retain the draft. Ordinary mentions such as `add ask Gemini to my notes` still use Agentix. A pending backend request retains its original retry destination.
5. Verify switching apps stops Agentix recognition/speech, and the normal Inbox/alarm/Snooze/Archive flows still work.

Read-only inspection of the attached Pixel found Gemini's exported, enabled `text/plain` ACTION_SEND handler in `com.google.android.apps.bard` and Google's PROCESS_TEXT handler. This establishes installed handoff capability; no question was sent and no prompt-prefill, conversation or audible voice check was performed. No app was installed or default role changed by the agent. Agentix routes only explicit Ask Gemini commands and forwards only the question, without history, recordings or a backend tell. It does not implement a Hey Google wake phrase.

The sections below preserve earlier foundation evidence; their disconnected-backend and original APK instructions describe that earlier scope.

Run from `/home/sam/home/agentix/024-agentix-android`:

```bash
nix build .#agentix-android
nix develop .#android --command adb install -r result/share/agentix-android/agentix-android-debug.apk
```

See `agentix-android/README.md` for interactive builds and updating the Gradle dependency snapshot.

## Automated evidence

Verified 2026-10-28 for the complete scope, including the Android default assistant, phone recording and backup exclusions:

- `nix build .#agentix-android` passed with hash-pinned Gradle dependencies in the sandboxed derivation.
- Thirteen JUnit tests passed: endpoint validation (2), Plaud advertisement parsing (3), WAV format and interrupted-file recovery (4), assistant command/state/cancellation (4); zero failures/errors/skips.
- Android lint passed with no errors. Remaining warnings concern pinned SDK/dependency versions, a version-specific manifest attribute, and optional Kotlin extension style; primitive-state optimization hints also remain.
- `apkanalyzer` confirmed application ID `us.disasm.agentix`, version code 1/version 0.1.0, microphone permission/service type, recording shortcut/deep link, launcher icon, backup exclusions, protected assistant/session/recognition services, non-exported assistant activity, and packaged voice-service metadata with lockscreen launch disabled.
- APK size: 9,455,848 bytes; output `result/share/agentix-android/agentix-android-debug.apk`.
- Alejandra checks passed for both Nix files; Git whitespace checks passed.
- No Rust source changed; workspace Cargo tests were not rerun for this Android-only addition.

## Physical acceptance — pending hardware

- [ ] Install on an Android 8.0+ phone; open Inbox, Devices, Settings offline.
- [ ] Save an HTTPS address, restart the app, verify it persists; reject malformed/plaintext URLs.
- [ ] Deny scan permission and retry; test Bluetooth disabled and permission revocation.
- [ ] Scan the charged NB-100; confirm its advertisement appears.
- [ ] Confirm scan ends after 15 seconds, on navigation, backgrounding, and rotation.
- [ ] Verify no pairing change or recording mutation occurred.
- [ ] Record with the phone microphone; turn the screen off and verify continuous audio.
- [ ] Stop from the notification and from the app; verify local playback and explicit sharing.
- [ ] Force-stop during capture, reopen, and verify complete PCM frames are recovered as a playable note.
- [ ] Test microphone/notification permission refusal and revoked microphone access.
- [ ] Open the launcher shortcut and `agentix://record`; confirm capture waits for the Record action.
- Gemini custom-tool invocation is unsupported on the owner's phone; the custom assistant replaces this dependency.
- [ ] Record the owner's firmware and protocol version before implementing binding or download operations.

No phone or charged NB-100 is connected to this session (`adb devices` returned an empty list). The client does not yet implement Plaud audio extraction or server integration.

## Android default assistant acceptance

Install the APK, open Agentix Assistant, and select Choose default assistant. On the Pixel, select Agentix in Settings → Apps → Default apps → Digital assistant app → Default digital assistant app. Configure the phone's assistant gesture/power-button action if needed. Open assistant from the app for an independent test without changing the default.

- [ ] Agentix appears in the default assistant selector; system gesture opens its assistant activity.
- [ ] First invocation requests microphone permission and does not begin capture automatically after permission grant; Talk starts a turn.
- [ ] Subsequent unlocked system invocation starts one turn with a visible screen; opening from the app waits for Talk.
- [ ] In airplane mode, dictate “add pineapple to my grocery list”; see the final instruction and the explicit backend-not-connected acknowledgement. No task completion is claimed.
- [ ] Finish speaking produces a final result; Cancel discards unfinished speech. Late results after Cancel/retry cannot produce a reply.
- [ ] Test microphone denial/revocation, missing offline language model and unavailable recognizer; typed instructions still work.
- [ ] Lock, press Home, pull down notification shade, switch apps and rotate during capture/output. Microphone and speech stop, no hidden capture resumes; rotation discards ephemeral text.
- [ ] Try Talk during an active note recording and note recording during recognition; the second capture is refused and the first stays intact.
- [ ] With an installed offline TTS voice matching device language, hear the acknowledgement; without one, see the text and offline-voice error. Stop speaking and disabling spoken replies stop output.
- [ ] Restore the previous default assistant if desired. Confirm no custom wake-word listening is claimed or active.

No Agentix backend or MCP service is needed for these Android-only checks. The extension introduces no new Gradle dependencies; existing pinned artifacts are reused.
