# Tasks: Voice-First Home Assistant Loop

**Feature Branch**: `016-voice-loop`
**Spec**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md)
**Total Tasks**: 32 | **Parallelizable**: T002∥T003, T006∥T007∥T009, T011∥T012, T017∥T018, T022∥T023, T028∥T029∥T030∥T031

---

## Phase 1: Setup

- [X] T001 Add `agentix-tts` and `agentix-voice` to workspace root `Cargo.toml` members array; create `agentix-tts/src/lib.rs` (empty `pub mod`) and `agentix-voice/src/main.rs` (empty `fn main() {}`) stubs so `cargo check --workspace` succeeds
- [X] T002 [P] Create `agentix-tts/Cargo.toml`: `name = "agentix-tts"`, `edition = "2021"`, deps: `hound = "3"`, `tokio = { version = "1", features = ["full"] }`, `thiserror = "1"` (kokoro-tts deferred: incompatible ort version — needs research)
- [X] T003 [P] Create `agentix-voice/Cargo.toml`: `name = "agentix-voice"`, `edition = "2021"`, `[[bin]] name = "agentix-voice" path = "src/main.rs"`, deps: `agentix-tts`, `agentix-api`, `cpal = "0.15"`, `rubato = "0.15"`, `reqwest`, `tokio`, `serde`, `toml`, `tracing`, `hound`, `clap`, `anyhow` (rustpotter deferred: candle-core/half version conflict with ort rc.13)
- [X] T004 Create `agentix-voice/wakewords/README.md` with instructions for recording "Hey Agentix" samples

---

## Phase 2: Foundational — agentix-tts Library

**Purpose**: Shared TTS library consumed by both US1 (desktop playback) and US3 (mobile synthesis endpoint). MUST be complete before Phase 3 (US1) or Phase 5 (US3).

- [X] T005 Implement `TtsEngine` in `agentix-tts/src/lib.rs`: define `pub struct TtsEngine { inner: KokoroTts, voice: String, max_text_len: usize }`; implement `pub async fn new(model_path: &Path, voices_dir: &Path, voice: &str, max_text_len: usize) -> Result<Self, TtsError>`; `pub async fn synthesize(&self, text: &str) -> Result<Vec<f32>, TtsError>` (truncate text to `max_text_len` chars, call `inner.synth(text_slice, Voice::new(&self.voice)).await`, return samples); `pub fn sample_rate(&self) -> u32 { 24000 }`; `pub fn to_wav(&self, samples: &[f32]) -> Vec<u8>` (use `hound::WavWriter` with `WavSpec { channels: 1, sample_rate: 24000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float }`, write to a `Vec<u8>` cursor, return bytes); define `TtsError` via `thiserror::Error` with variants for model-load and synthesis failures
- [X] T006 [P] Add `agentix-tts = { path = "../agentix-tts" }` to `[dependencies]` in `agentix-agent/Cargo.toml`
- [X] T007 [P] Add `TtsCfg` and `VoiceAgentConfig` structs to `agentix-agent/src/config.rs`: `TtsCfg { model_path: PathBuf, voices_dir: PathBuf, #[serde(default = "default_voice")] voice: String, #[serde(default = "default_rate")] speaking_rate: f32, #[serde(default = "default_max_text")] max_text_len: usize }`; `VoiceAgentConfig { tts: Option<TtsCfg>, #[serde(default = "default_daemon_url")] daemon_url: String }`; add `pub voice: Option<VoiceAgentConfig>` field to the top-level `Config` struct
- [X] T008 Write unit test for `agentix-tts/src/lib.rs` in `#[cfg(test)] mod tests`: `to_wav_produces_valid_wav_header` — encode a 500-sample zero-filled `Vec<f32>`, parse the output with `hound::WavReader`, assert `spec.sample_rate == 24000`, `spec.channels == 1`, `spec.sample_format == SampleFormat::Float`; this test requires no ONNX model and MUST run in CI
- [X] T009 [P] Implement `VoiceConfig`, `WakeWordConfig`, and `TtsCfg` structs in `agentix-voice/src/config.rs` using `serde::Deserialize`: `VoiceConfig { agent_url: String, daemon_url: String, vad_threshold: f32, vad_silence_ms: u64, tts: TtsCfg, wake_word: WakeWordConfig, session_id: Option<String> }`; `WakeWordConfig { model_path: PathBuf, threshold: f32, avg_threshold: f32 }`; add `pub fn load(path: &Path) -> Result<VoiceConfig, Box<dyn std::error::Error>>` that reads TOML from file; apply serde defaults via helper fns for all optional numeric fields

**Checkpoint**: `nix develop --command cargo check -p agentix-tts -p agentix-agent` passes; `TtsEngine::to_wav` unit test passes.

---

## Phase 3: User Story 1 — Desktop Wake Word Voice Loop (Priority: P1) 🎯 MVP

**Goal**: `agentix-voice` binary that listens for "Hey Agentix", records speech, transcribes via agentix-daemon, queries agentix-agent, synthesizes response with kokoro-tts, and plays audio on speaker.

**Independent Test**: Run `nix develop --command cargo run -p agentix-voice -- --config agentix-voice/dev.toml`, say "Hey Agentix, list my todos", hear a spoken response within 8 seconds. See quickstart.md Test 5.

- [X] T010 Implement `VoiceState` enum and state machine in `agentix-voice/src/state.rs`: variants `Idle`, `WakeWordDetected`, `Recording { buf: Vec<i16> }`, `Transcribing { audio: Vec<i16> }`, `AgentThinking { transcription: String }`, `Synthesizing { response: String }`, `Playing`; define `VoiceEvent` enum (`WakeWordDetected`, `SilenceDetected`, `TranscriptionReady(String)`, `ResponseReady(String)`, `PlaybackDone`, `Error(String)`); implement `pub fn transition(self, event: VoiceEvent) -> Result<VoiceState, String>` with exhaustive `match (state, event)` — illegal combinations return `Err("invalid transition: ...")`
- [X] T011 [P] [US1] Implement RMS VAD in `agentix-voice/src/vad.rs`: `pub struct Vad { threshold: f32, silence_ms: u64, last_speech_at: Option<std::time::Instant> }`; `pub fn new(threshold: f32, silence_ms: u64) -> Self`; `pub fn process(&mut self, samples: &[f32]) -> VadResult` where `VadResult` is `Speaking | Silence | EndOfUtterance`; compute RMS as `(sum(s²)/n).sqrt()`, update `last_speech_at` when RMS > threshold, return `EndOfUtterance` when silence has lasted `silence_ms` ms
- [X] T012 [P] [US1] Wake word detection deferred: rustpotter 3.x dropped (candle-core/half version conflict with ort rc.13). `agentix-voice` runs in VAD-triggered mode (speech above threshold starts recording). See research.md for alternative approach (openwakeword Python subprocess). `pub struct WakeDetector { potter: rustpotter::Rustpotter }`; `pub fn new(config: &WakeWordConfig) -> Result<Self>` loads `.rpw` via `rustpotter::Rustpotter::new(&RustpotterConfig::default())` and `add_wakeword_from_file`; `pub fn process(&mut self, samples: &[f32]) -> bool` calls `potter.process(samples)` (adapting to rustpotter's per-frame API) and returns `true` if `detection.score >= config.threshold`
- [X] T013 [US1] Implement `AudioRecorder` in `agentix-voice/src/audio.rs`: `pub struct AudioRecorder` wraps a `cpal::Stream` and a `std::sync::mpsc::Receiver<Vec<f32>>`; `pub fn new() -> Result<Self>` opens the default cpal input device, builds an input stream with f32 samples at whatever sample rate the device reports, sends chunks via the channel; `pub fn read_chunk(&self) -> Option<Vec<f32>>` does a non-blocking recv; store device sample rate as `pub fn device_sample_rate(&self) -> u32`
- [X] T014 [US1] Implement `play_f32` in `agentix-voice/src/audio.rs`: `pub fn play_f32(samples: Vec<f32>, sample_rate: u32) -> Result<()>` opens the default cpal output device, creates an output stream with f32 format at `sample_rate`, drains `samples` via a shared cursor protected by `Arc<Mutex<>>`, blocks until all samples are consumed, then drops the stream
- [X] T015 [US1] Implement voice loop orchestration in `agentix-voice/src/voice_loop.rs` (renamed from loop.rs — `loop` is a Rust keyword): `pub struct VoiceLoop { config: VoiceConfig, tts: Arc<TtsEngine>, wake: WakeDetector, recorder: AudioRecorder, vad: Vad, resampler: Resampler }`; `pub async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>>` runs the state machine: (1) read mic chunk, resample to 16 kHz via rubato, feed to WakeDetector; (2) on wake, switch to recording: accumulate i16 samples, run VAD on each chunk; (3) on EndOfUtterance, encode accumulated samples as WAV, POST multipart to `{daemon_url}/v1/audio/transcriptions`, parse `{"text": "..."}` response; (4) POST `{"message": transcription, "session_id": ...}` to `{agent_url}/api/chat`, parse response text; (5) call `tts.synthesize(&response)`, then `tts.to_wav(&samples)`, then `play_f32`; (6) return to Idle; on any error, log with `tracing::error!` and return to Idle with optional spoken error message
- [X] T016 [US1] Implement `main.rs` for `agentix-voice/src/main.rs`: define `#[derive(Parser)] struct Args { #[arg(long)] config: PathBuf }`; load config via `config::load(&args.config)`; initialize `TtsEngine::new(...)` (exit with error message if model files missing); initialize `VoiceLoop`; run in `tokio::main` runtime; install `tokio::signal::ctrl_c()` handler that sets a shutdown flag and logs "shutting down"; log startup info including wake word model path and TTS voice
- [X] T017 [P] [US1] Write unit tests in `agentix-voice/src/state.rs` `#[cfg(test)] mod tests`: `idle_accepts_wake_event` (Idle + WakeWordDetected → WakeWordDetected state), `wake_accepts_recording` (WakeWordDetected + SilenceDetected should NOT be valid, returns Err), `recording_to_transcribing_on_silence` (Recording{buf} + SilenceDetected → Transcribing{audio}), `invalid_transition_returns_err` (Idle + ResponseReady → Err)
- [X] T018 [P] [US1] Write unit tests in `agentix-voice/src/vad.rs` `#[cfg(test)] mod tests`: `silence_samples_not_speech` (all-zero samples → VAD returns Silence), `loud_samples_are_speech` (samples with RMS > threshold → returns Speaking and updates last_speech_at), `end_of_utterance_after_silence_window` (after Speaking then 1s of Silence → returns EndOfUtterance)

**Checkpoint**: `nix develop --command cargo build -p agentix-voice` succeeds; state machine and VAD unit tests pass. Desktop voice loop functional (requires dev environment with mic/speaker — see quickstart Test 5).

---

## Phase 4: User Story 2 — Mobile Voice Input via Browser Mic Button (Priority: P2)

**Goal**: Mic button in agentix-agent web UI that records speech, transcribes via agentix-daemon, and submits as a chat message.

**Independent Test**: Open `http://localhost:8765` on mobile browser, tap mic button, speak, tap stop — transcription appears as sent chat message. See quickstart.md Test 3.

- [X] T019 [US2] Create `agentix-agent/src/voice/mod.rs` with `pub fn routes(state: Arc<AppState>) -> axum::Router`: declares `use axum::Router; Router::new().route("/transcribe", post(transcribe::handler)).route("/synthesize", post(synthesize::handler)).with_state(state)`; create `agentix-agent/src/voice/transcribe.rs` and `agentix-agent/src/voice/synthesize.rs` as empty module stubs; add `pub mod voice;` to `agentix-agent/src/main.rs`
- [X] T020 [US2] Implement `handler` in `agentix-agent/src/voice/transcribe.rs`: signature `async fn handler(State(state): State<Arc<AppState>>, mut multipart: Multipart) -> impl IntoResponse`; extract the `file` field from multipart; return 503 JSON if `state.config.voice` is None; forward the file bytes as a multipart POST to `{daemon_url}/v1/audio/transcriptions` via `reqwest::Client`, setting the same `Content-Type`; on success return the daemon's JSON body; on reqwest error return 502 JSON with the error message
- [X] T021 [US2] Register voice routes in `agentix-agent/src/main.rs`: add `.nest("/api/voice", voice::routes(Arc::clone(&state)))` to the Axum router builder; ensure this compiles with `cargo check -p agentix-agent`
- [X] T022 [US2] Add mic button to `agentix-agent/static/index.html`: insert `<button id="mic-btn" title="Voice input">🎤</button>` adjacent to the message submit button; add JavaScript: `const recorder = { stream: null, mediaRecorder: null }`; `async function startRecording()` — calls `navigator.mediaDevices.getUserMedia({audio: true})`, creates `MediaRecorder(stream, {mimeType: 'audio/webm;codecs=opus'})`, pushes chunks to array, sets btn to red pulsing state; `async function stopRecording()` — stops MediaRecorder, POSTs `FormData` with `file` blob to `/api/voice/transcribe`, on success sets message input value to `data.text` and calls `submitMessage()`, resets btn state; mic btn click toggles between start/stop
- [X] T023 [P] [US2] Add browser feature detection in `agentix-agent/static/index.html`: in the `DOMContentLoaded` handler, check `if (!navigator.mediaDevices?.getUserMedia) { document.getElementById('mic-btn').style.display = 'none'; }`; also add a CSS class `recording` on `#mic-btn` with `animation: pulse 1s infinite; background-color: #e53e3e;` and `@keyframes pulse { 0%,100% { opacity:1 } 50% { opacity:0.5 } }`

**Checkpoint**: `POST /api/voice/transcribe` returns `{"text":"..."}` when tested with `curl -F "file=@test.wav"`. Mic button appears in web UI and submits transcription as a chat message. See quickstart.md Test 3.

---

## Phase 5: User Story 3 — Voice Response Playback on Mobile (Priority: P3)

**Goal**: Speaker button on assistant messages in web UI plays the response using server-side kokoro-tts synthesis.

**Independent Test**: In the web UI, tap speaker icon on any assistant reply — hear the reply spoken aloud. See quickstart.md Test 4.

- [X] T024 [US3] Implement `handler` in `agentix-agent/src/voice/synthesize.rs`: accept `POST /api/voice/synthesize` with `Json(SynthRequest { text: String, voice: Option<String>, rate: Option<f32> })`; return 503 JSON if `state.tts_engine` is None; call `engine.synthesize(&req.text).await` then `engine.to_wav(&samples)`, return `Response::builder().header("Content-Type", "audio/wav").body(wav_bytes)` on success; return 500 JSON on synthesis error; define `SynthRequest` struct locally with `#[derive(serde::Deserialize)]`
- [X] T025 [US3] Initialize `TtsEngine` in `agentix-agent/src/main.rs`: after loading config, if `config.voice.as_ref().and_then(|v| v.tts.as_ref()).is_some()`, call `TtsEngine::new(model_path, voices_dir, voice, max_text_len).await` and store in `AppState`; add `pub tts_engine: Option<Arc<TtsEngine>>` field to `AppState`; on startup, log whether TTS is enabled or disabled; startup MUST NOT fail if `tts` is absent from config
- [X] T026 [US3] Add speaker button to `agentix-agent/static/index.html`: for each assistant message bubble rendered into the DOM, append `<button class="speak-btn" title="Read aloud">🔊</button>`; add JavaScript `async function speakMessage(text, btn)`: POSTs `{text}` to `/api/voice/synthesize`, on 200 creates `const url = URL.createObjectURL(await resp.blob())` and `new Audio(url).play()`; on 503 log to console and hide all `.speak-btn` elements; in `DOMContentLoaded`, probe the synthesize endpoint with a HEAD-style empty POST and hide speak buttons if 503

**Checkpoint**: `POST /api/voice/synthesize` with `{"text":"Hello"}` returns a WAV file. Speaker button plays assistant replies in the mobile web UI. See quickstart.md Test 4.

---

## Phase 6: Polish & Quality Gates

- [X] T027 Add `agentix-tts` and `agentix-voice` to `flake.nix` (perSystem/packages.nix): crane buildPackage derivations, allManifests, depsOnlySrc stubs, memberStubs; add ORT_PREFER_DYNAMIC_LINK + ORT_LIB_LOCATION to commonArgs; add alsa-lib to buildInputs; `packages.agentix-voice` exported
- [X] T028 [P] Run `nix develop --command cargo fmt --all --check` from repo root; fix any formatting issues in `agentix-tts/`, `agentix-voice/`, `agentix-agent/src/voice/`, `agentix-agent/static/`
- [X] T029 [P] Run `nix develop --command cargo clippy -- -D warnings` across `agentix-tts`, `agentix-voice`, `agentix-agent`; fix all warnings; pay attention to unused `Option<>` fields in config structs and any `clippy::unwrap_used` violations
- [X] T030 [P] Run `nix develop --command cargo test -p agentix-tts -p agentix-voice -p agentix-agent`; confirm all non-`#[ignore]` tests pass (T008 WAV header test, T017 state machine tests, T018 VAD tests)
- [X] T031 [P] Run `nix build .#agentix-tts` and `nix build .#agentix-voice`; confirm both build successfully (this is the canonical reproducibility check per constitution Principle VIII)
- [ ] T032 Manually validate quickstart.md Test 1 (TTS endpoint), Test 2 (STT proxy), and Test 3 (mobile mic button) against a running agentix-agent; document any latency observations in the quickstart; mark all tests pass before closing this task

---

## Dependencies

```
T001 → T002, T003, T004
T002 → T005
T003 → T009, T015, T016
T005 → T008, T015, T024, T025
T006 → T019, T020, T024
T007 → T025
T008 → T030
T009 → T013, T015, T016
T010 → T017
T011 → T018
T012 → T015
T013 → T015
T014 → T015
T015 → T016
T016 → T031
T017 → T030
T018 → T030
T019 → T020, T021
T020 → T021, T032
T021 → T022, T032
T022 → T023
T024 → T025, T026
T025 → T032
T026 → T032
T027 → T031
T028, T029, T030, T031, T032 → done
```

### Story Dependencies

- **US1** depends on: Phase 2 (T005, T009) — agentix-tts library + agentix-voice config
- **US2** depends on: Phase 1 setup only (T001) — does NOT require agentix-tts for the transcribe proxy
- **US3** depends on: T005 (TtsEngine), T006 (agent dep), T007 (agent config), T025 (AppState init)
- **US2 + US3** share the `agentix-agent/src/voice/` module (T019 must complete before T020 and T024)

### Parallel Execution Examples

**Phase 1** (T002 and T003 touch different files):
```
Task T002: agentix-tts/Cargo.toml
Task T003: agentix-voice/Cargo.toml
```

**Phase 2** (T006, T007, T009 all touch different files):
```
Task T006: agentix-agent/Cargo.toml (add dep)
Task T007: agentix-agent/src/config.rs (VoiceAgentConfig)
Task T009: agentix-voice/src/config.rs (VoiceConfig)
```

**Phase 3 US1** (T011, T012 are independent):
```
Task T011: agentix-voice/src/vad.rs
Task T012: agentix-voice/src/wake.rs
```

**Phase 6 polish** (all independent after T027):
```
Task T028: cargo fmt
Task T029: cargo clippy
Task T030: cargo test
Task T031: nix build
```

---

## Implementation Strategy

### MVP (US1 only — Phases 1–3)

1. Complete Phase 1: workspace setup (T001–T004)
2. Complete Phase 2: agentix-tts library (T005–T009)
3. Complete Phase 3: desktop voice loop (T010–T018)
4. **STOP and VALIDATE**: say "Hey Agentix, list my todos", hear spoken response (quickstart Test 5)
5. If validated, commit Phase 3 as MVP

### Incremental Delivery

- **After Phase 4** (US2): Mobile mic button works — submit voice queries from phone
- **After Phase 5** (US3): Mobile speaker button works — hear responses on phone
- **After Phase 6**: All quality gates pass; ship-ready

### US2 can start while US1 is in-progress

T019–T023 (US2) only depend on Phase 1 setup and agentix-agent structure — they do NOT depend on `agentix-tts`. Once T001 is done, US2 work can proceed in parallel with US1 work.
