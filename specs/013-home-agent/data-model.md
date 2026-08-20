# Data Model: agentix-agent Home Assistant

**Phase**: 1 | **Branch**: `013-home-agent` | **Date**: 2026-08-19

## Storage Technology

`sled` v0.34 — pure-Rust embedded key-value store. A single `sled::Db` is opened at startup from `db_path` (a directory). Each logical collection is a named `sled::Tree`. Values are serialised as `serde_json` for human-debuggability. All sled I/O must go through `tokio::task::spawn_blocking` since the sled API is synchronous.

Tests open a `sled::Config::new().temporary(true).open()` database — no filesystem cleanup needed.

---

## Trees (Collections)

### `todos` tree

**Key**: UUID v4 as a 16-byte big-endian array (enables lexicographic iteration in creation order).  
**Value**: JSON-serialised `Todo`.

```rust
pub struct Todo {
    pub id: Uuid,
    pub title: String,
    pub due_at: Option<DateTime<Utc>>,
    pub completed: bool,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub source: TodoSource,
    pub source_id: Option<String>,   // Gmail message ID, calendar event ID, etc.
    pub notes: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TodoSource {
    Manual,
    Gmail,
    Calendar,
}
```

**Access patterns**:
- Insert: generate UUID, serialise, `tree.insert(id_bytes, json_bytes)`
- List all open: iterate tree, deserialise each value, filter `!completed`
- Complete: fetch by key, update `completed = true`, `completed_at = now`, re-insert
- Search by title: iterate tree, filter by `title.to_lowercase().contains(query)`
- Overdue open: iterate tree, filter `!completed && due_at < (now - grace)`

### `todos_by_source` tree

Secondary index for Gmail deduplication. No SQL index equivalent — maintained manually.

**Key**: `{source}/{source_id}` as UTF-8 bytes (e.g., `"gmail/18f3a2b9c4d1e7f0"`)  
**Value**: UUID bytes of the corresponding todo.

**Invariant**: inserted atomically alongside the `todos` tree entry using a sled `transaction`.

---

### `notifications` tree

**Key**: `{kind}/{reference_id}` as UTF-8 bytes (e.g., `"todo_overdue/550e8400-..."`)  
**Value**: JSON-serialised `NotificationRecord`.

```rust
pub struct NotificationRecord {
    pub id: Uuid,
    pub kind: NotificationKind,
    pub reference_id: String,
    pub title: String,
    pub body: String,
    pub sent_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub retry_count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum NotificationKind {
    TodoOverdue,
    EventReminder,
    GmailAction,
}
```

**Access patterns**:
- Dedup check: `tree.get(key)?.map(|v| deserialise(v)).map(|r| r.sent_at.is_some())`
- Record attempt: upsert with incremented `retry_count`; abandon silently (log error) after 5 failures
- Mark sent: fetch, set `sent_at = now`, re-insert

---

### `gmail_cursor` tree

Single-entry tree tracking the Gmail scan position.

**Key**: `b"cursor"` (constant)  
**Value**: JSON-serialised `GmailCursor`.

```rust
pub struct GmailCursor {
    pub history_id: Option<String>,
    pub last_scanned_at: Option<DateTime<Utc>>,
}
```

---

### `sessions` tree

**Key**: session UUID as 16-byte big-endian array  
**Value**: JSON-serialised `Vec<serde_json::Value>` (OpenAI-format message objects `{role, content}`)

**Retention**: at daemon startup, iterate all sessions; delete entries where `updated_at` (embedded in the first scan of the serialised messages, or stored separately) is older than 7 days. A lightweight approach: store as `SessionRecord { messages, updated_at }`.

```rust
pub struct SessionRecord {
    pub messages: Vec<serde_json::Value>,
    pub updated_at: DateTime<Utc>,
}
```

---

## Validation Rules

These are enforced in store methods, not in sled itself:

- `Todo.title`: non-empty after trim; max 500 bytes.
- `NotificationRecord.retry_count`: if ≥ 5 and `sent_at` is None, log an error and skip — no further retry.
- Session messages: each element must be a JSON object with `role` (string) and `content` (string or null).

---

## Sled Transaction Pattern (dedup-safe todo insert)

When creating a todo from Gmail, both trees must be updated atomically:

```rust
// Pseudocode — actual impl uses sled::transaction
(&todos_tree, &todos_by_source_tree).transaction(|(todos, by_source)| {
    let source_key = format!("gmail/{}", message_id);
    if by_source.get(&source_key)?.is_some() {
        return Ok(None); // already exists
    }
    let todo = Todo::new(...);
    todos.insert(&todo.id.as_bytes(), serde_json::to_vec(&todo)?)?;
    by_source.insert(source_key.as_bytes(), todo.id.as_bytes())?;
    Ok(Some(todo))
})?
```

---

## Configuration (AgentConfig)

Loaded from `~/.config/agentix-agent/agent.toml` (or `--config` override).

```toml
# agentix-agent configuration
gateway_url = "http://localhost:11434"
local_model  = "qwen3:8b"
db_path      = "~/.local/share/agentix-agent/db"   # sled directory
bind_addr    = "0.0.0.0:8765"

[ntfy]
url   = "https://ntfy.sh"
topic = "my-secret-topic"

[google]
credentials_path = "~/.config/agentix-agent/google-credentials.json"
token_cache_path = "~/.config/agentix-agent/google-token.json"
calendar_id      = "primary"

[scheduler]
notification_interval_secs = 300    # 5 minutes
gmail_scan_interval_secs   = 900    # 15 minutes
event_reminder_lead_secs   = 1800   # notify 30 min before
overdue_grace_secs         = 300    # notify 5 min after due
```
