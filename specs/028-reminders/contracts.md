# Proposed Android reminder contract boundary

All routes share the existing daemon listener and owner authorization/WireGuard policy. Bounds, rejection codes and exact JSON fixtures must be pinned in T004 before production code. No existing v1 endpoint changes semantics.

| Route | Purpose | Important guarantees |
|---|---|---|
| `GET /v2/assistant/capabilities` | Advertise supported protocol and reminder/device support | An old server's missing route does not authorize reminder delivery |
| `POST /v2/assistant/tell` | Device-bound turn, carrying the existing request fields plus explicit protocol/device identity | Replay fingerprint includes context; separate typed reminder receipts from note/task/list receipts |
| `POST /v2/assistant/reminders/enroll` | Select the first installation under owner authorization | Reject implicit replacement or multiple destinations |
| `GET /v2/assistant/reminders/snapshot` | Fetch a complete bounded view for the enrolled device | Backend/device identity and snapshot sequence validated before applying; no silent truncation |
| `POST /v2/assistant/reminders/report` | Acknowledge exact revision/capability/alarm/post/cancellation observations | Stable report ID, duplicate replay and monotonic revision checks; no model involved |

An accepted reminder turn means durable backend intent. The updated app fetches/applies the sync snapshot and arms alarms before presenting phone-specific success. If it cannot arm, show saved-awaiting-phone or permission-blocked state, preserving the pending operation/effect identity needed for recovery. Client acknowledgements do not rewrite the original accepted turn result.

New app compatibility: preserve any pre-upgrade pending v1 envelope and drain it through v1 unchanged. Negotiate v2 for newly admitted turns; do not mutate a pending request into a new protocol/device context. An old backend remains usable for its existing capabilities while reminders stay unavailable. An old app continues to use v1 with unchanged record kinds and response validation.

Reminder synchronization is authoritative only when a full response passes schema, namespace, sequence, size and revision validation. Use bounded text/count/body limits and capacity errors rather than partial snapshots. A failed request or changed endpoint never clears existing alarms as if a valid empty snapshot had arrived. An explicit destination switch is a separate user-visible operation.

Versioned persistence and rollback behavior must be documented before live scheduling. No successful HTTP response or stale device observation can be used to claim the owner has actually seen a reminder.
