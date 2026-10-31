# Android reminders: deploy and test

This feature needs both a backend deployment and an Android update. It uses the existing assistant database/listener and local Android alarms, without a push account or ntfy configuration. One phone can enroll; a different installation is rejected rather than silently replacing it.

## Backend

Pin the deployment input to the feature commit, retaining the current assistant database, OurGroceries environment file and authentication policy. No additional TOML section, reminder credentials or listener is needed. Back up the stopped assistant database before the first live test. Keep legacy `ntfy` configuration as it is; these reminders do not use it.

The existing nginx `/v1/assistant/` location proxies all assistant/reminder routes to the same configured agent port (the examples use 8767). Update the flake input and rebuild/deploy; no additional nginx location is needed. If an earlier development configuration added `/v2/assistant/`, remove that unused location. The owner confirmed nothing is in production, so backend and Android use one current contract without old-client/server fallback.

The full TOML-generating examples are [personal assistant](../../agentix-agent/examples/nixos-personal-assistant.nix) and [OurGroceries](../../agentix-agent/examples/nixos-ourgroceries.nix). Retain existing TLS, WireGuard restrictions and optional bearer authentication. Do not route this to Synaptex on 8765.

The app explicitly enrolls through `POST /v1/assistant/reminders/enroll` and receives its initial complete snapshot. There is no capability-negotiation endpoint. A 404 during enrollment means the expected route/backend is missing; 401 means authentication needs correction.

## Android update

Build from the repository root:

```bash
nix build .#agentix-android -o result-reminders-android
android_state="$(cd .. && pwd)/.android-state"
mkdir -p "$android_state/apks"
nix run .#agentix-android-sign -- \
  result-reminders-android/share/agentix-android/agentix-android-debug-unsigned.apk \
  "$android_state/apks/agentix-028-reminders.apk" \
  "$android_state/signing/debug.keystore" \
  agentix-debug \
  "$android_state/signing/keystore-password"
nix develop .#android --command adb install -r "$android_state/apks/agentix-028-reminders.apk"
```

Use the existing persistent signing key. The wrapper refuses to overwrite an APK; use a fresh output filename when rebuilding. Signing happens outside the Nix store. Do not uninstall: a same-key update preserves recordings, preferences and pending instructions.

On **Agentix Assistant**, retain the HTTPS endpoint/token and enabled backend. In the reminder setup, allow Android notifications and **Alarms & reminders**, then select **Enable reminders**. This explicitly enrolls the installation and starts synchronization. New installations do not enroll merely by saving an address. On older Android versions only the relevant permissions are shown. A denied permission or blocked notification channel produces a blocked state, not a claim that the phone is scheduled.

Try “Remind me in two minutes to check the laundry.” The reply should say **Scheduled on this phone** and identify its time. Then lock the phone. The alarm uses cached intent and does not need backend/WireGuard connectivity after arming. “Save a task…” continues to save a task without scheduling an alert. Recurrence and snooze are not implemented; the assistant asks for clarification instead of silently creating a repeating schedule.

## Status and recovery

- **Saved / awaiting phone** means durable backend intent, without evidence that the alarm is armed.
- **Armed** is the phone's last report that Android accepted the local alarm. It does not continuously verify permissions or power state.
- **Blocked** means notification or alarm access is unavailable. Restore access and open the app or select **Sync reminders**.
- **Ringing / Alarm active** means foreground alarm playback started. The system alarm tone loops using alarm volume until dismissed; audio focus, OS policies and volume can interrupt or silence it. This does not establish that the owner heard it.
- **Dismissed** means explicit dismissal was persisted on the phone for that exact revision. It will not ring again after restart/sync. Use Dismiss in Inbox or Dismiss/Dismiss all in the ongoing alarm notification. Opening the notification or app does not dismiss it.
- **Posted** is notification-only history from the earlier app. Updating will not replay those historical notifications as alarms.
- **Pending / uncertain** preserves an interrupted or failed effect for reconciliation. Retrying uses stable alarm and notification identities; duplicates remain possible after a crash around posting.
- **Cancelled on backend** awaits propagation. An offline phone can still notify from its previous revision. **Cancellation applied on this phone** means its obsolete alarm/tray entry has been cancelled locally; an already observed alert cannot be recalled.

Open the app or select Sync after restoring connectivity. Native network-constrained jobs also synchronize in the background, subject to Android scheduling and backoff. The periodic interval is at least 15 minutes; synchronization is not the deadline mechanism. Boot/package replacement, clock changes, alarm-access grants and app entry reconcile cached alarms. Reboot recovery requires the phone's private storage to become accessible. Force-stop prevents delivery until the app is opened again. Idle quotas, closely spaced alarms, device power policies and notification settings can delay or suppress alerts; no universal timing or exactly-once guarantee is made.

The notification timestamp is the original scheduled instant, including catch-up. Reminders store an absolute instant plus their source IANA time zone; changing the phone's time zone does not reinterpret them. Ambiguous/nonexistent DST times and unspecified AM/PM require clarification. Relative time is frozen when the original request is accepted, including after retry.

## Persistence and rollback

The backend stores reminders, revisions, due keys, device-bound request receipts, delivery reports and cancellation tombstones in the existing Sled database. Its private reminder book has schema version 1. Every reminder mutation and its request receipt commit together and flush before success. Report receipt IDs are retained for idempotency; last observations include server receipt time. Phone storage is a private, backup-excluded atomic journal (`files/reminders-v1.json`, schema 1). Malformed or unknown data is refused rather than replaced with an empty schedule.

Back up backend state and app-private files/preferences together before live acceptance. Do not reset either side's identity to fix an error. A changed backend namespace or endpoint is rejected; restore the original configuration. Installation replacement/multi-phone migration and automatic tombstone/report pruning are outside this increment. Capacity is 1,000 retained reminders, including cancelled/posted/dismissed history. Spoken lists contain at most three pending reminders and a remaining count, without IDs or observation metadata. Inbox contains cached reminder details and history alongside recordings (initially20 entries, expandable). Notification taps select Inbox and bring the selected reminder to the top. At capacity, creation fails explicitly without dropping older reminders.

Update the backend and Android app together; earlier development wire formats are unsupported. Pending instructions retain their request/device identity across enrollment changes. Downgrading an app with active alarms or rolling back the server cannot safely undo phone effects: cancel and synchronize first, then retain backups. Restore a coordinated backup only with a deliberate recovery plan; restoring just one side can lose evidence or resurrect already-delivered work.

## Real-phone acceptance (still required)

After deploying both components and authorizing a same-key update, record actual due/post times and visible outcomes for:

1. Normal and locked/background delivery, including two closely spaced alarms and idle/power-saving conditions.
2. Backend outage and WireGuard loss after arming; restore connectivity and verify reports/replay.
3. Reboot, app process death and package update, preserving reminder identity and original deadline.
4. Reschedule/cancel before dispatch, stale receivers, and offline backend cancellation followed by phone synchronization.
5. Denied/revoked notifications, blocked channel and revoked alarm access; restore access and verify catch-up.
6. Retry an unresolved instruction; verify one schedule and its original relative deadline. Confirm existing notes/tasks/OurGroceries and recordings remain intact.
7. Let the alarm loop for at least30seconds while locked. Open the notification drawer, then tap the notification: Inbox opens and the alarm continues. Dismiss it there; verify audio stops and dismissed history remains after reopening/sync/reboot.
8. Ring two reminders together. Dismiss one in Inbox and confirm the other remains active; Dismiss all stops the captured set. Reschedule/cancel a ringing reminder and synchronize; obsolete playback stops. Test alarm volume and audio-focus interruption/recovery. Existing posted history must not ring merely because the APK was updated.
9. Ask for upcoming reminders with more than three pending entries: hear only three texts and natural local times plus the remaining count, without IDs or delivery timestamps. Inspect all cached entries in Inbox.

An APK build and synthetic model tests do not establish physical-phone delivery. See [verification evidence](verification.md).
