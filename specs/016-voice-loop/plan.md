# Implementation Plan: Voice-First Home Assistant Loop

**Branch**: `016-voice-loop` | **Date**: 2026-08-22 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/016-voice-loop/spec.md`

## Summary

Add a voice-first interaction loop to agentix: a desktop daemon (`agentix-voice`) that continuously listens for the wake phrase "Hey Agentix", records the user's speech, transcribes it via agentix-daemon's whisper endpoint, queries agentix-agent, synthesizes the response with Kokoro TTS, and plays it on the speaker. Mobile voice input is added to the agentix-agent web UI as a mic button, with optional TTS playback. A shared `agentix-tts` library crate wraps `kokoro-tts` for use by both `agentix-voice` and the new `agentix-agent` synthesis endpoint.

## Technical Context

**Language/Version**: Rust 2021 (workspace standard, fenix stable toolchain)
**New Dependencies**:
- `kokoro-tts 0.3.3` — Kokoro TTS via ONNX Runtime (`ort ^2.0.0-rc.12`), 24 kHz mono f32 output
- `rustpotter 3.0.2` — Pure-Rust wake word detection (DTW, custom `.rpw` model); no ONNX, uses `candle-core`
- `cpal` — Already in workspace (agentix-whisper); microphone capture + audio playback
- `rubato` — Already in workspace (agentix-whisper); sample rate resampling (mic native rate → 16 kHz)
- `reqwest` — HTTP client for agentix-voice to call daemon/agent; already used in agentix-agent
- `hound` — WAV encoding (lightweight, no C++ dep); for `agentix-tts` `to_wav()` helper

**New Crates**:
- `agentix-tts` — library crate; wraps `kokoro-tts`; exposes `TtsEngine::new()`, `synthesize()`, `to_wav()`
- `agentix-voice` — binary crate; desktop voice loop; depends on `agentix-tts`, `agentix-api`, `cpal`, `rustpotter`, `rubato`, `reqwest`, `hound`

**Modified Crates**:
- `agentix-agent` — add `agentix-tts` dep; new voice HTTP routes; mobile UI changes

**Storage**: N/A — voice turns are ephemeral; config in existing TOML files
**Testing**: `cargo test -p agentix-tts` (unit); `cargo test -p agentix-voice` (unit); integration tests for new agentix-agent HTTP endpoints
**Target Platform**: Linux desktop (NixOS, x86_64) + mobile browser (any modern WebKit/Blink)
**Performance Goals**: Full desktop voice turn < 8s; TTS synthesis < 2s for 50-word response
**Constraints**: Fully offline; no internet required at runtime; Kokoro model 300 MB (user-provided, not bundled)
**Scale/Scope**: Single-user home assistant; no concurrent request handling required for agentix-voice

## Constitution Check

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ PASS | `agentix-tts` is a library crate (no port binding). `agentix-voice` is a daemon/binary. New crates have clear, non-overlapping purposes. |
| II. Local-First | ✅ PASS | All inference (STT, agent, TTS) is local. No cloud escalation path needed. |
| III. Reproducible | ✅ PASS | Both new crates added as Nix package attributes. `kokoro-tts` pinned in `Cargo.lock`. Kokoro model is user-provided (too large to pin). |
| IV. Isolation | ✅ PASS | `agentix-voice` is a system service, not a jailed agent. It has no credentials to protect; communicates only with localhost. |
| V. Layered API | ✅ PASS | Voice endpoint types stay in `agentix-agent` (agent-specific, not OpenAI-compat). `agentix-tts` has no HTTP surface. |
| VI. Testing | ✅ PASS | `agentix-tts` unit tests (synthesize produces non-empty Vec<f32>). Integration tests for `/api/voice/transcribe` and `/api/voice/synthesize` using mock audio. agentix-voice unit tests for VAD and state machine logic. |
| VII. Formal State Machine | ✅ PASS | `agentix-voice` voice loop uses a typed `VoiceState` enum with exhaustive `match`. Defined in data-model.md. |
| VIII. Code Quality | ✅ PASS | fmt + clippy + test + `nix build .#agentix-voice` required before merge. |

## Project Structure

### Documentation (this feature)

```text
specs/016-voice-loop/
├── plan.md              # This file
├── research.md          # Technology decisions (kokoro-tts, rustpotter, RMS VAD)
├── data-model.md        # VoiceState machine, config structs, VoiceTurn entity
├── quickstart.md        # Setup + 6 validation tests
├── contracts/
│   └── http-api.md      # POST /api/voice/transcribe, POST /api/voice/synthesize
└── tasks.md             # Phase 2 output (speckit.tasks)
```

### Source Code

```text
agentix-tts/
├── Cargo.toml           # kokoro-tts, hound, tokio
└── src/
    └── lib.rs           # TtsEngine: new(), synthesize(), to_wav(), sample_rate()

agentix-voice/
├── Cargo.toml           # agentix-tts, agentix-api, cpal, rustpotter, rubato, reqwest, tokio
├── wakewords/
│   └── placeholder.rpw  # placeholder rustpotter model for development
└── src/
    ├── main.rs          # CLI arg parsing, config loading, spawn voice loop
    ├── config.rs        # VoiceConfig, WakeWordConfig, TtsConfig deserialization
    ├── state.rs         # VoiceState enum + state machine transitions
    ├── audio.rs         # cpal microphone capture, playback helpers
    ├── vad.rs           # RMS energy VAD
    ├── wake.rs          # rustpotter wrapper (load .rpw, per-frame detection)
    └── loop.rs          # main voice loop: orchestrates wake→record→STT→agent→TTS→play

agentix-agent/src/
├── config.rs            # Add VoiceAgentConfig block (tts, daemon_url)
├── voice/
│   ├── mod.rs           # route registration (POST /api/voice/transcribe, /synthesize)
│   ├── transcribe.rs    # proxy to agentix-daemon /v1/audio/transcriptions
│   └── synthesize.rs    # call agentix-tts TtsEngine, return WAV bytes
└── static/
    └── index.html       # Add mic button (P2) + speaker button (P3)
```

**Structure Decision**: Multi-crate workspace extension following existing patterns. `agentix-tts` is a new leaf library (no deps on other agentix crates). `agentix-voice` is a new binary that depends on `agentix-tts` and `agentix-api`. `agentix-agent` gains a new `voice/` module and a dep on `agentix-tts`.

## Complexity Tracking

> No constitution violations. No entries required.
