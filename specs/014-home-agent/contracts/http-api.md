# HTTP API Contract: agentix-agent

**Base URL**: `http://<host>:8765`
**Content-Type**: `application/json` (unless noted)
**Auth**: None (LAN-only service; protect via network/VPN)

---

## Chat Endpoints

### POST /chat

Send a text message to the home agent and receive a reply.

**Request**
```json
{
  "message": "remind me to call the plumber tomorrow"
}
```
Headers: `Cookie: session=<uuid>` (optional; omit to start a new session)

**Response 200**
```json
{
  "reply": "Got it! I've added 'Call the plumber' due tomorrow (2026-08-20). Anything else?",
  "session": "550e8400-e29b-41d4-a716-446655440000"
}
```
Set-Cookie: `session=<uuid>; Path=/; HttpOnly`

**Response 500**
```json
{ "error": "Agent loop failed: <message>" }
```

---

### POST /chat/voice

Upload an audio file, transcribe it, and process the result as a chat message.

**Request**: multipart/form-data
- `audio`: audio file (WebM/Opus, MP4/AAC, or WAV)
- `session`: (optional) session UUID string

**Response 200**: same schema as POST /chat

**Response 422**
```json
{ "error": "Transcription returned empty text — please speak more clearly or try again." }
```

**Response 502**
```json
{ "error": "Transcription service unavailable: <daemon error>" }
```

---

## Todo Endpoints

### GET /todos

List todos. Defaults to open todos only, sorted by `due_at` ascending (nulls last), then `created_at`.

**Query params**
- `completed=true` — include completed todos
- `limit=N` — max results (default 50)
- `q=<text>` — simple case-insensitive title search

**Response 200**
```json
{
  "todos": [
    {
      "id": "550e8400-...",
      "title": "Call the plumber",
      "due_at": "2026-08-20T09:00:00Z",
      "completed": false,
      "created_at": "2026-08-19T14:00:00Z",
      "completed_at": null,
      "source": "manual",
      "notes": null
    }
  ],
  "total": 1
}
```

---

### POST /todos

Create a todo directly (bypassing the agent loop).

**Request**
```json
{
  "title": "Buy groceries",
  "due_at": "2026-08-21T18:00:00Z",
  "notes": "milk, eggs, bread"
}
```
`due_at` and `notes` are optional.

**Response 201**
```json
{
  "id": "550e8400-...",
  "title": "Buy groceries",
  "due_at": "2026-08-21T18:00:00Z",
  "completed": false,
  "created_at": "2026-08-19T14:05:00Z",
  "completed_at": null,
  "source": "manual",
  "notes": "milk, eggs, bread"
}
```

**Response 422**
```json
{ "error": "title must not be empty" }
```

---

### PATCH /todos/:id

Update a todo (complete it, change due date, edit notes).

**Request** (all fields optional)
```json
{
  "completed": true,
  "due_at": "2026-08-22T09:00:00Z",
  "notes": "called, rescheduled"
}
```

**Response 200**: updated Todo object (same schema as POST /todos response)

**Response 404**
```json
{ "error": "todo not found" }
```

---

## Web UI

### GET /

Serves the embedded single-page HTML chat interface. Designed for mobile browsers.

**Response 200**: `text/html`

---

## Health

### GET /health

**Response 200**
```json
{
  "status": "ok",
  "db": "ok",
  "gateway": "ok | unreachable",
  "google_auth": "ok | expired | unconfigured"
}
```

---

## ESP32 Voice Seam (reserved, not implemented in v1)

The following endpoint is **not implemented** in this phase but is reserved to keep the interface stable for ESP32 integration:

```
POST /voice/stream
Content-Type: audio/pcm;rate=16000;bits=16;channels=1
```

The ESP32 will stream raw 16 kHz mono 16-bit PCM over this endpoint. Server-side streaming transcription + agent response will be added when hardware is ready. The URL path and content-type are locked; do not repurpose them.
