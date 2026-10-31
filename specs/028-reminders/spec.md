# Feature Specification: Durable personal reminders

**Feature Branch**: `028-reminders`
**Created**: 2026-10-29
**Status**: Implemented — automated verification and physical-phone acceptance tracked in verification.md
**Input**: After merging OurGroceries, the owner identified a reminder system as the likely next feature.

**Owner clarification (2026-10-30)**: Nothing is in production. Use one current assistant HTTP contract under the existing `/v1/assistant/` prefix; remove the parallel v2 routes, protocol negotiation and legacy request/response fallback. Backend and Android are updated together. Preserve user data and immutable request identities, rather than maintaining old wire formats.

## Scope and assumptions

Start with explicit, one-off personal reminders through the existing assistant. Creating a task with a due date remains distinct from requesting a notification. This increment covers create, list, reschedule and cancel, durable delivery state, recovery and honest feedback. Recurrence, location triggers, calendar synchronization, shared accounts, automatic migration of legacy todos and snooze are deferred assumptions for owner review.

The existing personal-assistant specification (025 US5) supplies the reliability requirements. Keep the current local inference, personal storage, OurGroceries integration and ingress configuration. The legacy overdue-todo notifier is a separate existing feature; it must not also send the same new personal reminder.

The owner selected Agentix Android notifications. The first increment targets one enrolled phone. The backend owns reminder intent; the phone keeps a durable delivery cache and arms local alarms. Once armed, a reminder can notify without a live backend connection. New reminders and changes made elsewhere remain pending phone synchronization; the assistant must distinguish this from a change already applied on the phone. This feature requires an Android app update.

## User Scenarios & Testing

### US1 — Schedule an explicit reminder (P1)

The owner says “remind me in twenty minutes to take the laundry out,” or specifies a local date/time. The assistant confirms the reminder and its resolved time only after durable scheduling succeeds.

**Independent test**: Schedule a reminder through the assistant, reopen storage and verify the same reminder and deadline remain. Retry the original request and verify no duplicate reminder is created.

**Acceptance scenarios**:

1. Given a valid request and owner time zone, when scheduling succeeds, then the reply identifies the text, resolved local date/time and notification destination, and the schedule survives restart.
2. Given an ambiguous time such as “at eight,” an ambiguous daylight-saving transition or an invalid local date/time, when the request is interpreted, then the assistant asks for clarification without creating a schedule.
3. Given the same accepted request is retried after a lost response, when replay finishes, then it returns the original result without creating another schedule or reinterpreting “in twenty minutes” against a later clock.
4. Given “save a task to replace the filter next week,” when a task is saved without an explicit notification request, then it does not silently become a reminder.

### US2 — Receive reminders despite restarts and temporary outages (P1)

A due reminder produces an owner-visible notification. Restarting the backend or temporarily losing connectivity preserves the pending reminder and exposes its eventual delivery state.

**Independent test**: Schedule a reminder, interrupt the service before its deadline and restore it afterward. Verify catch-up delivery and inspect its durable outcome. Repeat with a temporary delivery failure.

**Acceptance scenarios**:

1. Given an enabled and healthy destination, when a reminder becomes due, then delivery proceeds without another model inference or foreground assistant interaction.
2. Given the backend restarts before or after the deadline, when it recovers, then acknowledged pending reminders are retained and overdue reminders are offered for catch-up delivery with their original scheduled time.
3. Given a temporary notification failure, when the destination recovers, then pending delivery is retried with a stable reminder/delivery identity and bounded retry frequency.
4. Given delivery may have happened but its acknowledgement was lost, when recovery proceeds, then status remains honest about uncertainty. At-least-once delivery may produce duplicates; no universal exactly-once promise is made.
5. Given the phone is locked and Agentix is not in the foreground, when the selected delivery route is available, then the agreed notification behavior is verified on the actual phone before the feature is declared ready.

### US3 — Inspect, change and cancel reminders (P1)

The owner can ask what reminders are pending, move one to another time or cancel it. Ambiguous targets require clarification.

**Independent test**: Schedule two reminders with similar text, select one explicitly, reschedule it and cancel the other. Verify exact pending state and absence of obsolete future notifications.

**Acceptance scenarios**:

1. Given stored reminders, when the owner lists them, then results come from current authoritative state and show text, local deadline and pending/cancelled/delivery status without relying on semantic search.
2. Given a current reminder revision, when it is rescheduled, then the new deadline and invalidation of the old pending delivery commit together. A stale revision cannot overwrite a newer change.
3. Given cancellation is durably applied on the phone before dispatch, when the old deadline passes, then that delivery is not sent. A backend-only cancellation awaiting an offline phone must be reported as awaiting synchronization.
4. Given dispatch already started during cancellation, when the owner receives the result, then the reply explains that an in-flight notification may still arrive rather than promising it was recalled.

### US4 — Understand whether delivery is actually available (P2)

The assistant distinguishes a saved task, a scheduled reminder, an accepted delivery and a notification actually acknowledged by a recipient where such acknowledgement exists.

**Independent test**: Disable the configured destination or deny its required permission and verify the assistant reports the unavailable/degraded state without claiming a notification was delivered.

## Functional Requirements

- **FR-001**: Support explicit one-off reminder create/list/reschedule/cancel through typed assistant operations with deterministic validation.
- **FR-002**: Persist reminder identity, revision, text, original scheduling instant, resolved deadline, owner time zone, selected destination and scheduling/delivery state before acknowledging success.
- **FR-003**: Resolve relative time from the accepted request instant. Use explicit owner time-zone context; ask about ambiguous or nonexistent local times instead of guessing. Reject an unintentionally past deadline with clarification.
- **FR-004**: Keep reminder changes and pending-delivery invalidation consistent under replay, concurrent edits and restarts. The model cannot manufacture scheduling or delivery receipts.
- **FR-005**: Due-time selection and delivery recovery operate independently of inference, embeddings and search indexes.
- **FR-006**: Persist delivery attempts/outcomes and use a stable identity across recovery. Prevent two concurrent workers from independently claiming the same attempt. Describe uncertainty and duplicate possibilities accurately for the chosen transport.
- **FR-007**: Retain missed reminders for catch-up after a temporary outage and identify their original due time. Do not silently drop acknowledged reminders or silently expire them.
- **FR-008**: Provide owner-visible pending and failed/uncertain status. Keep scheduling acknowledgement distinct from transport acceptance and observed device delivery.
- **FR-009**: Cancellation/rescheduling applied on the phone prevents obsolete work that has not begun dispatch. Backend persistence and phone application have separate acknowledgement states; an offline phone may still notify from its last synchronized revision. Explicitly report this and handle the race with already-started delivery.
- **FR-010**: Reuse existing authentication/ingress policy and runtime-secret handling. Keep personal reminders, transcripts, notification contents and credentials out of committed fixtures and build inputs.
- **FR-011**: Preserve existing local tasks/notes/lists, remote groceries and request replay. No implicit import or modification of legacy notification records.
- **FR-012**: Deliver through Agentix Android system notifications on one enrolled phone. Persist local alarm intent and reconcile it after process restart, device restart and relevant permission changes. Scheduling needs both notification capability and the required alarm access; unavailable capability is visible instead of silently promising precise delivery.
- **FR-013**: A reminder already armed on the phone can notify offline. Remote changes await synchronization; synchronization failures never clear the last valid local schedule. Endpoint/account changes must not mix reminders or receipts between backend installations.
- **FR-014**: Notifications identify a stable reminder/revision. Retried delivery must reuse its notification identity where supported. A notification posted to Android does not establish that the owner saw or heard it.

## Key Entities

- **Reminder**: Stable identity, revision, owner-visible text, resolved deadline/time zone and current scheduling state.
- **Delivery attempt**: Reminder/revision identity, destination, claim/attempt state, retry timing and acknowledged/failed/uncertain outcome.
- **Scheduling receipt**: Durable result binding an accepted request to its exact reminder change, reusable on replay.

## Edge Cases

- Duplicate HTTP requests, a reused request ID with different content, stale edits and ambiguous reminder targets.
- Backend downtime, unavailable phone/destination, denied notification permission, clock corrections and time-zone changes after scheduling.
- Crash before dispatch, crash after notification acceptance but before recording it, and competing worker claims.
- Cancellation or rescheduling while notification delivery is already in flight.
- A delivery route that accepts a message but cannot prove the owner saw a notification.

## Success Criteria

- **SC-001**: Create/list/reschedule/cancel acceptance scenarios pass against actual persistent storage, including reopen and request replay.
- **SC-002**: An already-armed reminder with required permissions targets notification within 60 seconds under normal, unrestricted acceptance conditions. Measure locked/background/idle behavior separately, including closely spaced alarms and Android throttling. This is a measured target, not a timing guarantee under all power states.
- **SC-003**: Restart/outage tests retain every acknowledged pending reminder and recover its exact deadline and delivery identity; obsolete revisions do not dispatch after phone-applied cancellation before dispatch. Test backend-only cancellation while the phone is offline as a pending propagation state.
- **SC-004**: Duplicate-request tests create one reminder per accepted request identity, and concurrent-worker tests authorize one dispatch attempt per claimed delivery identity.
- **SC-005**: An owner-authorized real-phone checkpoint demonstrates scheduling, background notification, rescheduling, cancellation and recovery. Synthetic/provider acceptance is reported separately from actual phone delivery.

## Stage boundary

Android delivery is implemented for one-off reminders; recurrence and snooze remain deferred. Automated implementation evidence and the outstanding live-phone checkpoint are recorded in [verification.md](verification.md). Physical-phone delivery has not yet been accepted.

## Owner refinement — persistent alarms, Inbox and concise speech (2026-10-31)

- FR-015: Newly due reminders play the system alarm tone repeatedly until explicit dismissal. Opening the app or notification drawer does not dismiss them. Respect alarm volume, audio focus and OS restrictions; no claim that audible playback proves the owner heard it.
- FR-016: Persist ringing and dismissed observations for each namespace/id/revision. Dismissal is terminal for that revision, survives restart and synchronization, and cannot dismiss a newer revision. Applied cancellation/rescheduling stops obsolete playback. Existing posted notifications remain history rather than unexpectedly becoming alarms after upgrade.
- FR-017: Inbox shows cached reminder text, local due time and upcoming/ringing/dismissed/blocked/history state alongside existing recordings. Notification taps open Inbox; dismissal is available there and in the alarm notification. Offline inspection and dismissal work without the backend.
- FR-018: Spoken reminder lists contain at most the next three pending reminder texts and natural local deadlines, then a remaining count. Exclude cancelled/dismissed/posted history. Never speak UUIDs, raw timestamps or observation metadata; full delivery details belong in Inbox.

Acceptance: test stale dismissals, dismissal/restart/snapshot replay, concurrent alarms, cancellation while ringing and historical notification upgrades in policy/store tests. Build/lint the actual Android adapter. Physical acceptance still requires checking continuous alarm audio while locked/background, explicit dismissal, offline/reboot recovery, alarm volume and Inbox navigation on the owner's phone.
