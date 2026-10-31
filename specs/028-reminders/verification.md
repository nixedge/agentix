# Reminder implementation evidence

Date: 2026-10-30. Feature branch `028-reminders`, base `9da4ab4`.

## Automated checks

- Actual-store/core/application tests: 43 passed. Includes persisted accepted time and receipts across reopen, concurrent replay, stale revisions, atomic due-index cancellation, DST gaps/folds, report ordering/terminal posting, unknown schema rejection, ordinary record receipts and a Unicode-heavy list with local deadlines, last-reported state and pending reminders prioritized before posted history. Device binding is checked under the assistant admission lock.
- Agent crate tests: 25 passed, including 11 HTTP/gateway regressions; 3 installed-model suites run separately. Covers real Sled enrollment, owner authentication, optional device context conflicts, reminder replay/reporting and independent rejection of invented edit revisions or a time-kind disagreement. The current `/v1/assistant/tell` envelope supports both ordinary and enrolled turns. Bare legacy bodies and protocol-version fields return400; removed v2 and capability routes return404.
- Strict Clippy for core, Sled and application including test targets passes. Production `agentix-agent` binary Clippy passes. Existing all-target legacy agent test lint debt is outside this check; no claim that full workspace/GPU checks ran.
- The single-contract Android Nix build passes JVM tests, lint and APK assembly. Seven reminder policy tests cover durable intent, replay, snapshots, namespaces, cancellation and posted-state recovery. Five tell queue tests include the regression that enrollment changes cannot retarget an unresolved instruction.
- Nix isolation comparison against master: `agentix-llama`, `agentix-whisper`, `agentix-infer`, `agentix-daemon`, `agentix-search`, `agentix-indexer` and `agentix-jails` derivations remain identical. No Cargo manifest/lock or Android dependency snapshot changed.

## Local model acceptance

Tests use synthetic records and the already installed local Qwen2.5-7B-Instruct Q8_0 through the existing gateway. They do not create real reminders, edit the owner's groceries, or post notifications.

Initial reminder checks exposed unnecessary target confirmation and incorrect calendar-time interpretation. Prompt changes alone did not fix rescheduling: the model selected a 24-hour duration for “tomorrow at 3 PM” and invented a revision. The owner authorized the changed approach with “continue.” Edit schemas now enumerate actual active reminder ID/revision pairs, scheduling separately selects elapsed duration versus calendar time, and Rust independently checks both. Operation selection explicitly separates task deadlines from notification requests. Unsupported recurrence/snooze and ambiguous time selection return bounded application-owned clarification text.

The single-contract acceptance run passes all 28 cases in three suites: the 13-case ordinary note/task/list/unsupported-operation suite, six OurGroceries cases and nine reminder cases. Reminder assertions include the exact next local calendar date at 15:00, current ID/revision, relative duration 1200 seconds, ambiguous AM/PM, unsupported recurrence, and ordinary task/note/grocery routing.

## Build / physical device checkpoint

The owner rejected parallel compatibility routes because nothing is in production. Backend and Android now share one current contract under the existing `/v1/assistant/` prefix; no extra nginx location or protocol negotiation is needed. All canonical Nix builds pass for this simplification: `.#assistant-core`, `.#assistant-api`, `.#agentix-agent` and `.#agentix-android`. T018 is complete. Physical-phone acceptance remains open.

The owner's phone checkpoint exposed a missing `ACCESS_NETWORK_STATE` manifest permission when registering network-constrained sync jobs. Android14+ requires it for these jobs ([platform reference](https://developer.android.com/reference/android/app/job/JobInfo.Builder#setRequiredNetworkType(int))). The permission is now declared. The canonical Android rebuild passes JVM tests, lint and APK assembly (Gradle24seconds), and `aapt2 dump permissions` confirms the built APK contains it. The fix is app-only; runtime phone retry remains outstanding.

The final APK was signed and verified outside the store with the existing persistent host key:

`/home/sam/home/agentix/.android-state/apks/agentix-android-028-reminders-network-permission.apk`

APK SHA256: `a646859cb10d608ae7fe38ec47ddc42d748845c3efd44eac66804a8d6f5a0457`. Earlier reminder APKs are superseded by this build.

The unsigned package and core/lint reports are under `result-reminders-android/share/agentix-android/`. No signing secrets entered a derivation or repository.

The agent's earlier `adb devices -l` found no connected devices; the agent performed no installation, uninstall, permission change, backend deployment or real notification. The owner has started the phone checkpoint and reported the permission failure above. Actual AlarmManager/NotificationManager dispatch, receiver recovery and power-state behavior still require the [physical-phone checklist](quickstart.md#real-phone-acceptance-still-required). Pure Kotlin recovery policy and a successful Android build do not replace that evidence. T011/T014 platform acceptance and T017 remain open until it is recorded.

## Alarm / Inbox / concise speech refinement — 2026-10-31

The owner confirmed one audible notification from the previous build, then requested a persistent alarm, reminder entries in Inbox and less spoken detail. The new implementation loops the system alarm tone through a native mediaPlayback foreground service, using alarm volume and audio focus. Explicit dismissal is persisted against namespace/id/revision before playback stops. One loop handles simultaneous reminders; the notification dismisses its captured set, and Inbox permits individual dismissal. Applied cancellation/rescheduling invalidates old playback. Opening the app/notification does not dismiss it. Posted notification-only history is retained without becoming a new alarm after update.

Inbox now includes cached upcoming/active/dismissed/blocked/history reminder entries above recordings, with expandable history. Notification taps select Inbox and bring the reminder to the top. Spoken lists contain at most three pending texts and natural owner-local times, then a remaining count; UUIDs, raw deadlines and observation metadata are omitted. Completed history is excluded.

Verified for this refinement:

- 45 actual-store/core/application Rust tests passed, including dismissal/reopen/replay/terminal ordering, rescheduling a dismissed revision, concise Unicode-heavy list bounds and timezone/DST formatting.
- Scoped strict Rust Clippy (core/Sled/application, all targets), formatting and diff checks passed.
- Canonical Android build passed core JVM tests, Android lint and APK assembly. Eleven reminder policy tests cover concurrent dismissal, stale namespace/revision actions, dismissal/snapshot replay, clock rollback and uncertain-playback preservation. Lint reports0errors; existing dependency/style warnings remain. No full workspace/GPU or physical audio test is claimed.
- Packaged manifest inspection confirms `FOREGROUND_SERVICE_MEDIA_PLAYBACK`, `ACCESS_NETWORK_STATE` and existing alarm/notification/wake permissions. Native playback uses API26-compatible MediaPlayer rather than relying on a newer Ringtone looping API.
- Final canonical Nix builds passed: `.#assistant-core`, `.#assistant-api`, `.#agentix-agent` and `.#agentix-android`. The HTTP regression check covers the shared current assistant contract; installed-model acceptance was not repeated because inference selection did not change.
- Isolation comparison against master: llama, whisper, infer, daemon, search, indexer and jails derivation paths are identical. No manifests, Cargo locks or Android dependency snapshots changed.

Current signed APK (supersedes the network-permission APK above):

`/home/sam/home/agentix/.android-state/apks/agentix-android-028-reminders-alarm-inbox.apk`

SHA256: `2a8df651756e097ae0556174611c32e75f1a8f8b73e4ea7a22f121c3e9f5261c`.

Signed and verified outside the store with the existing persistent host key. Update the backend and Android together to understand ringing/dismissed reports; no new TOML, credentials, listener or nginx route. The agent has not installed this APK, changed phone permissions, deployed the backend or created a real alarm. T011/T014/T017 physical acceptance remains open: locked/background looping, explicit dismissal, concurrent alarms, offline/reboot recovery, audio focus/volume and Inbox navigation must be measured on the phone. Force-stop/power-off and OS restrictions remain delivery limits.
