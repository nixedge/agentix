# Feature Specification: Offline Kokoro voice options

**Branch**:031-kokoro-voices · **Date**:2026-11-01 · **Status**: implementation

Owner wants British, measured speech reminiscent of Captain Picard and explicitly selected Kokoro's stock `bm_george` and `bm_daniel` as voice options. Clarified: **run on the phone, offline, accepting a larger app**. These are named stock voices, not a custom actor clone. Reply wording/personality is unchanged.

## User stories and acceptance

### US1 — Choose and preview either voice (P1)

Given the assistant conversation screen, when I select Kokoro George or Kokoro Daniel, then Preview voice and subsequent spoken replies use that exact stock voice locally. The choice persists after reopening and process restart. Existing Android offline voices remain selectable and current preferences migrate without resetting them. A voice switch stops previous output; selected-but-failed Kokoro does not silently change voice or call a server.

### US2 — Control speech safely (P1)

Given Kokoro initializing, synthesizing or playing, when I press Stop speaking, disable spoken replies, switch voice, background/lock the assistant or begin another turn, then no stale output starts or continues beyond the bounded audio buffer. Synthesis/copy/playback/native cleanup happen off the UI thread. Wake detection pauses until Kokoro has finished or cancelled its actual output, then rearms. Cancelling speech must not cancel or replay a backend mutation.

### US3 — Understand availability (P2)

The packaged model and voices work without network, API keys, backend deployment or runtime downloads. Missing native ABI/model, corrupt local model copy, synthesis/audio failure and unsupported locale produce honest visible errors; typed replies and Android voices stay available. The first preview can initialize/cache the model and may be slower.

## Requirements

- FR-001: Offer exactly the requested Kokoro IDs by name alongside existing Android speech; persist the provider/voice separately from Android profile/installed voice selection.
- FR-002: Bundle a hash/revision-pinned local model and matching pinned native SDK through Nix/Gradle snapshot. No Python/runtime cloud endpoint. Use the full local pronunciation frontend rather than silently omitting unknown words or reading raw letters as phonemes.
- FR-003: Preview and replies share the same selected voice, British locale and measured0.95 speaking rate. Do not apply Android pitch controls to Kokoro or claim actor identity/voice quality from metadata alone.
- FR-004: Use bounded Unicode-safe text chunks, finite PCM buffers, serialized synthesis and generation-aware cancellation; no command text/audio written to logs or speech files.
- FR-005: Integrate wake holds, audio focus and lifecycle cleanup. Reject callbacks/output from previous utterances and release native resources without blocking main thread or racing an in-flight native call.
- FR-006: Keep existing commands, pending identities, alarms/snooze/Inbox/Archive, recording, Gemini handoff and settings intact. No backend protocol/deployment change.

## Edge cases

Stop during first asset copy or model initialization; repeated Preview/voice changes; foreground loss while synthesis waits for wake release; focus denied/lost during audio; unsupported native ABI; corrupt/truncated cached model; non-English/Unicode/emoji/names/numbers in text; very long or empty replies; cold model and application process recreation.

## Success criteria

- SC-001: Canonical Nix APK build passes core policy tests, lint and assembly with pinned dependency snapshot and packaged model/native/license inspection, same-key downstream signing and16KiB alignment.
- SC-002: Real local native synthesis produces non-empty finite24kHz audio for both exact voice IDs; retain sample/evidence without treating it as phone latency evidence.
- SC-003: On Pixel, Preview both voices in airplane mode, choose preferred sound, verify persistence/reply output, Stop/lifecycle/focus/wake rearming and preserved app data. Record cold/warm latency and memory; no synthetic/build-only naturalness claim.
