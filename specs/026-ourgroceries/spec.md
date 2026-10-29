# Feature Specification: Existing OurGroceries list

**Branch**: `026-ourgroceries`
**Created**: 2026-10-29
**Status**: Specified
**Input**: Native Rust integration with the owner's existing OurGroceries list, potentially the only grocery storage.

## User scenarios and acceptance

### US1 — Read the existing list (P1)

Given runtime credentials and an explicitly selected existing list, asking for groceries reads current remote items and names the remote source. Another household member's edits appear on the next read. No local grocery mirror or implicit migration is created. List discovery is read-only and supports choosing an opaque list ID. Offline/auth/protocol errors produce an unavailable response with no local fallback.

### US2 — Add, correct and check off groceries (P1)

Given that list, ordinary assistant instructions add an item, change a selected item's text, mark it purchased or reopen it. Item identities must come from the current remote snapshot; ambiguous duplicates require clarification. Renames preserve category and other fields; completion sets an explicit boolean. Single-item deletion is explicit; no list creation, list deletion or bulk clearing. Existing local notes/tasks/other lists keep working.

### US3 — Safe uncertain outcomes (P1)

Given an admitted remote mutation, its immutable plan and dispatch marker are flushed before sending. Identical phone retries/restarts never blindly repeat a dispatched mutation. Confirmed results replay exactly. A lost response/crash produces an explicit durable uncertain result that tells the owner to inspect the remote list before repeating. The phone may then perform a fresh read; it must not remain permanently blocked by an unresolvable pending mutation. A new deliberate instruction has a new request ID and may produce a new action.

## Requirements

- FR-001: Native Rust HTTP adapter library, no Python runtime/service. Follow the referenced unofficial protocol with upstream attribution. Use TLS validation and strict origin restrictions; credentials/cookies never enter model context, Git, Nix outputs or logs.
- FR-002: Configure credentials via runtime environment-variable names and select an existing list ID. Disabled by default; no account access merely from saving Android settings. Provide read-only list discovery using the same Rust adapter.
- FR-003: Remote groceries are authoritative. Do not duplicate them as local Record/ListItem objects or fabricate local CommandReceipts. Existing local grocery data is retained without automatic import/deletion.
- FR-004: Fresh bounded snapshots feed explicit grocery tool schemas. Rust checks configured list/item IDs and stale target snapshots, preserves unrelated fields, and prevents routing grocery requests into a local fallback when remote groceries are enabled.
- FR-005: Single-attempt remote writes with bounded requests/responses/timeouts, no automatic mutation retries, including authentication retries or redirects. Successful write replies require a successful RPC and readback of the intended state. Read failures after dispatch are uncertain.
- FR-006: Durable remote journal states distinguish prepared and dispatched. Only the transition into dispatched authorizes one send; replay of dispatched plans cannot send again. Store final outcome and assistant message atomically before returning.
- FR-007: Preserve the deployed v1 Android contract. Remote acknowledgements use Answer replies explicitly naming OurGroceries, with zero local outcomes/records; uncertain outcomes are also definitive honest answers. Existing local save statuses/receipt validation remain intact.
- FR-008: OurGroceries offers no demonstrated idempotency key or compare-and-swap in the reference. Do not claim exactly-once execution or atomic protection against concurrent household edits. Fresh preflight validation and field-specific operations reduce stale writes; report uncertainty honestly.
- FR-009: Real-Sled tests cover prepare/dispatch/finish/reopen/replay, crash windows and no duplicate remote calls. HTTP fixtures exercise login/cookies/team parsing/RPC/readback and malformed/auth/timeout/redirect/error handling. Existing assistant/API tests must still pass.
- FR-010: Canonical Nix packages/checks and deployment instructions are required. Live account testing requires runtime credentials and explicit selected list; do not request passwords in chat or mutate the owner's remote list as incidental testing.

## Success criteria

Synthetic end-to-end assistant instructions read/add/rename/check/reopen the fixture remote list, preserve unrelated fields and never create local grocery records. Original-envelope retries and restart recovery do not duplicate external mutations. The existing Android v1 response validator accepts remote confirmed/uncertain replies. Credentials/errors remain redacted. Real account acceptance is reported separately from fixtures.
