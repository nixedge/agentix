# Tasks: Android personal reminders

## Planning checkpoint

- [x] T001 Allocate028/verify host worktree, draft requirements and audit existing task/legacy notification behavior.
- [x] T002 Record owner's Android delivery decision, research official platform constraints and clarify offline/cancellation semantics.
- [x] T003 Write architecture/state/compatibility plan and dependent implementation tasks.

## Contracts and durable backend (US1, US3, US4)

- [ ] T004 Define versioned turn/sync/report fixtures and bounded reminder/receipt types in `agentix-assistant-core`; specify request/device identity and capability negotiation before implementation.
- [ ] T005 Implement typed reminder validation/transitions/ports and explicit timezone/relative-time rules; test ambiguous/nonexistent times and stale revisions.
- [ ] T006 Implement atomic reminder state, request receipts, due indexes, sync sequence/tombstones and monotonic device observations in `agentix-assistant-sled`; test actual Sled replay/reopen/concurrency and unknown schema handling.
- [ ] T007 Implement application create/list/reschedule/cancel and sync/report coordination in `agentix-assistant`; preserve ordinary tasks and keep backend persistence distinct from phone-applied effects.
- [ ] T008 Add v2 routes/capability negotiation and conditional typed model operations in `agentix-agent`; preserve v1 and old non-reminder prompts. Test auth, context binding, response bounds, request replay and local model selection.

## Android delivery (US1, US2, US3, US4; depends on T004)

- [ ] T009 Add pure Kotlin reminder protocol, revision/sync policy and alarm/post journal state machine in `agentix-android/core`; test stale snapshots, tombstones, namespace changes and replay.
- [ ] T010 Implement private atomic journal, capability/permission UI, notification channel and backend/device enrollment in the Android app; preserve recordings/preferences and pending v1 envelopes.
- [ ] T011 Implement native local alarm/receiver/notification adapters with stable identities, durable-before-effect ordering, revision checks and blocked/uncertain status. Test platform effect boundaries.
- [ ] T012 Implement boot/package/permission reconciliation and bounded native background synchronization; verify current background job API behavior and avoid using sync as the deadline mechanism.
- [ ] T013 Integrate v2 assistant completion with immediate phone synchronization/arming and honest spoken/UI status. Drain legacy pending requests unchanged and handle old backends without enabling unsupported reminders.

## Verification and handoff

- [ ] T014 Run actual-store/backend and Kotlin/platform integration regressions: restart, crashes around dispatch/post, concurrent state changes, pending offline cancellation, denied access and stale receivers.
- [ ] T015 Run targeted fmt/Clippy/tests, local model acceptance and canonical Nix backend/API/unsigned Android builds. Check unrelated model derivations/artifacts remain stable; document completed checks and known legacy failures separately.
- [ ] T016 Document deployment, permissions, phone enrollment, backup/rollback limits, delivery status meanings and existing-key runtime signing; update requirements checklist/verification evidence.
- [ ] T017 After explicit owner authorization, install a same-key APK update preserving data and perform the real locked/background/idle/offline/reboot phone checkpoint. Record lateness, throttling, visible outcomes and limitations.

No implementation or phone mutation has been performed at this checkpoint. Recurrence, snooze and multi-phone delivery remain outside the initial task set.
