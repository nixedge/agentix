# Speech output state

Private `assistant_voice` preferences gain `speech_output`: `android`, `kokoro:bm_george` or `kokoro:bm_daniel`. Missing/unknown values resolve to Android. Existing Android profile, engine identity and voice name keys remain independent. No backend/journal migration.

The facade owns Android and Kokoro adapters; only the selected adapter reports speech status. Kokoro is lazy: one serialized worker owns native model initialization, synthesis, AudioTrack playback and destruction. One replaceable queued utterance bounds pending work. Atomic generation identity invalidates stopped/replaced requests; an old completion or focus callback cannot stop or report success for a newer utterance.

Each utterance owns a wake hold. Stop releases an unsubmitted hold immediately; submitted work releases it only after worker audio/focus cleanup. Nonblocking writes use finite slices with a five-second stalled-output deadline. Native cancel uses a nonblocking read lock to avoid racing worker-side native destruction or blocking the UI. Native close is queued after in-flight work.

The pinned asset manifest contains model filename, revision, byte count and SHA256. A process mutex serializes atomic installation into `noBackupFilesDir/kokoro/<revision>/kokoro_int8_fp16.litertlm`. Existing copies are hash-checked; cancelled/failed copies roll back. Cache lives separately under app cache storage. Input text and generated PCM remain in memory; the optional standalone smoke fixture writes only its fixed synthetic samples.
