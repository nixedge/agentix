# Research: agentix-agent Home Assistant

**Phase**: 0 | **Branch**: `013-home-agent` | **Date**: 2026-08-19

## Decision 1: Google API Integration

**Decision**: `yup-oauth2` v12 + direct `reqwest` REST calls to Google Calendar v3 and Gmail v1 APIs.

**Rationale**: The `google-calendar3` / `google-gmail1` machine-generated crates (from `google-apis-rs`) are in explicit maintenance mode (no new maintainer, ~35 open issues, pinned to hyper 0.14) and are incompatible with the workspace's reqwest 0.12 / hyper 1.x stack. Direct REST calls to Google's APIs are trivial — each needs at most 2 endpoints. `yup-oauth2` v12.1.x provides device-flow OAuth2 with automatic token refresh and disk persistence, which is exactly what a headless home server needs.

**OAuth2 flow**: Device flow — on first start, agentix-agent prints a URL and code; Samuel authorises once in a browser; subsequent runs load and auto-refresh the cached token. A service account is an alternative but cannot access a personal Google Calendar unless explicitly shared.

**Alternatives considered**: `gcal` crate (unmaintained), `google-calendar3` (dependency conflict), service account (requires calendar sharing).

**Minimal API surface**:
- Calendar list events: `GET /calendar/v3/calendars/{id}/events?timeMin=&timeMax=&singleEvents=true`
- Calendar create event: `POST /calendar/v3/calendars/{id}/events`
- Gmail list unread: `GET /gmail/v1/users/me/messages?q=is:unread&maxResults=20`
- Gmail get message: `GET /gmail/v1/users/me/messages/{id}?format=metadata&metadataHeaders=Subject,From,Date`

**Scopes required**: `calendar` (read/write), `gmail.readonly`.

**Quotas**: Personal use is far below free tiers (10k Calendar req/min, 1.2M Gmail quota-units/min).

---

## Decision 2: Push Notifications

**Decision**: Plain `reqwest` POST to `https://ntfy.sh/{topic}`.

**Rationale**: ntfy.sh's publish API is a single HTTP POST with optional headers for title, priority, and tags. No client library is needed. The `ntfy` crate (v0.9.1) exists but adds a dependency that delivers negligible value over 5 lines of reqwest code.

**API**: `POST https://ntfy.sh/{topic}` with headers `Title:`, `Priority:` (1-5), `Tags:` and body as plain text. No auth for public topics; Bearer token for access-controlled topics.

**Alternatives considered**: Pushover (paid), Home Assistant companion (requires HA installation), Firebase (requires native app).

---

## Decision 3: Storage for Todos and Notifications

**Decision**: `sled` v0.34 — pure-Rust embedded key-value store.

**Rationale**: Todos, notifications, Gmail cursor, and conversation sessions are not relational. There are no joins, no foreign-key constraints needed at the DB layer, and the dataset is tiny (tens to low hundreds of todos for a single-user home assistant). Sled is zero-configuration, pure Rust (no C++ FFI, no system library deps), runs embedded in-process, and survives crashes safely via its lock-free log-structured design. All data is modelled as typed sled trees with `serde_json`-serialised values. Secondary lookup (e.g., dedup by Gmail message ID) uses a second tree as an explicit index. Since sled's API is synchronous, all store calls go through `tokio::task::spawn_blocking`.

PostgreSQL is already in the workspace for `agentix-search` (BM25/pgvector) and remains available if any future data need becomes relational — but that is not the case here.

**Alternatives considered**: SQLite via sqlx (relational overhead for non-relational data, C-library dep, requires migration files); PostgreSQL (operational coupling — agentix-agent should start standalone without a running PG instance); JSON file (no concurrent-write safety, no indexing).

---

## Decision 4: Transcription for Phone Voice Input

**Decision**: Proxy audio uploads to agentix-daemon's existing `/v1/audio/transcriptions` endpoint via HTTP.

**Rationale**: agentix-whisper links whisper.cpp (C++) and symphonia (audio decoding). Linking that into agentix-agent would add heavy C++ compile-time deps and violate the isolation principle that keeps C++ contained to `agentix-whisper` and `agentix-llama`. The daemon already exposes the transcription endpoint. agentix-agent just streams the upload bytes to the daemon and gets back text — clean separation.

**Audio format**: Mobile browsers send WebM/Opus (Android/modern iOS) or MP4/AAC (older iOS). The daemon's symphonia-based pipeline handles both.

**Alternatives considered**: Linking agentix-whisper directly (violates C++ isolation), cloud Whisper API (violates local-first principle), wav-only upload requirement (breaks iOS Safari < 18.4).

---

## Decision 5: Binary Structure — Separate Binary vs. Extending Daemon

**Decision**: New binary crate `agentix-agent` binding its own HTTP port (default 8765).

**Rationale**: The home agent is a distinct user-facing service with separate lifecycle concerns (persistent background schedulers for Gmail polling and notification delivery, mobile web UI, different config). Folding it into `agentix-daemon` would couple two unrelated services and make the daemon harder to reason about. The constitution's "daemon is sole crate that binds a port" refers to the inference gateway; `agentix-whisper` already has a binary with Axum, and `agentix-ax` is a standalone binary — a second user-facing daemon is consistent with the workspace pattern.

**Complexity justification**: Documented in `plan.md` Complexity Tracking table.

**Alternatives considered**: Routes added to agentix-daemon (lifecycle coupling, bloats gateway), MCP server extension (wrong protocol for phone chat UI).

---

## Decision 6: Web UI for Phone Chat

**Decision**: Minimal single-page HTML served from an embedded string literal in agentix-agent, using the Fetch API and MediaRecorder for audio.

**Rationale**: No build toolchain needed; no separate frontend crate; easily iterable. The UI surface is deliberately minimal (text input + voice record button + chat history). The embedded HTML is replaced by a richer frontend later without protocol changes.

**Audio recording**: Use `MediaRecorder.isTypeSupported('audio/webm;codecs=opus')` to pick WebM/Opus (Android, modern iOS) or fall back to `audio/mp4` (older iOS). Upload as multipart form data.

**Alternatives considered**: React SPA (overkill, requires build step), Telegram bot (requires third-party account), native iOS/Android app (too heavy for v1).

---

## Key Crates (New)

| Crate | Version | Role |
|---|---|---|
| `yup-oauth2` | 12.x | Google OAuth2 device flow + token caching |
| `sled` | 0.34 | Embedded key-value store (trees per entity type) |
| `chrono` | 0.4 | Datetime parsing and formatting |
| `uuid` | 1.x | UUID v4 for entity IDs |
| `toml` | 0.8 | Config file deserialization |

All other dependencies (`reqwest`, `axum`, `tokio`, `serde`, `serde_json`, `anyhow`, `tracing`) are already in the workspace. `sqlx` is **not** used.
