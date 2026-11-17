# Data model

`RemoteRecording(sessionId:u32,size:u32,scene:u16)` requires positive80-byte-aligned size bounded to256MiB and a nonzero session.

`RecorderKey`: SHA256 of normalized BLE address, truncated to128bits for safe deterministic filenames. Token is stored separately in backup-excluded private preferences and is never part of filenames.

`Download`: `plaud-partials/<key>-<session>-<size>.part`; complete89-byte wire records only. Each has matching session, strictly contiguous compressed-byte offset, supported length and Opus frame duration. Partial trailing record from process interruption is truncated before resume. Full records are rescanned on reopen, including checksums. A protocol/checksum error does not publish an import.

`ImportedRecording`: `recordings/<session*1000>-plaud-<key>-<size>.ogg`. Duration derives from declared size/80*20ms encoded in the filename; Ogg granule positions also represent this duration. Final names are deterministic and only become visible via atomic rename after completed transfer, checksum acknowledgement and fsync. Files persist through archive actions. WAV partial recovery remains exclusive to WAV recordings.

Sync state: idle→connecting→authenticating→listing→downloading→publishing→complete, with cancellation/error closing GATT. A controller permits one operation at a time; screen/activity disposal cancels it. Errors retain imported files and recoverable partials.

## Server and upload extension

`RecordingMetadata`: UUID deterministically derived on Android from the published filename,32hex device key,u32 session,source raw bytes,recordedAt,duration,audio length,SHA256 and media type. The immutable envelope is validated on the server, including source/time/duration consistency and bounds.

`RecordingArchive`: existing personal Sled DB keys `audio/recording/<uuid>`, `audio/source/<device>/<session>` and `audio/transcript/<uuid>`; server identity at `meta/recordings-server-id`. Original files at configured recordings root `blobs/<hash-prefix>/<sha256>.ogg`, published/fsynced before durable metadata. New Unix directories/files use0700/0600. A replay validates immutable identity and original hash; source reuse or changed metadata is409. Metadata receipts flush Sled before acknowledgement.

`StoredRecording`: immutable metadata,storedAt,transcription attempts/retry deadline/error,transcribedAt and deterministic transcript Note IDs. Full text and declared local gateway-model route are retained separately. Transcription completion atomically writes full text, UTF8-bounded Note chunks, projection jobs and completed state. Search pending derives from actual projection keys. Full transcript limit2MiB; audio64MiB/four hours.

`RecordingUploadJournal`: phone-private atomic version2 binary entries per deterministic recording UUID, containing immutable metadata/filename/destination, optional confirmed receipt and independently retained expected server identity. A lost response retains the original pending envelope. Current Settings cannot redirect it. Acknowledged files use metadata GET to refresh transcription/search status; missing server originals can be re-uploaded with the same identity. No automatic local deletion.
