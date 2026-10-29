# Data model

## Endpoint

A normalized HTTPS base URI with a host and optional base path/port; no username/password, query, or fragment. Empty value means unconfigured. Stored in app-private SharedPreferences and excluded from Android backup.

## Recorder advertisement

Transient address-keyed discovery result: advertised name, RSSI, optional project code. Parser accepts missing/truncated manufacturer data. No binding, compatibility, recording, or upload state is inferred.

## Scan state

Idle → Scanning → Finished, or Error with a retry. Scan is stopped on timeout, leaving Devices, or activity backgrounding. Results are in memory only.

## Phone recording (scope extension)

Each note is a 16 kHz/mono/16-bit PCM WAV under private `files/recordings/`. File names combine capture-start epoch milliseconds and a UUID. Active capture uses `.wav.part`; clean stop writes the WAV header, synchronizes, and renames to `.wav`. Orphaned partial files are recovered as `.recovered.wav`, preserving complete PCM frames. Completed files provide timestamps, duration derived from data length, and source Phone. No relational database or audio metadata service is required.

Capture phases: Idle → Starting → Recording → Stopping → Idle, with a user-visible completion/error message. The microphone service returns START_NOT_STICKY. A volatile active file identity prevents Inbox recovery from modifying a live capture. Playback stops when the activity is backgrounded or recording begins.

Upload jobs, server credentials, extracted tasks, and assistant conversations remain deferred until their contracts and storage requirements are defined.

## Ephemeral assistant turn

TellCommand contains one nonblank text instruction. AssistantClient.tell returns an AssistantReply; the Android-only implementation returns Unavailable with no network calls or execution claim. Turn states: Idle, Listening, Processing, Ready, Error. Each turn receives a monotonically increasing identity. Partial text is accepted only for the current Listening turn; a final transcript transitions once to Processing; a reply transitions Processing to Ready. Cancellation/new turn/background invalidates old identities, so delayed recognizer/backend callbacks have no effect. Transcript, response and errors live only in activity memory. The sole new persisted preference is whether to speak replies.
