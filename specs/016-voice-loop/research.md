# Research: Voice-First Home Assistant Loop

**Feature**: 016-voice-loop
**Date**: 2026-08-22

---

## Decision 1: TTS Library

**Decision**: Use `kokoro-tts` 0.3.3 (crates.io)

**Rationale**:
- Pure Rust + ONNX; no Python dependency; cross-compiles to mobile
- Outputs `Vec<f32>` at 24,000 Hz mono — directly compatible with `cpal` playback
- Async (Tokio) with both one-shot (`synth()`) and streaming (`stream()`) APIs
- Released August 2026, actively maintained
- The name `kokoro-rs` in the spec referred to this category of crate; `kokoro-tts` is the canonical library

**Alternatives considered**:
- `kokoro-en` 0.1.5 — English-only, older API, less complete
- `ghchinoy/kokoro-rs` GitHub CLI tool — a binary, not a library; not suitable

**Notes**:
- Requires `ort ^2.0.0-rc.12` (ONNX Runtime); model is a 300 MB ONNX file + voices directory
- Audio output: `Vec<f32>`, 24 kHz mono, range [-1.0, 1.0]
- API: `KokoroTts::new(model_path, voices_dir).await?` → `tts.synth(text, Voice::new("af_heart")).await?`
- Models are NOT bundled; user must download `kokoro-v0_19.onnx` and a `voices/` directory

---

## Decision 2: Wake Word Detection

**Decision**: Use `rustpotter` 3.0.2 (pure Rust, DTW-based, no ONNX)

**Rationale**:
- Zero Python dependency (openwakeword has no stdin/stdout protocol and would require a custom wrapper script plus a Python venv in Nix — significant complexity)
- Does NOT use ort/ONNX — uses `candle-core` + `rustfft` + `rubato` — avoids ort version conflicts with kokoro-tts
- DTW (Dynamic Time Warping) mode creates a custom "Hey Agentix" wake word from 3–8 WAV recordings; no training pipeline needed
- Wake word file is a `.rpw` binary produced by `rustpotter-cli`; can be committed to the repo
- 74k downloads; maintained since 2022

**Alternatives considered**:
- `openwakeword` Python subprocess — no built-in stdin/stdout mode; would need a wrapper script, Python venv, ONNX Python wheels in Nix. Eliminated due to dependency complexity.
- `livekit-wakeword` 0.1.3 — openWakeWord ONNX model compatible, most polished API, but pulls in `ort` and bundled ONNX embeddings. Suitable alternative if rustpotter proves insufficient.
- `oww-rs` 0.3.0 — minimal port of openWakeWord to Rust using `tract-onnx`; less maintained than livekit-wakeword
- Picovoice Porcupine — commercial license required for custom wake words

**Notes**:
- Frame size determined by `rustpotter.get_samples_per_frame()` (varies by config)
- Accepts f32 mono PCM at 16 kHz (resamples internally)
- To create a custom wake word: record 3–8 WAV samples of "Hey Agentix", run `rustpotter-cli create-wakeword --name hey_agentix *.wav -o hey_agentix.rpw`
- The `.rpw` file is committed to the repository under `agentix-voice/wakewords/`
- For initial development, use "alexa" or "hey_home" as placeholder (rustpotter ships a few built-in test models)

---

## Decision 3: Voice Activity Detection (VAD)

**Decision**: RMS energy threshold VAD for v1

**Rationale**:
- `voice_activity_detector` 0.2.1 (Silero VAD) pins `ort =2.0.0-rc.10` exactly, which conflicts with `kokoro-tts`'s `ort ^2.0.0-rc.12`. These two exact versions cannot coexist in the same crate.
- Energy-based RMS is zero-dependency, adequate for a quiet home environment, and straightforward to tune.
- Upgrade path: once `ort` stabilizes (planned for 2.x stable release), replace with Silero VAD via `voice_activity_detector`.

**Algorithm**:
```
threshold: f32 = 0.01  (configurable)
silence_window: Duration = 800ms (configurable)

per 512-sample chunk (32ms at 16kHz):
  rms = sqrt(sum(sample^2) / n)
  if rms > threshold:
    last_speech = now()
  if now() - last_speech > silence_window:
    end_of_utterance()
```

**Alternatives considered**:
- `voice_activity_detector` 0.2.1 (Silero VAD) — correct long-term choice but blocked by ort version conflict
- `wavekat-vad` 0.1.16 — unified trait over WebRTC VAD and Silero backends; still likely has ort dependency

---

## Decision 4: Audio I/O

**Decision**: `cpal` (already in workspace via `agentix-whisper`), with `rubato` for resampling

**Rationale**:
- cpal is already vendored in the Nix flake (agentix-whisper uses it)
- Provides cross-platform microphone capture and audio playback
- For wake word detection, audio must be 16 kHz mono i16; cpal captures at the device's native rate, so `rubato` (also already in workspace) handles resampling
- For TTS playback, kokoro-tts outputs 24 kHz f32; cpal output device will accept f32 samples

**Notes**:
- Capture format: device native → resample to 16 kHz i16 for rustpotter
- After wake word: continue capturing at 16 kHz, apply RMS VAD, accumulate until silence
- Before STT: send raw 16 kHz i16 PCM as WAV multipart to `/v1/audio/transcriptions`
- TTS playback: 24 kHz f32 from kokoro-tts → cpal output stream

---

## Decision 5: Mobile Audio Format for STT Proxy

**Decision**: Accept `audio/webm;codecs=opus` from MediaRecorder and forward to agentix-daemon as-is

**Rationale**:
- Browser MediaRecorder defaults to `audio/webm;codecs=opus` on Chrome/Firefox; Safari uses `audio/mp4;codecs=mp4a.40.2`
- agentix-daemon's whisper endpoint already accepts multipart form data; whisper.cpp handles common audio formats including webm/opus and mp4 via ffmpeg-style format detection
- No server-side transcoding needed for the proxy; just forward the blob with the correct Content-Type

**Notes**:
- The mobile transcribe proxy (`POST /api/voice/transcribe`) accepts multipart/form-data with a `file` field (matching the whisper endpoint contract)
- It forwards verbatim to `POST /v1/audio/transcriptions` on agentix-daemon
- Response: `{"text": "transcribed text"}` — relay this back to the browser

---

## Decision 6: Architecture — agentix-tts Library Crate

**Decision**: Add `agentix-tts` library crate to workspace

**Rationale**:
- Both `agentix-voice` (desktop TTS playback) and `agentix-agent` (mobile synthesize endpoint) need TTS
- Creating a shared library crate follows Constitution Principle I (library-first) and avoids code duplication
- `agentix-tts` wraps `kokoro-tts` with: config-driven initialization, text→f32 synthesis, WAV encoding helper

**Interface**:
```rust
pub struct TtsEngine { /* kokoro-tts KokoroTts + voice config */ }
impl TtsEngine {
    pub async fn new(model_path: &Path, voices_dir: &Path, voice: &str) -> Result<Self>;
    pub async fn synthesize(&self, text: &str) -> Result<Vec<f32>>;
    pub fn sample_rate(&self) -> u32;  // always 24000
    pub fn to_wav(&self, samples: &[f32]) -> Vec<u8>;  // PCM→WAV encoding
}
```

---

## Decision 7: New Crates / Modified Crates

| Crate | Type | Role |
|-------|------|------|
| `agentix-tts` | library | Wraps kokoro-tts; `synthesize(text) → Vec<f32>` + WAV encoding |
| `agentix-voice` | binary | Desktop loop: rustpotter wake word → cpal record → STT → agent → TTS → cpal play |
| `agentix-agent` | binary (modified) | Add `/api/voice/transcribe`, `/api/voice/synthesize` endpoints + mobile mic UI |

`agentix-voice` depends on: `agentix-tts`, `agentix-api`, `cpal`, `rustpotter`, `reqwest`, `rubato`, `tokio`
`agentix-agent` depends on: `agentix-tts` (added)

---

## ort Version Strategy

`kokoro-tts` uses `ort ^2.0.0-rc.12`. This is the only ort consumer in the workspace (agentix-whisper uses whisper-rs which uses its own bundled C++ ONNX runtime, not the Rust ort crate). No conflict.

If `voice_activity_detector` is added later, upgrade the workspace to a common ort version after ort 2.0 stable is released.
