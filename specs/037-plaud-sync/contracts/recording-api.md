# Recording API v1

Same owner authentication as assistant HTTP; optional bearer only when configured. All routes use existing `/v1/assistant/` prefix and listener. No credentials in metadata or logs.

`PUT /v1/assistant/recordings/{uuid}` sends raw `audio/ogg`, maximum64MiB, with compact JSON `X-Agentix-Recording-Metadata` (maximum4096bytes):

`{id,source_device,source_session,source_bytes,recorded_at,duration_ms,audio_bytes,sha256,media_type}`.

Source device is the existing32hex private recorder key; session is u32 and recorded_at its Unix timestamp. Source bytes are80-aligned; duration=source_bytes/80*20ms, maximum4hours. SHA256 is lowercase64hex of the exact standard audio file; audio_bytes matches body length. Server verifies Ogg page CRCs, mono Opus header, complete stream/duration and immutable source identity before durable acceptance.

PUT/GET metadata return `{server_id,search_pending,recording:{metadata,stored_at,transcription_attempts,retry_at,transcription_error,transcript_record_ids,transcribed_at}}`. These receipts exclude full transcript text and are bounded. Same ID+metadata/body replays; different immutable identity or device/session reuse returns409.400invalid,401auth,404unknown,413oversize,503storage/uncertainty. A successful receipt means durable original audio+metadata, independently of transcription/search progress. Client pins pending ID/hash/metadata/endpoint and validates echoed identity before resolving.

`GET /v1/assistant/recordings/{id}` fetches current metadata/status. `GET .../{id}/audio` returns the original authenticated audio; `GET .../{id}/transcript` returns persisted full text and local-model provenance when ready, otherwise409. Projection readiness is separate from transcription completion; existing projection jobs retry/rebuild.

After explicit upload opt-in, the phone's manual **Sync to server** retries retained work over the externally managed WireGuard route. It never redirects, auto-deletes audio, or silently moves an unresolved upload to a different endpoint.
