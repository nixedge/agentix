# Data Model: Voice-First Home Assistant Loop

**Feature**: 016-voice-loop
**Date**: 2026-08-22

---

## Entities

### VoiceTurn (in-memory, not persisted)

Represents a single completed voice interaction cycle. Lives only in the `agentix-voice` process log; not stored to disk.

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID | Unique ID for this turn |
| `started_at` | DateTime\<Utc\> | Timestamp of wake word detection |
| `transcription` | String | Text output from STT |
| `response` | String | Agent's text reply |
| `tts_duration_ms` | u64 | Time spent in TTS synthesis |
| `total_duration_ms` | u64 | Wake word → playback start |

**State transitions**:
```
Idle → WakeWordDetected → Recording → Transcribing → AgentThinking → Synthesizing → Playing → Idle
```

Invalid transitions are rejected by the state machine's exhaustive `match` (Principle VII).

---

### TtsConfig (config file)

Persisted in `agentix-agent`'s and `agentix-voice`'s shared config TOML.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model_path` | PathBuf | (required) | Path to `kokoro-v0_19.onnx` |
| `voices_dir` | PathBuf | (required) | Directory containing voice `.bin` files |
| `voice` | String | `"af_heart"` | Kokoro voice name |
| `speaking_rate` | f32 | `1.0` | Playback speed multiplier |
| `max_text_len` | usize | `500` | Chars; longer responses truncated before synthesis |

Validation: `model_path` and `voices_dir` must exist at startup; startup MUST fail with a clear error message if they are missing.

---

### WakeWordConfig (config file)

Part of `agentix-voice`'s config TOML.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model_path` | PathBuf | `"wakewords/hey_agentix.rpw"` | Path to rustpotter `.rpw` file |
| `threshold` | f32 | `0.5` | Detection confidence threshold [0.0, 1.0] |
| `avg_threshold` | f32 | `0.2` | Rustpotter avg threshold |

---

### VoiceConfig (config file, agentix-voice)

Top-level config for the desktop voice daemon, in TOML.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `agent_url` | String | `"http://localhost:8765"` | agentix-agent base URL |
| `daemon_url` | String | `"http://localhost:8080"` | agentix-daemon base URL (for STT) |
| `tts` | TtsConfig | — | TTS configuration block |
| `wake_word` | WakeWordConfig | — | Wake word configuration block |
| `vad_threshold` | f32 | `0.01` | RMS silence threshold |
| `vad_silence_ms` | u64 | `800` | Silence duration before end-of-utterance |
| `session_id` | Option\<String\> | `None` | Persistent chat session ID for agentix-agent |

---

### VoiceAgentConfig (config file, agentix-agent)

Block added to agentix-agent's existing `Config` struct.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tts` | Option\<TtsConfig\> | `None` | TTS block; voice endpoints return 503 if absent |
| `daemon_url` | String | `"http://localhost:8080"` | agentix-daemon URL for STT proxy |

---

## State Machine (VoiceTurn lifecycle)

```
         ┌──────────────────────────────────────────────────────┐
         │                                                      │
  ┌──────▼──────┐                                       ┌──────┴──────┐
  │    Idle     │◄──────────────── done ────────────────│   Playing   │
  └──────┬──────┘                                       └──────▲──────┘
         │ wake word                                           │
  ┌──────▼──────┐                                    ┌─────────┴────────┐
  │WakeDetected │                                    │   Synthesizing   │
  └──────┬──────┘                                    └─────────▲────────┘
         │ start rec                                           │
  ┌──────▼──────┐                                    ┌─────────┴────────┐
  │  Recording  │──── end-of-utterance ──────────────│ AgentThinking    │
  └──────┬──────┘                                    └─────────▲────────┘
         │                                                     │
  ┌──────▼──────┐                                             │
  │Transcribing │─────────── transcription text ─────────────┘
  └─────────────┘
```

Error from any state → back to Idle (error is spoken if TTS available, otherwise logged).
