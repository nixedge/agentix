# Android delivery research — 2026-10-30

Owner decision: notifications appear in Agentix Android. Initial delivery is to one phone over the existing private HTTPS/WireGuard connection.

## Verified platform constraints

- Android alarms can run outside the app process and while the device sleeps. Exact-alarm access must be checked; revocation cancels future exact alarms, and grant/boot handling should reconcile them. [Scheduling guide](https://developer.android.com/develop/background-work/services/alarms).
- Android 13+ requires runtime notification permission for ordinary notifications. A manifest declaration alone is insufficient. [Notification permission](https://developer.android.com/develop/ui/compose/notifications/notification-permission).
- Allow-while-idle alarms are rate limited; Android may delay or reorder them, especially in idle mode. Exact scheduling is not a universal one-second or one-minute guarantee. [AlarmManager API](https://developer.android.com/reference/android/app/AlarmManager#setExactAndAllowWhileIdle(int,long,android.app.PendingIntent)).
- Android clears alarms on device reboot, so the app needs to restore them from its journal when that storage becomes available. [AlarmManager lifecycle](https://developer.android.com/reference/android/app/AlarmManager).
- Native JobScheduler execution is deferred by constraints and power state; periodic jobs have a platform minimum period and are not deadline delivery. Persisted jobs require boot permission. [JobInfo.Builder](https://developer.android.com/reference/android/app/job/JobInfo.Builder).

## Design decisions derived from those constraints

- Use native phone-local alarms for already-synchronized reminders. A server push or foreground connection is not needed when the alarm fires. The backend remains authoritative for reminder intent, and the local phone journal records the applied revision and delivery progress.
- Request `SCHEDULE_EXACT_ALARM` special access where required and `POST_NOTIFICATIONS` at runtime. Check app/channel capability before claiming armed status. Do not silently substitute an inexact alarm while claiming precise delivery. Keep blocked reminders visible and reconcile after access is restored.
- Use an immutable broadcast PendingIntent with stable reminder/revision identity and a receiver that validates the current local revision before posting. Re-arm from durable state on boot/app upgrade and reconcile on app entry, command completion and permission changes.
- Keep sync opportunistic with bounded native background jobs; it is not the deadline mechanism. Backend-created reminders remain awaiting-phone until applied. Remote cancellations cannot revoke an offline alarm immediately; report pending propagation until the phone applies the tombstone. This is a distributed-state limit, not a guarantee solved by retrying HTTP.
- Use JobScheduler for bounded network-constrained refresh/reports, respecting its platform minimum interval and retry/backoff rules. Keep private reminder storage credential-encrypted; first-unlock recovery is explicit, with no claim of notification delivery while the device is off or before its journal is accessible.
- Keep the notification-post outbox durable. Reuse the same notification tag/ID after a crash; duplicate alerts remain possible across uncertainty. A posted receipt means the API accepted a post, not that a person saw it.
- No Firebase/Google account, ntfy destination, new public ingress or always-running foreground connection is required by this initial design. No new Android database library is needed; use a private atomic file for the small delivery journal, consistent with existing file-based client persistence.

These are proposed architecture choices, not implemented behavior. Real locked/idle-phone tests and permission denial/revocation tests are required before reporting delivery ready.
