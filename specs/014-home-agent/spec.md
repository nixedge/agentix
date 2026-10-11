# Feature Specification: agentix-agent Home Assistant

**Feature Branch**: `014-home-agent`
**Created**: 2026-08-19
**Status**: Draft
**Input**: User description: "agentix-agent: a home assistant agent crate in the agentix workspace. It tracks household todos, has access to Google Calendar and Gmail (via MCP or OAuth), sends push notifications via ntfy.sh, and will eventually support ESP32 voice I/O (hardware deferred). It runs as a long-lived daemon, uses the existing agentix-harness for the agent loop, and exposes a simple CLI or HTTP interface for interacting with it. The user is Samuel Leathers (samuel.leathers@iohk.io)."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Receive proactive push notification reminders (Priority: P1)

Samuel is away from his desk. The agent notices an upcoming calendar event or an overdue household todo and pushes a reminder to his phone via the ntfy app.

**Why this priority**: Proactive delivery is the core value proposition — the agent surfaces things Samuel might forget without him having to ask. It exercises calendar integration, todo storage, and the push notification channel all at once, and is independently demonstrable.

**Independent Test**: Can be fully tested by creating a todo with a past due date and waiting for the overdue notification to arrive in the ntfy app — no Google credentials required. Calendar event reminders (acceptance scenario 2) require Phase 5 (Google Calendar integration) and are not part of the US1 MVP.

**Acceptance Scenarios**:

1. **Given** a household todo exists with a due date that is now overdue, **When** the agent's background check runs, **Then** Samuel receives a push notification on his phone describing the overdue task within 5 minutes.
2. **Given** a Google Calendar event is starting in 30 minutes, **When** the agent's background check runs, **Then** Samuel receives a push notification summarising the event and any related open todos.
3. **Given** ntfy.sh is unreachable, **When** the agent attempts delivery, **Then** the failure is logged and delivery is retried on the next check interval without crashing the daemon.

---

### User Story 2 - Add and complete household todos via phone chat (Priority: P2)

Samuel is standing in the kitchen and thinks of something he needs to do. He opens a chat interface on his phone, types (or speaks) the task, and the agent adds it to his todo list and confirms. Later he marks it done the same way.

**Why this priority**: Text and voice chat from the phone is the primary interactive channel. It makes the agent useful without being at a computer and covers the full create/complete lifecycle.

**Independent Test**: Can be tested end-to-end by sending a message to the HTTP endpoint from a mobile browser, receiving a confirmation response, and verifying the todo appears in the list.

**Acceptance Scenarios**:

1. **Given** the agent is running and reachable from Samuel's phone, **When** he sends "remind me to call the plumber tomorrow", **Then** the agent creates a todo and replies with confirmation including the due date it inferred.
2. **Given** a todo exists for "call the plumber", **When** he sends "mark the plumber todo as done", **Then** the agent marks it complete and confirms.
3. **Given** Samuel sends a voice memo from his phone via the chat interface, **When** the agent receives the audio, **Then** it transcribes it, processes it as a natural-language command, and replies with the result.
4. **Given** the request is ambiguous (e.g., "call them"), **When** the agent cannot determine the referent, **Then** it asks a single clarifying question rather than silently guessing.

---

### User Story 3 - Query upcoming calendar events and open todos (Priority: P3)

Samuel asks the agent "what's on my plate this week?" and receives a consolidated summary of calendar events and open household todos, ordered by urgency.

**Why this priority**: Read-only calendar queries validate the Google Calendar integration independently of write operations and demonstrate agent reasoning across two data sources.

**Independent Test**: Can be tested by asking "what do I have coming up?" and verifying the response includes real calendar events from the connected Google account.

**Acceptance Scenarios**:

1. **Given** Google Calendar is connected and has events this week, **When** Samuel asks "what do I have coming up?", **Then** the agent replies with a list of events and any open todos due before the end of the week.
2. **Given** no events exist this week, **When** Samuel asks, **Then** the agent responds that the calendar is clear and shows only open todos.
3. **Given** the Google Calendar token has expired, **When** Samuel queries, **Then** the agent notifies him that re-authentication is needed rather than returning an empty or incorrect response.

---

### User Story 4 - Surface actionable emails as todos or notifications (Priority: P4)

The agent monitors Gmail for emails that imply something Samuel needs to do (bills due, package deliveries, appointment confirmations) and either pushes a notification or adds a todo automatically.

**Why this priority**: Gmail integration completes the "ambient awareness" loop but is less urgent than the interactive and notification features. It must not block the other stories.

**Independent Test**: Can be tested by sending a test email with a recognisable action pattern (e.g., "your package will arrive tomorrow") and verifying a todo or notification is generated.

**Acceptance Scenarios**:

1. **Given** an email arrives with a recognisable action pattern (bill, delivery, appointment), **When** the agent's scheduled Gmail scan runs, **Then** a todo or push notification is created with the relevant details extracted from the email.
2. **Given** an email is clearly informational with no action required, **When** the agent scans it, **Then** no todo is created and no notification is sent.
3. **Given** the agent already created a todo from a specific email, **When** the next Gmail scan runs, **Then** no duplicate todo is created for that same email.

---

### Edge Cases

- What happens when the Google OAuth token expires mid-session? Agent must notify Samuel and continue operating with cached data rather than crashing.
- What happens when Samuel sends conflicting commands rapidly? Agent must process them sequentially without corrupting the todo list.
- What happens when the todo list grows very large (hundreds of items)? Agent should surface only the most urgent/relevant items in responses, with a way to list all.
- What if a voice memo is too noisy to transcribe reliably? Agent should ask Samuel to repeat rather than acting on a low-confidence transcription.
- What if Samuel asks about a todo from weeks ago that is no longer visible in the default view? Agent must be able to search the full history on request.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The agent MUST run as a persistent background daemon that survives network interruptions and resumes normal operation without manual intervention.
- **FR-002**: The agent MUST maintain a persistent list of household todos with at minimum: title, optional due date, completed state, creation timestamp, and optional source (manual, gmail).
- **FR-003**: The agent MUST send push notifications to Samuel's phone via ntfy.sh when a todo is overdue or a calendar event is imminent (configurable lead time).
- **FR-004**: The agent MUST expose an HTTP endpoint that accepts natural-language text messages from a mobile browser and returns plain-text or markdown responses.
- **FR-005**: The agent MUST serve a minimal mobile-friendly web UI from the same HTTP server so Samuel can chat without installing a separate app.
- **FR-006**: The agent MUST accept audio file uploads via the web UI, transcribe them, and process the transcription as a natural-language command.
- **FR-007**: The agent MUST read from and optionally create events in Samuel's Google Calendar.
- **FR-008**: The agent MUST scan Samuel's Gmail on a configurable schedule to identify actionable emails and convert them into todos or notifications.
- **FR-009**: The agent MUST deduplicate todos created from Gmail to prevent repeated entries across scans.
- **FR-010**: The agent MUST use the existing agentix-harness agent loop for all LLM-driven reasoning.
- **FR-011**: The agent MUST be configurable via a local config file specifying: ntfy topic, Google credentials path, check intervals, and notification lead times.
- **FR-012**: The agent MUST provide a command-line interface for querying state (list todos, list upcoming events) without requiring the full daemon to be running.
- **FR-013**: The agent MUST bind on all interfaces (IPv4 and IPv6) by default so it is reachable from any device on the network. Firewall and network access control are the user's responsibility; the agent does not enforce them.
- **FR-014**: The architecture MUST accommodate ESP32 voice I/O as a future input channel without requiring structural changes to the agent core; no ESP32 code is written in this phase.

### Key Entities

- **Todo**: A household task with title, optional due date, completed flag, creation timestamp, and optional source (manual, gmail, calendar-derived).
- **Notification**: A push event delivered to the phone with title, body, and delivery status. Tracked to allow retry without duplication.
- **CalendarEvent**: A read/write view of a Google Calendar event: title, start/end time, optional location, and attendees (list of email addresses).
- **ConversationTurn**: A single exchange (user message + agent response) stored for multi-turn context within a session.
- **AgentConfig**: Runtime configuration — ntfy topic/URL, credential file paths, polling intervals, notification lead times, LLM backend selection.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Push notifications for overdue todos arrive on Samuel's phone within 5 minutes of the due time, with no missed notifications over a 7-day continuous observation period.
- **SC-002**: Samuel can add, query, and complete todos from his phone in under 30 seconds per interaction via the chat interface.
- **SC-003**: The agent correctly identifies and converts at least 80% of actionable emails (validated against a manual test set of 20 labelled messages) into todos or notifications, with zero false positives on clearly informational emails.
- **SC-004**: Calendar queries return results that accurately reflect the live Google Calendar within a 1-event margin (accounting for propagation delay).
- **SC-005**: The daemon runs continuously for at least 7 days without a crash or manual restart after initial deployment on the home server.
- **SC-006**: Voice memos spoken clearly in a quiet environment are transcribed and acted upon correctly at least 90% of the time.

## Assumptions

- Samuel is the sole user; no multi-user access control or authentication between Samuel and the agent is required for v1.
- The daemon runs on a home server (e.g., ai.lan.disasm.us) that is always on. It binds to all interfaces by default; Samuel controls network access via firewall rules.
- Google account credentials are obtained via a one-time OAuth2 device-flow or pre-existing token file; the agent does not need to manage browser-based OAuth redirect flows at runtime.
- ntfy.sh is used as a hosted service rather than self-hosted; the ntfy topic name is treated as a shared secret.
- The mobile chat interface is a minimal web page served directly by the daemon — no separate native mobile app is required for v1.
- Voice input from the phone is delivered as an uploaded audio file; real-time streaming audio from the phone is out of scope for v1.
- The existing agentix-whisper crate (already in the workspace) handles transcription; no external speech recognition service is needed.
- Gmail scanning runs on a polling schedule rather than IMAP IDLE; near-real-time email detection is not required.
- ESP32 hardware integration is fully deferred to a future feature branch; only interface seams are reserved in this phase.
- Persistent storage uses an embedded key-value store (sled) so the agent runs standalone without a PostgreSQL instance; PostgreSQL remains available in the workspace if future data needs become relational.
