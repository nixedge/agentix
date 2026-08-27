# HTTP API Contracts: Voice Endpoints (agentix-agent)

**Feature**: 016-voice-loop
**Service**: agentix-agent (port 8765)
**Date**: 2026-08-22

---

## POST /api/voice/transcribe

Proxy: forwards audio to agentix-daemon's whisper endpoint and returns the transcription.

### Request

```
POST /api/voice/transcribe
Content-Type: multipart/form-data
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file` | binary | yes | Audio file (webm/opus, mp4/aac, wav, etc.) |
| `model` | string | no | Whisper model name; forwarded to daemon (default: omit) |

### Response (200 OK)

```json
{
  "text": "What's on my calendar today?"
}
```

### Error Responses

| Status | Body | Condition |
|--------|------|-----------|
| 400 | `{"error": "missing file field"}` | No `file` field in form |
| 502 | `{"error": "transcription service unavailable: <detail>"}` | agentix-daemon unreachable or returned error |
| 503 | `{"error": "daemon_url not configured"}` | `voice.daemon_url` absent from config |

### Notes
- This is a transparent proxy; the audio is forwarded as-is to `POST <daemon_url>/v1/audio/transcriptions`
- No audio is stored on disk; the forwarded stream is ephemeral
- Used by the mobile web UI (MediaRecorder → browser POST) and potentially agentix-voice (if it routes through the agent instead of the daemon directly)

---

## POST /api/voice/synthesize

Synthesize text to speech using kokoro-tts. Returns a WAV audio file.

### Request

```
POST /api/voice/synthesize
Content-Type: application/json
```

```json
{
  "text": "You have three todos: buy milk, call the dentist, and check the mail.",
  "voice": "af_heart",
  "rate": 1.0
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `text` | string | yes | Text to synthesize (max 500 chars; truncated if longer) |
| `voice` | string | no | Kokoro voice name (default: config value) |
| `rate` | number | no | Speaking rate multiplier (default: 1.0) |

### Response (200 OK)

```
Content-Type: audio/wav
Content-Length: <bytes>

<WAV binary data — 24kHz mono f32 PCM>
```

The response body is a valid WAV file playable by any standard audio player or browser `<audio>` element.

### Error Responses

| Status | Body | Condition |
|--------|------|-----------|
| 400 | `{"error": "text is required"}` | Empty or missing `text` field |
| 503 | `{"error": "TTS not configured"}` | `voice.tts` block absent from config |
| 500 | `{"error": "synthesis failed: <detail>"}` | kokoro-tts returned an error |

### Notes
- Synthesis is blocking (CPU-bound); the endpoint may take 200–2000ms depending on text length
- Responses are not cached; each request triggers a fresh synthesis
- Used by the mobile web UI "speaker" button (P3); also available to any HTTP client on the LAN

---

## Mobile Web UI Changes

The existing `agentix-agent/static/index.html` gains:

### Mic Button (P2)

A `<button id="mic-btn">` in the chat input area:
- Click to start recording (MediaRecorder, `audio/webm;codecs=opus`)
- Visual state: idle → recording (red pulsing dot) → processing (spinner)
- On stop: POST to `/api/voice/transcribe`, then submit transcription as chat message
- On error: display inline error message

### Speaker Button (P3)

A `<button class="speak-btn">` on each assistant message bubble:
- Click to fetch `/api/voice/synthesize` with the message text
- Play returned WAV via `new Audio(url).play()`
- Visual state: idle → loading → playing; click again to stop

### Feature Detection

If `navigator.mediaDevices.getUserMedia` is unavailable, the mic button is hidden.
If `/api/voice/synthesize` returns 503, the speak button is hidden on page load (probe on init).
