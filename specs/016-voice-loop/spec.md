# Feature Specification: Voice-First Home Assistant Loop

**Feature Branch**: `016-voice-loop`
**Created**: 2026-08-22
**Status**: Draft
**Input**: User description: "Voice-first home assistant loop — wake word, STT, agent, TTS on desktop and mobile"

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Desktop Wake Word Voice Loop (Priority: P1)

Samuel is in his kitchen getting ready for the day. Without touching any device, he says "Hey Agentix, what do I have on my calendar today?" The system detects the wake word, records his speech, transcribes it, queries the assistant, and speaks the response aloud through the desktop speakers.

**Why this priority**: Core value proposition of the feature — hands-free, voice-first interaction on the desktop. All other stories depend on pieces built here (STT proxy, TTS synthesis).

**Independent Test**: Run `agentix-voice` on the desktop, say "Hey Agentix, list my todos", hear a spoken response. Full voice loop without touching a browser or keyboard.

**Acceptance Scenarios**:

1. **Given** `agentix-voice` is running and the microphone is active, **When** Samuel says "Hey Agentix" followed by a question, **Then** the system transcribes his speech and speaks the assistant's answer within 8 seconds of the wake word.
2. **Given** background noise is present, **When** Samuel speaks the wake phrase clearly at normal volume, **Then** the wake word is detected reliably and spurious activations are infrequent (fewer than 1 per hour in a quiet home environment).
3. **Given** the assistant finishes speaking, **When** Samuel speaks again without the wake phrase, **Then** the system does NOT activate (wake phrase is always required to start a new turn).
4. **Given** agentix-daemon or agentix-agent is unreachable, **When** a voice query is attempted, **Then** the system speaks a brief error message instead of silently failing.

---

### User Story 2 — Mobile Voice Input via Browser Mic Button (Priority: P2)

Samuel is on his phone at work. He opens the agentix-agent web UI and taps a mic button. He speaks his question, taps stop, and sees the transcription automatically submitted as a chat message with the assistant's text response shown.

**Why this priority**: Extends voice input to mobile without any native app. Leverages the existing browser-based chat UI and the STT service already present.

**Independent Test**: Open the web UI on a mobile browser, tap the mic button, speak a question, tap stop — transcription appears as a sent message and the assistant replies. No desktop daemon needed.

**Acceptance Scenarios**:

1. **Given** the user is on the chat page, **When** they tap the mic button and speak, **Then** a recording indicator is visible and audio is captured.
2. **Given** recording is active, **When** the user taps stop, **Then** the speech is transcribed and the transcription text is submitted as a chat message automatically.
3. **Given** a chat message is submitted via voice, **When** the assistant replies, **Then** the response text is displayed in the chat UI normally.
4. **Given** the user's browser does not support audio capture, **When** they visit the chat page, **Then** the mic button is hidden or disabled with explanatory text.

---

### User Story 3 — Voice Response Playback on Mobile (Priority: P3)

After receiving a text reply in the mobile web UI, Samuel taps a speaker icon to hear the assistant's response read aloud through his phone speaker.

**Why this priority**: Completes the mobile voice experience. Useful but not critical — users can already read responses; P2 delivers functional voice input without this.

**Independent Test**: Submit a chat message via the web UI, then tap the speaker icon on the assistant's reply — hear the response spoken aloud.

**Acceptance Scenarios**:

1. **Given** the assistant has replied to a message, **When** the user taps the speaker icon, **Then** the reply text is synthesized and played through the device's audio output.
2. **Given** synthesis is in progress, **When** the user taps stop or navigates away, **Then** playback stops immediately.
3. **Given** the TTS model is not configured or unavailable, **When** the speaker icon is tapped, **Then** an error indicator is shown and no audio plays.

---

### Edge Cases

- What happens if microphone permission is denied on mobile?
- How does the system handle a voice query that produces an empty transcription?
- What if the assistant response is very long (>500 words) — is it truncated before TTS synthesis?
- What if the user says the wake word while the system is already speaking a TTS response?
- What happens when the TTS model files are missing or corrupt at startup?
- How does VAD tuning avoid cutting off slow speakers or capturing only silence?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The desktop daemon MUST continuously monitor microphone input and detect the wake phrase "Hey Agentix" before activating a recording session.
- **FR-002**: Upon wake word detection, the daemon MUST record the user's subsequent speech until silence is detected (voice activity detection) and stop automatically.
- **FR-003**: Recorded audio MUST be sent to the transcription service and the resulting text retrieved.
- **FR-004**: The transcribed text MUST be sent to the assistant and the assistant's text response retrieved.
- **FR-005**: The assistant's text response MUST be synthesized to audio and played through the system's default audio output.
- **FR-006**: The complete desktop voice turn (wake word detected → spoken response begins) MUST complete within 8 seconds for queries under 20 words on a local LAN.
- **FR-007**: The mobile web UI MUST provide a mic button that activates audio recording via the browser's media capture API.
- **FR-008**: When recording stops on mobile, the captured audio MUST be sent to the server for transcription and the transcribed text submitted as a chat message.
- **FR-009**: The server MUST expose a voice synthesis endpoint that accepts text and returns audio data playable by a browser.
- **FR-010**: The desktop daemon's configuration (STT endpoint URL, agent endpoint URL, TTS model paths, wake word sensitivity) MUST be loadable from the shared config file.
- **FR-011**: The TTS voice and speaking rate MUST be configurable; the default MUST produce natural-sounding English speech.
- **FR-012**: The system MUST log each voice turn (timestamp, transcription, response snippet) at debug level for troubleshooting.

### Key Entities

- **Voice Turn**: A complete interaction cycle — wake word → user speech → transcription → assistant response → TTS audio. Has a start timestamp, transcribed input, and text response.
- **TTS Config**: Voice name, speaking rate, model file paths. Part of the application config file.
- **Wake Word Config**: Sensitivity threshold, keyword name, subprocess path. Part of the application config file.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The complete desktop voice loop (wake word to first audio output) completes in under 8 seconds for questions under 20 words, measured on a local LAN with no internet dependency.
- **SC-002**: Wake word false-positive rate is fewer than 1 spurious activation per hour in a typical home environment (TV or radio playing at moderate volume).
- **SC-003**: Wake word true-positive rate is at least 90% — the phrase "Hey Agentix" spoken at normal conversational volume triggers recording 9 out of 10 attempts.
- **SC-004**: Mobile voice input to chat message submission (tap stop → message appears) takes under 5 seconds.
- **SC-005**: TTS synthesis for a 50-word response completes in under 2 seconds on the host machine.
- **SC-006**: The entire voice pipeline operates without an internet connection, using only local models and services.

## Assumptions

- agentix-daemon (STT endpoint at `POST /v1/audio/transcriptions`) and agentix-agent (chat endpoint at `POST /api/chat`) are already running on the local network; `agentix-voice` is a client that calls them.
- The Kokoro TTS model files must be downloaded separately by the user and their paths provided in config; they are not bundled in the repository.
- The wake word detector runs as a local Python subprocess (Python 3.x with openwakeword installed); it is not a compiled Rust dependency.
- A pre-trained "Hey Agentix" wake word model does not exist; initial implementation uses the closest available pre-built model (e.g., "hey_jarvis") as a placeholder. Training a custom model is out of scope.
- Desktop audio I/O uses the system's default microphone and speaker; per-device selection is out of scope.
- Mobile TTS (P3) uses the server-side synthesis endpoint to ensure consistent voice quality; it does not rely on the browser's built-in speech synthesis.
- The `agentix-voice` binary runs on the same host as `agentix-daemon` and communicates over localhost.
- No authentication is required between `agentix-voice` and `agentix-agent`/`agentix-daemon` (both on the same trusted LAN).
- Responses longer than 500 words are truncated before TTS synthesis to keep response latency reasonable.
