# Tasks: agentix-agent Home Assistant

**Input**: Design documents from `/specs/014-home-agent/`
**Branch**: `014-home-agent`
**Plan phases A→G** map to **Implementation phases 3→7** below.

## Format: `[ID] [P?] [Story] Description with file path`

- **[P]**: Can run in parallel (different files, no incomplete-task deps)
- **[US1–US4]**: User story this task belongs to (maps to spec.md priorities P1–P4)

---

## Phase 1: Setup (Project Initialization)

**Purpose**: New crate exists in workspace and compiles empty.

- [X] T001 Add `"agentix-agent"` to `members` array in root `Cargo.toml`; add a minimal `agentix-agent` package attribute to `flake.nix` using crane `buildPackage` with a placeholder source filter (full source scoping deferred to Phase 7 T044)
- [X] T002 Create `agentix-agent/Cargo.toml` with all dependencies: agentix-harness, agentix-api, axum 0.8, tokio full, reqwest 0.12, sled 0.34, yup-oauth2 12, serde+derive, serde_json, toml 0.8, anyhow, tracing, tracing-subscriber env-filter, chrono+serde, uuid+v4+serde, clap+derive, tokio-util (CancellationToken), metrics 0.22, metrics-exporter-prometheus 0.14; dev-dependencies: tempfile 3
- [X] T003 Create source skeleton: `agentix-agent/src/main.rs` (stub `fn main()`), empty `mod` declarations for `config`, `db`, `http`, `tools`, `scheduler`, `google`, `store`; create empty `agentix-agent/src/{config,db}.rs` and subdirs `http/`, `tools/`, `scheduler/`, `google/`, `store/`, `static/`; confirm `cargo check -p agentix-agent` passes

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Config loading, database pool, all 4 SQLite migrations, and all store modules ready. No user story work can begin until this phase is complete.

**⚠️ CRITICAL**: Completes before ANY user story phase.

- [X] T004 Write `agentix-agent/src/config.rs`: `AgentConfig` struct with fields `gateway_url`, `local_model`, `db_path`, `bind_addr`, and sub-structs `NtfyConfig { url, topic }`, `GoogleConfig { credentials_path, token_cache_path, calendar_id }`, `SchedulerConfig { notification_interval_secs, gmail_scan_interval_secs, event_reminder_lead_secs, overdue_grace_secs }`; implement `AgentConfig::load(path: &Path) -> anyhow::Result<AgentConfig>` via `toml::from_str`; expand `~` in path fields via `std::env::var("HOME")`
- [X] T005 Write `agentix-agent/src/db.rs`: `pub struct AgentDb { pub todos: sled::Tree, pub todos_by_source: sled::Tree, pub notifications: sled::Tree, pub gmail_cursor: sled::Tree, pub sessions: sled::Tree }`; `impl AgentDb { pub fn open(path: &str) -> anyhow::Result<Self> }` — calls `sled::open(path)`, opens all five named trees, returns `AgentDb`; derive `Clone` (sled::Tree is cheaply cloneable)
- [X] T006 [P] Write `agentix-agent/src/store/todos.rs`: `TodoStore(AgentDb)`; define `Todo` and `TodoSource` structs with serde; methods all use `spawn_blocking`: `insert(title, due_at, source, source_id, notes) -> anyhow::Result<Todo>` (generates UUID key, serialises JSON, atomically inserts into `todos` + `todos_by_source` via sled transaction — see data-model.md "Sled Transaction Pattern" for the canonical dedup-safe insert); `list(include_completed: bool, search: Option<&str>, limit: u32) -> anyhow::Result<Vec<Todo>>` (iterate tree, filter, sort by due_at); `complete(id: Uuid) -> anyhow::Result<()>`; `update(id: Uuid, due_at, notes) -> anyhow::Result<Todo>`; `find_by_source(source: &str, source_id: &str) -> anyhow::Result<Option<Todo>>` (lookup via `todos_by_source` tree); `overdue_open(grace_secs: i64) -> anyhow::Result<Vec<Todo>>` (iterate, filter `!completed && due_at < now - grace`)
- [X] T007 [P] Write `agentix-agent/src/store/notifications.rs`: `NotificationStore(AgentDb)`; define `NotificationRecord` and `NotificationKind` with serde; key format `"{kind}/{reference_id}"`; methods via `spawn_blocking`: `already_sent(reference_id, kind) -> anyhow::Result<bool>`; `record_attempt(kind, reference_id, title, body) -> anyhow::Result<Uuid>`; `mark_sent(kind, reference_id) -> anyhow::Result<()>`; `mark_failed(kind, reference_id) -> anyhow::Result<()>` (increments retry_count; logs error and returns Ok if retry_count >= 5)
- [X] T008 [P] Write `agentix-agent/src/store/sessions.rs`: `SessionStore(AgentDb)`; define `SessionRecord { messages: Vec<serde_json::Value>, updated_at: DateTime<Utc> }`; UUID key as 16-byte big-endian; methods via `spawn_blocking`: `load(session_id: Uuid) -> anyhow::Result<Vec<serde_json::Value>>`; `save(session_id: Uuid, messages: &[serde_json::Value]) -> anyhow::Result<()>`; `prune_older_than_days(days: i64) -> anyhow::Result<()>` (iterate all sessions, remove where `updated_at < now - days`)
- [X] T009 [P] Write `agentix-agent/src/store/gmail_cursor.rs`: `GmailCursorStore(AgentDb)`; define `GmailCursor { history_id: Option<String>, last_scanned_at: Option<DateTime<Utc>> }`; constant key `b"cursor"`; methods via `spawn_blocking`: `load() -> anyhow::Result<GmailCursor>`; `save(history_id: &str) -> anyhow::Result<()>`
- [X] T010 Write unit tests in each store module using `sled::Config::new().temporary(true).open()` (no tempfile needed — sled handles cleanup): `TodoStore`: `overdue_open` returns todos past grace; `find_by_source` dedup; `list` search filter; `TodoStore`: atomic transaction prevents duplicate when called twice with same source_id; `NotificationStore`: `already_sent` returns false before send, true after `mark_sent`; `SessionStore`: `prune_older_than_days` removes old sessions

**Checkpoint**: `cargo test -p agentix-agent` passes all store tests (sled temporary db); `cargo check -p agentix-agent` clean.

---

## Phase 3: User Story 1 — Proactive Push Notifications (Priority: P1) 🎯 MVP

**Goal**: Daemon starts, background scheduler fires every 5 min, overdue todos push to ntfy.sh. Independently usable with no Google credentials.

**Independent Test**: `agentix-agent todos add "test" --due <past-timestamp>` then `agentix-agent daemon` → overdue notification arrives in ntfy app within 5 minutes. No Google credentials needed. Calendar event reminders (US1 scenario 2) are validated in Phase 5.

- [X] T015 [US1] Write `agentix-agent/src/tools/ntfy.rs`: `NtfyClient { url: String, topic: String }` with `async fn send(title: &str, body: &str, priority: &str) -> anyhow::Result<()>` — builds `reqwest::Client`, POSTs to `{url}/{topic}` with headers `Title`, `Priority`, `Tags: agentix`; returns error on non-2xx
- [X] T016 [US1] Write `agentix-agent/src/scheduler/notification.rs`: async fn `run_notification_loop(store: TodoStore, notif: NotificationStore, ntfy: NtfyClient, config: SchedulerConfig)` — loops with `tokio::time::interval`; each tick calls `store.overdue_open(grace_secs)`, for each todo checks `notif.already_sent(id, "todo_overdue")`, if not: calls `ntfy.send(...)`, records result; also queries upcoming calendar events placeholder (no-op in US1, wired in US3)
- [X] T017 [US1] Write `agentix-agent/src/scheduler/mod.rs`: `pub fn spawn(state: Arc<AppState>)` — calls `tokio::spawn(run_notification_loop(...))` with cloned state references; `AppState` struct defined here holding `SqlitePool`, `NtfyClient`, `AgentConfig`, and `Option<GoogleAuth>` (None until US3)
- [X] T018 [US1] Expand `agentix-agent/src/main.rs`: `clap` derive CLI with subcommands `daemon` (starts HTTP + schedulers), `todos add --due <rfc3339>`, `todos list [--completed]`, `todos done <id>`, `notify test --title <t> --body <b>`; `todos` and `notify test` open the SQLite pool directly (no HTTP); `daemon` subcommand calls `scheduler::spawn` and starts Axum (stub router for now — just GET /health)
- [X] T019 [US1] Write `agentix-agent/src/http/health.rs`: `async fn health(state) -> Json<serde_json::Value>` returning `{"status","db","gateway","google_auth"}` — `db` is always `"ok"` if the handler runs (sled open succeeded at startup); `gateway` attempts a `HEAD {config.gateway_url}/health` with a 2s timeout and reports `"ok"` or `"unreachable"`; `google_auth` reports `"unconfigured"` (no credentials path), `"ok"`, or `"expired"` (wired fully in T036 Phase 5 — stub `"unconfigured"` for now); wire into `agentix-agent/src/http/mod.rs` at `GET /health`
- [X] T020 [US1] Write unit tests in `agentix-agent/src/scheduler/notification.rs` (or tests module): create two overdue todos, one already-sent notification record → verify only one ntfy POST is made; use a `MockNtfyClient` that records calls instead of HTTP; use in-memory SQLite

**Checkpoint**: `agentix-agent todos add "test" --due 2020-01-01T00:00:00Z && agentix-agent daemon` sends exactly one ntfy notification; re-run does not send duplicate.

---

## Phase 4: User Story 2 — Phone Chat Text + Voice (Priority: P2)

**Goal**: Mobile browser opens `/`, chats via text and voice, session context persists across turns.

**Independent Test**: `curl -X POST http://localhost:8765/chat -d '{"message":"list my todos"}' -b 'session=<uuid>'` returns a valid JSON reply with the `session` cookie set.

- [X] T021 [US2] Write `agentix-agent/static/index.html`: mobile-first single-page UI — chat message list, text input + send button, microphone button (triggers `MediaRecorder`); JS checks `MediaRecorder.isTypeSupported('audio/webm;codecs=opus')` → uses WebM/Opus or falls back to `audio/mp4`; uploads audio as `FormData` to `POST /chat/voice`; sends text to `POST /chat`; reads/writes `session` cookie; renders markdown replies as plain text (no external dependencies)
- [X] T022 [US2] Write `agentix-agent/src/http/ui.rs`: `GET /` handler returns `include_str!("../../static/index.html")` as `text/html`; wire into `agentix-agent/src/http/mod.rs`
- [X] T023 [US2] Write `agentix-agent/src/tools/todo.rs`: implement `agentix_harness::Tool` for four tools — `TodoListTool`, `TodoCreateTool`, `TodoCompleteTool`, `TodoUpdateTool`; each holds `TodoStore`; `parameters()` returns JSON schema from `contracts/tool-schemas.md`; `call(args)` dispatches to store and returns human-readable result string; implement `Clone` for all (needed for AgentLoop)
- [X] T024 [US2] Write `agentix-agent/src/tools/mod.rs`: `pub fn build_agent_loop(gateway_url: &str, model: &str, state: Arc<AppState>) -> AgentLoop` — constructs `AgentLoop::new(gateway_url, model)` and chains `.with_tool(Box::new(TodoListTool::new(...)))` etc. for all available tools; calendar and gmail tools added as `Option` (skipped if Google not configured)
- [X] T025 [US2] Write `agentix-agent/src/http/chat.rs` POST `/chat` handler: extract `session` cookie (generate UUID if absent); load session messages from `SessionStore`; prepend a system prompt with current date/time; build `AgentLoop` via `tools::build_agent_loop`; run loop; append user + assistant turns to session and save; return `{"reply": "...", "session": "<uuid>"}` with `Set-Cookie`
- [X] T026 [US2] Write `agentix-agent/src/http/chat.rs` POST `/chat/voice` handler: extract multipart `audio` field bytes; forward as `multipart/form-data` to `{gateway_url}/v1/audio/transcriptions` via reqwest; parse transcription text; if empty, return 422 with user-facing message; otherwise delegate to the same logic as POST `/chat` (DRY: extract shared `handle_text_message` fn)
- [X] T027 [US2] Write `agentix-agent/src/http/todos.rs`: `GET /todos` (query params: `completed`, `limit`, `q` → TodoStore::list), `POST /todos` (JSON body → TodoStore::insert, validate non-empty title), `PATCH /todos/:id` (JSON body → TodoStore::complete or update); wire all into `agentix-agent/src/http/mod.rs`
- [X] T028 [US2] Update `agentix-agent/src/main.rs` daemon subcommand: bind Axum on `config.bind_addr` (default `[::]:8765` — dual-stack IPv4+IPv6 on all interfaces); serve all routes from `http::mod`, pass `Arc<AppState>` as Axum extension; log startup address; install `tokio::signal::ctrl_c()` and `unix::signal(SIGTERM)` handlers that trigger a graceful shutdown — cancel the scheduler tasks via a `CancellationToken` (tokio-util) and wait for the Axum server to drain in-flight requests before exiting
- [X] T029 [US2] Write integration tests in `agentix-agent/src/http/chat.rs` test module: stub `AgentLoop` (via a fake tool that always returns "ok"); POST /chat with no session → receives reply + `Set-Cookie`; second POST with session cookie → agent receives prior context in message history; use `axum::test` or `tower::ServiceExt`

**Checkpoint**: Browser on LAN at `http://<server>:8765/` can chat with the agent, add todos via natural language, and complete them. Session context persists across page refresh.

---

## Phase 5: User Story 3 — Calendar Queries (Priority: P3)

**Goal**: Agent can answer "what's on my plate this week?" with real Google Calendar data; upcoming events trigger ntfy reminders.

**Independent Test**: `agentix-agent calendar today` prints real events from Google Calendar; chat "what's happening tomorrow?" returns calendar-informed answer.

- [X] T030 [US3] Write `agentix-agent/src/google/auth.rs`: `GoogleAuth` struct wrapping `yup_oauth2::Authenticator`; `GoogleAuth::init(credentials_path, token_cache_path) -> anyhow::Result<GoogleAuth>` — loads `ApplicationSecret` from credentials JSON, builds `DeviceFlowAuthenticator` with `persist_tokens_to_disk(token_cache_path)`, prints device URL/code to stderr if interaction needed; expose `async fn token() -> anyhow::Result<String>` returning bearer token string
- [X] T031 [US3] Write `agentix-agent/src/google/calendar.rs`: `CalendarClient { auth: Arc<GoogleAuth>, calendar_id: String, http: reqwest::Client }`; methods: `async fn list_events(time_min: DateTime<Utc>, time_max: DateTime<Utc>, max: u32) -> anyhow::Result<Vec<CalendarEvent>>` (GET /calendar/v3/…, parse JSON); `async fn create_event(title, start, end, location, description) -> anyhow::Result<CalendarEvent>` (POST /calendar/v3/…); define `CalendarEvent { summary, start, end, location, id, attendees: Vec<String> }` with serde
- [X] T032 [P] [US3] Write `agentix-agent/src/tools/calendar.rs`: implement `Tool` for `CalendarListEventsTool` and `CalendarCreateEventTool`; schemas from `contracts/tool-schemas.md`; `call` delegates to `CalendarClient`; returns formatted event list string
- [X] T033 [US3] Add `CalendarListEventsTool` and `CalendarCreateEventTool` to `tools::build_agent_loop` in `agentix-agent/src/tools/mod.rs` when `AppState.google_auth.is_some()`
- [X] T034 [US3] Expand `agentix-agent/src/scheduler/notification.rs` with upcoming-event reminders: query `CalendarClient::list_events(now, now + event_reminder_lead_secs)` each tick; for each event check `NotificationStore::already_sent(event.id, "event_reminder")`; if not, send ntfy notification with event title and start time; record sent
- [X] T035 [US3] Add `calendar today` subcommand to `agentix-agent/src/main.rs`: init `GoogleAuth`, call `CalendarClient::list_events(today_start, today_end)`, print formatted list to stdout
- [X] T036 [US3] Handle expired/unconfigured Google token in `agentix-agent/src/google/auth.rs` and `agentix-agent/src/http/health.rs`: `GET /health` reports `"google_auth": "ok" | "expired" | "unconfigured"` based on whether GoogleAuth is Some and token refresh succeeds
- [X] T037 [US3] Write fixture-based tests for `agentix-agent/src/google/calendar.rs`: use `wiremock` or `httpmock` to serve recorded Google Calendar API responses; verify `list_events` parses event list and `create_event` sends correct JSON body; no live Google auth needed

**Checkpoint**: `agentix-agent calendar today` prints calendar events; chat "what's on this week?" returns events + open todos; ntfy app receives event reminders 30 min before events.

---

## Phase 6: User Story 4 — Gmail → Todos (Priority: P4)

**Goal**: Background Gmail scan creates todos from actionable emails; no duplicate todos across repeated scans.

**Independent Test**: Send a test email with subject "your package will arrive tomorrow" → within one scan interval (15 min) a todo appears in `agentix-agent todos list`.

- [X] T038 [US4] Write `agentix-agent/src/google/gmail.rs`: `GmailClient { auth: Arc<GoogleAuth>, http: reqwest::Client }`; methods: `async fn list_unread(max: u32) -> anyhow::Result<Vec<String>>` (returns message IDs, GET /gmail/v1/users/me/messages?q=is:unread); `async fn get_message_meta(id: &str) -> anyhow::Result<GmailMessage>` (GET …/messages/{id}?format=metadata&metadataHeaders=Subject,From,Date — parse headers + snippet); define `GmailMessage { id, subject, from, date, snippet }`
- [X] T039 [US4] Write `agentix-agent/src/scheduler/gmail_scan.rs`: `async fn run_gmail_scan_loop(state: Arc<AppState>)` — `tokio::time::interval` at `gmail_scan_interval_secs`; each tick: list unread emails; for each message check `TodoStore::find_by_source("gmail", message_id)` — skip if already processed; build a short prompt with subject+snippet and run a lightweight `AgentLoop` to classify as actionable/not and extract a task title; if actionable: `TodoStore::insert(title, due_at, "gmail", message_id, snippet)`; update `GmailCursorStore`
- [X] T040 [US4] Write `agentix-agent/src/tools/gmail.rs`: implement `Tool` for `GmailScanInboxTool` — on-demand version of the scan (for use from chat: "check my email"); schema from `contracts/tool-schemas.md`; delegates to `GmailClient::list_unread` + `get_message_meta` for up to `max_results` messages; returns formatted summary
- [X] T041 [US4] Add `GmailScanInboxTool` to `tools::build_agent_loop` in `agentix-agent/src/tools/mod.rs` when `AppState.google_auth.is_some()`
- [X] T042 [US4] Add gmail scan task to `agentix-agent/src/scheduler/mod.rs` `spawn()`: `tokio::spawn(gmail_scan::run_gmail_scan_loop(Arc::clone(&state)))` alongside the notification loop
- [X] T043 [US4] Write fixture-based deduplication integration test in `agentix-agent/src/scheduler/gmail_scan.rs`: mock Gmail API returns same message ID on two consecutive scan calls; verify `TodoStore` contains exactly one todo for that source_id after both scans; use in-memory SQLite + `wiremock`

**Checkpoint**: `agentix-agent daemon` running; sending a test email → todo appears in list within one poll interval; sending same email again does not create a duplicate.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Nix build, NixOS service, code quality gates, documentation.

- [X] T044 Add `agentix-agent` package attribute to `flake.nix`: use crane `buildPackage` with `mkWorkspaceSrc`-scoped source filter matching `agentix-agent/`, `agentix-harness/`, `agentix-api/`; ensure `nix build .#agentix-agent` succeeds
- [X] T045 [P] Add NixOS service module for agentix-agent in `nix/modules/agentix-agent.nix`: `services.agentix-agent` options: `enable`, `configFile` (path), `user`, `group`; generates a `systemd.services.agentix-agent` unit with `ExecStart`, `Restart=on-failure`, working dir, and `EnvironmentFile` for any secrets not in the config file
- [X] T046 [P] Run `cargo fmt --all --check` and fix all formatting issues in `agentix-agent/` (rustfmt)
- [X] T047 [P] Run `cargo clippy -p agentix-agent -- -D warnings` and resolve all `unwrap_used`, `expect_used`, and other lint violations; replace `.unwrap()` / `.expect()` with `?` or explicit `anyhow::bail!`; add `// SAFETY:` comments for any `unsafe` blocks
- [X] T048 Verify `nix build .#agentix-agent` succeeds after T044; fix any Nix build failures (missing SQL migration files in source filter, missing static/ assets, link flags)
- [X] T049 [P] Update `ARCHITECTURE.md`: add `agentix-agent` row to crate inventory table; add `Port 8765` to port registry; document Complexity Tracking justifications (second port, no bubblewrap jail) per plan.md
- [X] T050 Add `agentix-agent` to CI test matrix (wherever other crates are tested): `cargo test -p agentix-agent`; ensure migration files and static assets are present in CI environment — N/A: no CI pipeline exists in this repo yet
- [X] T051 Add Prometheus metrics endpoint to `agentix-agent/src/http/mod.rs`: add `metrics` crate + `metrics-exporter-prometheus` to `agentix-agent/Cargo.toml`; register `GET /metrics` route returning Prometheus text format; instrument key counters: `agentix_notifications_sent_total{kind}`, `agentix_notifications_failed_total{kind}`, `agentix_gmail_scan_total`, `agentix_chat_requests_total{type}` (text/voice); instrument gauges: `agentix_todos_open`, `agentix_notification_loop_last_run_unix` (for alerting on missed scheduler ticks); place counter increments in the notification scheduler (T016/T034) and chat handlers (T025/T026); document Prometheus scrape target in quickstart.md (`scrape_configs: - job_name: agentix-agent, static_configs: - targets: [<server>:8765]`)
- [X] T052 [P] Register ESP32 voice seam stub in `agentix-agent/src/http/mod.rs`: `POST /voice/stream` returns `501 Not Implemented` with body `{"error":"ESP32 voice stream not yet implemented","seam":"reserved"}` and a comment `// SEAM: reserved for ESP32 PCM streaming — see contracts/http-api.md`; wire into the router alongside other routes

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — start immediately
- **Phase 2 (Foundational)**: Depends on Phase 1; **BLOCKS all user story phases**
- **Phase 3 (US1 — Notifications)**: Depends on Phase 2 only; no other story deps
- **Phase 4 (US2 — Phone Chat)**: Depends on Phase 2; can start in parallel with Phase 3
- **Phase 5 (US3 — Calendar)**: Depends on Phases 2 and 3 (notification scheduler extended); can start after Phase 3 checkpoint
- **Phase 6 (US4 — Gmail)**: Depends on Phase 5 (GoogleAuth reused); can start after Phase 5 checkpoint
- **Phase 7 (Polish)**: Depends on all story phases complete

### User Story Dependencies

- **US1 (P1)**: Independent after Foundational — deliverable as MVP
- **US2 (P2)**: Independent after Foundational — can develop alongside US1
- **US3 (P3)**: Requires US1 notification scheduler scaffolding (T034 extends T016)
- **US4 (P4)**: Requires US3 Google auth module (T038 reuses `GoogleAuth` from T030)

### Within Each Phase

- Models/stores before services
- Services before HTTP handlers
- HTTP handlers before integration tests
- Store tests can be written alongside stores (same file)

### Parallel Opportunities

- **Phase 2**: T005 (db.rs) must come first; T006, T007, T008, T009 (four store modules) can then run in parallel; T010 (tests) runs after all stores
- **Phase 3**: T015, T016 can run in parallel (different files); T020 after T015/T016
- **Phase 4**: T021 (HTML), T023 (TodoTool), T027 (todos HTTP) can all run in parallel
- **Phase 5**: T031 (CalendarTool) can run in parallel with T030 (GoogleAuth)
- **Phase 7**: T045, T046, T047, T049, T052 can all run in parallel; T051 (metrics) depends on T025/T026 handlers existing but otherwise independent

---

## Parallel Example: Phase 4 (US2)

```
# These three tasks touch different files — run concurrently:
Task T021: Write agentix-agent/static/index.html
Task T023: Write agentix-agent/src/tools/todo.rs
Task T027: Write agentix-agent/src/http/todos.rs

# T024 and T025 depend on T023 (TodoTool) — run after:
Task T024: POST /chat handler (agentix-agent/src/http/chat.rs)
Task T025: POST /chat/voice handler (agentix-agent/src/http/chat.rs)

# T028 depends on T024+T025 — runs last in this phase:
Task T028: daemon subcommand wiring (agentix-agent/src/main.rs)
```

---

## Implementation Strategy

### MVP (US1 only — Phases 1–3)

1. Complete Phase 1: Setup (T001–T003)
2. Complete Phase 2: Foundational (T004–T014)
3. Complete Phase 3: US1 Notifications (T015–T020)
4. **STOP and VALIDATE**: `agentix-agent daemon` → overdue todo → ntfy notification on phone
5. Ship MVP — Samuel gets proactive reminders

### Incremental Delivery

- **After Phase 4** (US2): Samuel can chat with the agent from his phone → full interactive experience
- **After Phase 5** (US3): Calendar-aware replies and event reminders
- **After Phase 6** (US4): Ambient email awareness — todos created automatically
- **After Phase 7**: Production-quality Nix service

### Notes

- `[P]` tasks touch different files and can be parallelised across agents or developers
- Story labels enable traceability back to spec.md acceptance scenarios
- Each phase ends with a named checkpoint — validate before moving to the next phase
- SQL migrations are numbered and must be applied in order; do not renumber
- `static/index.html` is embedded via `include_str!` — it must exist at compile time; add to `.gitignore` exemptions if needed
- The `/voice/stream` ESP32 endpoint (reserved in `contracts/http-api.md`) is intentionally absent from these tasks — do not implement it
