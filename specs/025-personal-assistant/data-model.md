# Personal assistant data model

## Foundation records

`Record { schema_version: 1, id: UUID, revision: u64, created_at, updated_at, deleted, body }`.

Bodies: Note{text}, Task{title, completed, due_at?}, List{name}, ListItem{list_id,text,completed}. All mutable text is bounded and nonblank. References use IDs rather than names; ambiguous natural-language matching is outside storage. Dates are UTC instants; a later interpretation layer retains the owner's timezone and original utterance. A record cannot change kind through Replace. Revisions increase on Replace/Delete; tombstones are retained.

`CommandRequest { request_id, command }` with commands Create{id,body}, Replace{id,expected_revision,body}, Delete{id,expected_revision}. `CommandReceipt { request_id, command, record }` preserves the original outcome. No idempotency expiration in A; pruning requires an explicit contract later.

`ProjectionJob { record_id, revision }`: one pending entry per entity, coalesced on each mutation. Worker acknowledgement checks both fields; acknowledging an older job cannot remove newer work. Projection documents carry original ID/revision and schema/embedding profile metadata. Search hits rehydrate originals and reject mismatched revisions/deletions.

## Key layout (Sled adapter private)

- meta/schema → version marker.
- record/<UUID> → serialized envelope, including tombstones.
- request/<UUID> → serialized command receipt.
- member/<list UUID>/<item UUID> → membership marker for live list items.
- projection/<record UUID> → latest pending revision.

All writes for a command are one atomic batch under the repository write boundary, followed by a flush. Point reads and scans share that boundary so no public operation observes a partially constructed snapshot. Raw key access is never public. Runtime Sled I/O executes outside the Tokio worker pool.

## Later entities

Conversation messages are separate append-only records with session identity and server-assigned ordering; do not keep replacing a whole conversation array. Facts carry assertion source, provenance and validity rather than treating model guesses as confirmed user facts. Reminder jobs have due time, target record revision, delivery identity and attempt state. Attachments have content checksum, media type and path reference; original bytes stay in private file storage. These schemas are introduced with their own acceptance tests in C/D, not populated with placeholder data in A.
