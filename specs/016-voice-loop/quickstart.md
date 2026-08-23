# Quickstart: Voice-First Home Assistant Loop

**Feature**: 016-voice-loop
**Date**: 2026-08-22

---

## Prerequisites

1. agentix-daemon running on `http://localhost:8080` (whisper model loaded)
2. agentix-agent running on `http://localhost:8765`
3. Kokoro TTS model files downloaded:
   ```
   ~/.local/share/agentix/kokoro-v0_19.onnx
   ~/.local/share/agentix/voices/
   ```
4. `agentix-voice` built and a `.rpw` wake word file created (see Setup)

---

## One-time Setup: Wake Word File

Record 3–8 short WAV clips of yourself saying "Hey Agentix" and create the `.rpw` file:

```bash
# Install rustpotter-cli
nix develop --command cargo install rustpotter-cli

# Create wake word from recordings
rustpotter-cli create-wakeword \
  --name hey_agentix \
  hey_agentix_1.wav hey_agentix_2.wav hey_agentix_3.wav \
  -o agentix-voice/wakewords/hey_agentix.rpw
```

For initial development/testing, a placeholder model (e.g., "alexa") is included at
`agentix-voice/wakewords/placeholder.rpw`.

---

## Config

Add to `agentix-agent/dev.toml` (agentix-agent voice endpoints):

```toml
[voice]
daemon_url = "http://localhost:8080"

[voice.tts]
model_path = "/home/sam/.local/share/agentix/kokoro-v0_19.onnx"
voices_dir = "/home/sam/.local/share/agentix/voices"
voice = "af_heart"
```

Create `agentix-voice/dev.toml` (desktop voice daemon):

```toml
agent_url = "http://localhost:8765"
daemon_url = "http://localhost:8080"
vad_threshold = 0.01
vad_silence_ms = 800

[wake_word]
model_path = "wakewords/hey_agentix.rpw"
threshold = 0.5

[tts]
model_path = "/home/sam/.local/share/agentix/kokoro-v0_19.onnx"
voices_dir = "/home/sam/.local/share/agentix/voices"
voice = "af_heart"
```

---

## Test 1 — TTS Synthesis Endpoint (agentix-agent)

Verify kokoro-tts works through the HTTP endpoint:

```bash
curl -s -X POST http://localhost:8765/api/voice/synthesize \
  -H "Content-Type: application/json" \
  -d '{"text": "Hello, your assistant is ready."}' \
  -o /tmp/test.wav

# Play it
aplay /tmp/test.wav
```

**Expected**: A WAV file is returned; `aplay` plays a clear voice saying "Hello, your assistant is ready."

---

## Test 2 — STT Proxy (agentix-agent)

Verify the transcribe proxy works:

```bash
# Record a 3-second clip of yourself speaking
arecord -d 3 -f S16_LE -r 16000 -c 1 /tmp/test.wav

curl -s -X POST http://localhost:8765/api/voice/transcribe \
  -F "file=@/tmp/test.wav"
```

**Expected**: `{"text": "...your spoken words..."}` returned within 5 seconds.

---

## Test 3 — Mobile Mic Button (browser)

1. Open `http://localhost:8765` on a mobile browser
2. Tap the mic button (bottom of the chat input area)
3. Allow microphone access when prompted
4. Speak: "What's on my calendar today?"
5. Tap the mic button again to stop

**Expected**: The transcription appears as a sent message, and the assistant replies with today's calendar events.

---

## Test 4 — Mobile Speaker Button (browser, P3)

1. After receiving any assistant reply in the web UI
2. Tap the speaker icon on the assistant's message bubble

**Expected**: The assistant's reply is spoken aloud through the device's speaker.

---

## Test 5 — Desktop Voice Loop (agentix-voice, P1)

Start the voice daemon:

```bash
nix develop --command cargo run -p agentix-voice -- --config agentix-voice/dev.toml
```

**Expected startup log**:
```
INFO agentix_voice: Wake word model loaded: hey_agentix.rpw
INFO agentix_voice: TTS engine ready (af_heart, 24kHz)
INFO agentix_voice: Microphone open, listening for wake word...
```

Then say aloud: **"Hey Agentix, what do I have to do today?"**

**Expected**:
1. Log: `INFO agentix_voice: Wake word detected (score=0.72), recording...`
2. Speak your question (recording stops after ~800ms of silence)
3. Log: `INFO agentix_voice: Transcription: "What do I have to do today?"`
4. Log: `INFO agentix_voice: Agent response received, synthesizing...`
5. Assistant's response is spoken aloud within 8 seconds of wake word

---

## Test 6 — Error Recovery

Kill agentix-agent while agentix-voice is running. Say the wake word.

**Expected**: agentix-voice speaks "Sorry, the assistant is unavailable right now" and returns to idle listening state. It does not crash.

---

## Latency Targets

| Step | Budget |
|------|--------|
| Wake word → recording starts | < 200ms |
| Recording → transcription returned | < 3s |
| Transcription → agent response | < 2s |
| Agent response → TTS synthesis starts | < 100ms |
| TTS synthesis (50-word response) | < 2s |
| **Total (wake word → audio begins)** | **< 8s** |
