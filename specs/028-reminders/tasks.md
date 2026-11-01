# Tasks: Android personal reminders

## Planning checkpoint

- [x] T001 Allocate028/verify host worktree, draft requirements and audit existing task/legacy notification behavior.
- [x] T002 Record owner's Android delivery decision, research official platform constraints and clarify offline/cancellation semantics.
- [x] T003 Write architecture/state/contract plan and dependent implementation tasks.

## Contracts and durable backend (US1, US3, US4)

- [x] T004 Define current turn/sync/report fixtures and bounded reminder/receipt types in `agentix-assistant-core`; specify request/device identity before implementation.
- [x] T005 Implement typed reminder validation/transitions/ports and explicit timezone/relative-time rules; test ambiguous/nonexistent times and stale revisions.
- [x] T006 Implement atomic reminder state, request receipts, due indexes, sync sequence/tombstones and monotonic device observations in `agentix-assistant-sled`; test actual Sled replay/reopen/concurrency and unknown schema handling.
- [x] T007 Implement application create/list/reschedule/cancel and sync/report coordination in `agentix-assistant`; preserve ordinary tasks and keep backend persistence distinct from phone-applied effects.
- [x] T008 Add the single current assistant/reminder HTTP contract and conditional typed model operations in `agentix-agent`. Test auth, device context binding, response bounds, request replay and local model selection.

## Android delivery (US1, US2, US3, US4; depends on T004)

- [x] T009 Add pure Kotlin reminder protocol, revision/sync policy and alarm/post journal state machine in `agentix-android/core`; test stale snapshots, tombstones, namespace changes and replay.
- [x] T010 Implement private atomic journal, permission UI, notification channel and backend/device enrollment in the Android app; preserve recordings/preferences and pending instruction identity.
- [ ] T011 Implement native local alarm/receiver/notification adapters with stable identities, durable-before-effect ordering, revision checks and blocked/uncertain status. Test platform effect boundaries.
- [x] T012 Implement boot/package/permission reconciliation and bounded native background synchronization; verify current background job API behavior and avoid using sync as the deadline mechanism.
- [x] T013 Integrate assistant completion with immediate phone synchronization/arming and honest spoken/UI status. Keep one current request/response format and immutable pending device context; update backend and app together.

## Verification and handoff

- [ ] T014 Run actual-store/backend and Kotlin/platform integration regressions: restart, crashes around dispatch/post, concurrent state changes, pending offline cancellation, denied access and stale receivers.
- [x] T015 Run targeted fmt/Clippy/tests, local model acceptance and canonical Nix backend/API/unsigned Android builds. Check unrelated model derivations/artifacts remain stable; document completed checks and known legacy failures separately.
- [x] T016 Document deployment, permissions, phone enrollment, backup/rollback limits, delivery status meanings and existing-key runtime signing; update requirements checklist/verification evidence.
- [ ] T017 After explicit owner authorization, install a same-key APK update preserving data and perform the real locked/background/idle/offline/reboot phone checkpoint. Record lateness, throttling, visible outcomes and limitations.
- [x] T018 Apply owner-directed single-contract simplification: keep only `/v1/assistant/`, use one turn envelope with optional enrolled device identity, remove protocol negotiation/fallback, update backend/app/tests/examples/docs together, and rebuild/sign the matching app without resetting data.

Implementation is present and local-model acceptance passes. T011 adapters are implemented and build/lint cleanly, with actual platform effect boundaries still awaiting a device. T014 store/policy coverage passes, with physical dispatch/recovery cases still open. T015 build/model results are recorded in verification.md. No phone mutation has been performed. Recurrence, snooze and multi-phone delivery remain outside the initial task set.

## Owner refinement (2026-10-31)

- [x] T019 Extend Rust/Kotlin observation policy with ringing/dismissed states, terminal dismissal and revision-safe recovery; cover actual-store and journal regression cases.
- [x] T020 Implement looping foreground alarm playback, explicit revision-bound dismissal, manifest declarations and AlarmManager catch-up; add reactive reminder Inbox and notification navigation while preserving recordings.
- [x] T021 Bound spoken lists to three pending items with natural local times and a remaining count; test timezone formatting, history exclusion and absence of spoken metadata.
- [x] T022 Run scoped Rust checks and canonical Android/backend Nix builds, sign with the existing key, document artifact and remaining real-phone acceptance. No installation or deployment implied.

## Owner-authorized snooze

- [x] T023 Add persisted local snooze deadline/generation, exact action validation and format1→2 migration; test repeated/stale actions, snapshots/restart, cancellation/rescheduling and dismissal.
- [x] T024 Add generation-aware AlarmManager recovery plus notification/Inbox Snooze10minutes actions and honest blocked/failed display; preserve existing alarms and data.
- [x] T025 Build/test/lint Android through Nix, sign with existing key, commit and document app-only update plus outstanding physical snooze checkpoint.
