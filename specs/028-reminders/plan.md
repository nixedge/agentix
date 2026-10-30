# Implementation plan: Android personal reminders

Branch `028-reminders`, based on merged master `9da4ab4`. Owner selected Android notifications on 2026-10-30 and authorized implementation. Implementation follows this architecture; checks and outstanding device acceptance are recorded in [verification.md](verification.md).

## Architecture

Keep reminder domain types and validation in `agentix-assistant-core`, authoritative persistence in `agentix-assistant-sled`, and orchestration behind typed ports in `agentix-assistant`. `agentix-agent` owns HTTP routes and local model adaptation. Extend existing crates rather than introducing another overlapping crate. Reminder due lookup never depends on LanceDB or inference.

Use a dedicated reminder aggregate and scheduling/delivery receipts alongside note/task/list receipts. Following the owner's clarification that nothing is in production, use one current turn contract and reminder synchronization endpoints under the existing `/v1/assistant/` prefix. Remove the parallel v2 surface, version negotiation and old-body fallback. Ordinary turns carry a null device identity; explicitly enrolled phones carry their installation identity. Gate reminder tools/context on enrollment, preserving ordinary grocery/note/task behavior and immutable request identity.

Backend reminder state includes identity, revision, text, resolved UTC instant, source time zone, target installation and desired active/cancelled state. Request receipts, state changes and sync sequence/tombstones commit and flush atomically. A due-time index is maintained with the same batch. Device reports are separate observations tied to the exact reminder revision; they do not change desired intent. Revision/identity conflicts are explicit.

The phone persists a bounded delivery journal in a private atomic file, namespaced by backend identity and installation. Core Kotlin policy handles validation, revision ordering, replay and alarm/post decisions without Android APIs. App adapters implement file persistence, AlarmManager, notification channels/permissions, receivers and HTTPS sync.

## Delivery lifecycle

1. An accepted turn durably creates or revises reminder intent on the backend and returns an exact scheduling receipt. This initially means saved/awaiting-phone.
2. Android validates and durably applies a complete bounded sync response, including tombstones. Truncation, malformed data or an interrupted response never becomes an empty authoritative snapshot.
3. Persist local alarm intent before calling AlarmManager; use stable PendingIntent identity for idempotent replacement. After scheduling returns, persist armed state and queue its acknowledgement. A crash at any boundary leads to reconciliation, not a lost reminder.
4. A receiver checks backend namespace, reminder revision, cancellation and current notification capability under the local journal lock. It durably records post intent, posts with a stable notification tag/ID, then records the result and queues an idempotent report. No network or model call is required on this deadline path.
5. Reconcile cached intent after boot, package replacement, app entry and alarm-permission grants. Catch up due entries with their original scheduled time. Permission denial/revocation is a blocked state; missing acknowledgement is not proof of user-visible delivery.

The app presents phone-specific confirmation only after local arming. Backend-only responses describe persistence/awaiting-phone. Offline backend edits/cancellation remain pending propagation until the phone applies the new revision; do not promise global recall of a previously armed offline alarm. Once a cancellation is locally applied, stale receivers cannot post. Already-posted or in-flight notifications have explicit status.

## Synchronization and enrollment

First version supports one explicitly enrolled phone. Enrollment uses the existing owner auth/WireGuard policy; a device ID is routing identity, not a replacement authentication secret. Backend installation identity prevents applying a snapshot or acknowledgement to the wrong server. Changing destination requires an explicit transition; do not silently enroll a second phone and produce duplicate notifications.

Sync on assistant command completion/app entry, boot recovery after private storage is accessible, and bounded network-constrained native JobScheduler jobs. Background sync is best effort, so reminders created elsewhere stay visibly awaiting-phone until acknowledged. Respect the platform's minimum periodic interval and test bounded retries/backoff. Do not use a job deadline as a notification timing guarantee or add WorkManager/Room/SQLite merely for a timer.

Cancellation tombstones remain available until the selected phone has applied them; stale snapshots cannot revive old revisions. Device acknowledgement requests are idempotent and monotonic. Backend status distinguishes latest observation from desired state and records observation time; permission revocation after an old armed report must not be presented as a continuously verified guarantee.

## Versioning and persistence

Use only `/v1/assistant/tell` plus `/v1/assistant/reminders/{enroll,snapshot,report}` on the same daemon. Every turn uses `{device_id: UUID|null, request: TellRequest}` and receives `{device_id: UUID|null, response: TellResponse, reminder_receipts: [...]}`. Remove wire protocol-version fields and the capability-negotiation endpoint. Device context is part of request identity; pending requests remain immutable across enrollment changes. Reminder receipts stay separate from record receipts. Private persistence schemas remain versioned for corruption detection; preserving old HTTP formats is explicitly outside scope.

Following installed-model acceptance failures and owner authorization, reminder inference selects the operation first, then selects elapsed duration versus local calendar time for create/reschedule, then fills a schema constrained to that time shape and current reminder ID/revision pairs. Rust revalidates the selected pair and time kind even if a gateway ignores its schema. Generic unsupported/ambiguous classification returns application-owned clarification text. Existing ordinary inference stays unchanged.

Design Sled key/schema handling explicitly before writing production data. Existing databases remain readable by the new binary; old binaries may not understand new turn states. Document rollback limits and require a backup checkpoint before live data tests. Phone delivery journal schema changes must not overwrite malformed/unknown persisted state as empty or lose pending turns/recordings.

## Constitution check

- Library/domain separation: existing pure Rust core/application ports; listener stays in agentix-agent. Android platform effects stay in app adapters.
- Local-first inference: existing Qwen/gateway path, no new cloud inference or push service.
- Reproducibility: existing Rust/Android Nix packages and pinned dependencies; runtime signing key remains outside the store.
- Persistence/testing: actual Sled reopen/concurrency/replay tests, pure Kotlin policy/journal tests, Android platform tests and explicit real-phone acceptance.
- State machines: typed backend desired/observed state and phone alarm/post outbox; explicit transitions and uncertainty.
- Single current contract: backend and Android update together under the existing ingress prefix. Preserve user storage, groceries and immutable pending identities; no old-server/client protocol fallback, legacy todo migration or duplicate ntfy path.

No constitution exception is proposed. Full workspace/GPU checks and unrelated existing lint debt are reported separately from feature-scoped checks.

## Validation and rollout

Test actual Sled atomic receipts, revision/due indexes, sync ordering/tombstones, concurrent claims, reopen and cancellation propagation. Test phone crashes around every durable/OS boundary, stable identity, stale receivers, permission changes, endpoint changes and notification-post uncertainty. Test local-model operation selection including task-vs-reminder and ambiguous time clarification.

Canonical checks build backend/core/API and unsigned Android APK through Nix. Verify unrelated model package/dependency artifact identities stay unchanged. Sign with the existing persistent host key using the runtime wrapper; no uninstall/key replacement is part of this feature.

An explicitly authorized phone checkpoint must test locked/background/idle delivery, backend/WireGuard loss after arming, reboot recovery, denied/revoked access, offline remote cancellation followed by sync, duplicate requests and closely spaced reminders. Record actual lateness and any OS throttling; do not claim platform-independent exactly-once or exact-deadline delivery.
