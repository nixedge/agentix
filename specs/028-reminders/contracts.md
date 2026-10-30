# Android reminder contract boundary

All routes share the existing daemon listener, `/v1/assistant/` prefix and owner authorization/WireGuard policy. The owner confirmed nothing is in production and rejected parallel compatibility routes. Backend and Android update together; there is one current wire contract, without negotiation or legacy-body fallback.

| Route | Purpose | Important guarantees |
|---|---|---|
| `POST /v1/assistant/tell` | All assistant turns, with optional enrolled device identity | Immutable request/device context; separate reminder and record receipts |
| `POST /v1/assistant/reminders/enroll` | Select the first installation under owner authorization | Reject implicit replacement or multiple destinations |
| `GET /v1/assistant/reminders/snapshot` | Complete bounded view for the enrolled device | Validate namespace, sequence and revisions before applying |
| `POST /v1/assistant/reminders/report` | Report exact revision/alarm/post/cancellation observations | Stable report identity and monotonic revision checks |

Every tell uses `{device_id: UUID|null, request: TellRequest}` and returns `{device_id: UUID|null, response: TellResponse, reminder_receipts: [...]}`. A null device supports ordinary notes/tasks/lists/groceries. A bound enrolled device additionally enables reminder operations. Enrollment is explicit; it does not occur merely because an app submits an instruction. Changing device context on a reused request ID conflicts.

Accepted reminder intent is durable backend state. The app fetches/applies the snapshot and arms alarms before presenting phone-specific success. Failure to arm remains blocked/pending, preserving request/effect identity. Reports do not rewrite the original turn result. HTTP success or a stale observation never proves the owner saw a notification.

A snapshot is authoritative only after full schema, namespace, sequence, size and revision validation. A failed or truncated response never clears existing alarms. Destination replacement is a separate deliberate operation. Private persistence remains versioned and malformed data is refused; there is no automatic data reset or unrelated user-data migration.
