# Current reminder-related implementation

Base: merged master `9da4ab4`. Feature number 028 was allocated after checking all branches, worktrees, existing specs and the known unused/abandoned feature numbers. Worktree verified at `/home/sam/home/agentix/028-reminders`.

- `specs/025-personal-assistant/spec.md` US5 already requires durable scheduling/delivery, exact due lookup, recovery, revision invalidation and honest at-least-once semantics. Reuse those requirements.
- `agentix-assistant-core/src/model.rs` has tasks with optional UTC due dates; it has no reminder entity or delivery state. A task deadline currently does not mean a notification is scheduled.
- `agentix-agent/src/assistant/gateway.rs` explicitly selects unsupported reminder delivery and distinguishes it from saving a task with a due date. This behavior must change only after an actual delivery path exists.
- `agentix-agent/src/scheduler/notification.rs` polls legacy overdue todos and sends through ntfy with attempt/sent/failed records. It operates on the separate legacy store, not the new personal assistant repository. This is useful adapter precedent, not an existing implementation of personal reminders.
- `agentix-android/app/src/main/AndroidManifest.xml` declares notification permission. This declaration alone does not establish a personal reminder delivery or background scheduling mechanism.

The owner selected Android notifications on 2026-10-30. See research.md for official platform sources and plan.md for the proposed phone-local alarm and backend synchronization design. Real-phone acceptance remains outstanding; no delivery guarantee is inferred from the permission declaration alone.
