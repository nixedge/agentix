# Reminder state and identity

## Backend

`Reminder` has a UUID, revision, text (nonblank, at most 1024 UTF-8 bytes), resolved UTC deadline, original IANA time zone and desired active/cancelled state. The associated immutable turn retains its accepted request instant; the containing reminder book binds the backend installation and selected device. Exact query and due-index entries are authoritative; there is no search projection requirement.

`ReminderReceipt` binds request ID, payload/device fingerprint and command to its exact resulting revision. Apply expected-revision checks, reminder write, due-index update, sync sequence/tombstone and receipt in one atomic batch and flush before returning success. Duplicate identical requests replay; changed context conflicts. Time proposals distinguish relative duration from local date/time; deterministic resolution uses accepted time and validates local-time ambiguity.

`DeviceObservation` binds device, reminder and revision to reported armed/blocked/post-pending/posted/uncertain/cancel-applied state, observation ID and sequence. The backend separately records its receipt time for the latest observation. Reports are idempotent and monotonic within the same revision; old reports cannot roll back new intent or revive cancelled work. Posted is terminal at that revision and means an Android notification API call returned successfully, not proof the owner saw it. The phone coalesces pending observations to its latest state per reminder; this is not a complete historical audit of every local attempt.

`DeviceEnrollment` binds one selected installation to the backend instance. Installation IDs are routing keys under existing owner authorization, not independent credentials. A new installation is not silently enrolled alongside an existing one.

`SnapshotSequence` orders complete synchronization snapshots. Tombstones stay until applied by the enrolled phone. Snapshot contents include current desired state and the device's reported outcome for that revision, so a fresh cache does not treat already-posted work as unconditionally pending. Enrollment/reset recovery must be explicit.

## Android

The private atomic journal contains a schema version, backend/device namespace, last applied snapshot sequence, reminder revisions/tombstones, alarm intent, post intent/result and pending device reports. Unknown/malformed persisted data is surfaced as recovery-needed; it is not replaced with an empty schedule.

Alarm lifecycle: desired -> alarm-intent-persisted -> armed, or blocked. Post lifecycle: due/current revision -> post-intent-persisted -> posted, or uncertain/blocked. Persist intent before each OS effect; reconcile interrupted steps with the same PendingIntent/notification identity. Cancel-applied and newer revision checks suppress stale receivers. Use one journal gate to serialize sync, cancellation and notification admission; keep network outside that gate.

PendingIntent identity includes namespace, reminder ID and revision; it is immutable and targets a non-exported receiver. Notification identity is stable per reminder, preventing separate duplicate tray entries on a retry. An update cancels the old alarm; receiver validation also rejects an old revision that was already queued for delivery.

No SQLite/Room database or new Rust crate is introduced. The private backend book and Android journal use independent version-1 schemas. See [wire types](wire-contract.md) and [persistence/rollback](quickstart.md#persistence-and-rollback); note/task/list record kinds stay unchanged. Cancellation tombstones and idempotent report receipts are retained without automatic pruning in this increment.
