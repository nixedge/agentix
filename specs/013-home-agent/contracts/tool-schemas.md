# Agent Tool Schemas: agentix-agent

These are the JSON Schema definitions for tools registered with the `AgentLoop` from `agentix-harness`. Each tool implements the `Tool` trait.

---

## todo_list

List open household todos, optionally filtered.

```json
{
  "name": "todo_list",
  "description": "List household todos. Returns open todos by default, sorted by urgency (overdue first, then by due date).",
  "parameters": {
    "type": "object",
    "properties": {
      "include_completed": {
        "type": "boolean",
        "description": "Set to true to include completed todos. Default: false."
      },
      "search": {
        "type": "string",
        "description": "Optional keyword to filter todos by title."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of todos to return. Default: 20."
      }
    }
  }
}
```

---

## todo_create

Create a new household todo.

```json
{
  "name": "todo_create",
  "description": "Create a new household todo. Use this whenever the user mentions something they need to do.",
  "parameters": {
    "type": "object",
    "required": ["title"],
    "properties": {
      "title": {
        "type": "string",
        "description": "Clear, concise description of the task."
      },
      "due_at": {
        "type": "string",
        "description": "Optional due date/time in ISO 8601 format (e.g. '2026-08-20T09:00:00Z'). Infer from relative expressions like 'tomorrow' or 'next Monday'."
      },
      "notes": {
        "type": "string",
        "description": "Optional additional detail."
      }
    }
  }
}
```

---

## todo_complete

Mark a todo as done.

```json
{
  "name": "todo_complete",
  "description": "Mark an existing todo as completed.",
  "parameters": {
    "type": "object",
    "required": ["id"],
    "properties": {
      "id": {
        "type": "string",
        "description": "UUID of the todo to complete."
      }
    }
  }
}
```

---

## todo_update

Update a todo's due date or notes.

```json
{
  "name": "todo_update",
  "description": "Update the due date or notes of an existing todo.",
  "parameters": {
    "type": "object",
    "required": ["id"],
    "properties": {
      "id": { "type": "string", "description": "UUID of the todo." },
      "due_at": { "type": "string", "description": "New due date in ISO 8601." },
      "notes": { "type": "string", "description": "Replacement notes." }
    }
  }
}
```

---

## calendar_list_events

Fetch upcoming Google Calendar events.

```json
{
  "name": "calendar_list_events",
  "description": "List upcoming events from Google Calendar in a given time range.",
  "parameters": {
    "type": "object",
    "properties": {
      "time_min": {
        "type": "string",
        "description": "Start of range in ISO 8601 (default: now)."
      },
      "time_max": {
        "type": "string",
        "description": "End of range in ISO 8601 (default: 7 days from now)."
      },
      "max_results": {
        "type": "integer",
        "description": "Maximum events to return. Default: 10."
      }
    }
  }
}
```

---

## calendar_create_event

Create a new event in Google Calendar.

```json
{
  "name": "calendar_create_event",
  "description": "Create a new event in Samuel's Google Calendar.",
  "parameters": {
    "type": "object",
    "required": ["title", "start", "end"],
    "properties": {
      "title": { "type": "string", "description": "Event title." },
      "start": { "type": "string", "description": "Start time in ISO 8601." },
      "end": { "type": "string", "description": "End time in ISO 8601." },
      "location": { "type": "string", "description": "Optional location." },
      "description": { "type": "string", "description": "Optional event notes." }
    }
  }
}
```

---

## gmail_scan_inbox

Read recent unread emails and identify actionable items.

```json
{
  "name": "gmail_scan_inbox",
  "description": "Fetch recent unread emails from Gmail and summarise any that require action. Returns a list of actionable email summaries.",
  "parameters": {
    "type": "object",
    "properties": {
      "max_results": {
        "type": "integer",
        "description": "Max emails to check. Default: 20."
      }
    }
  }
}
```

---

## send_notification

Send a push notification to Samuel's phone via ntfy.sh.

```json
{
  "name": "send_notification",
  "description": "Send a push notification to Samuel's phone. Use for important reminders or alerts that need his attention immediately.",
  "parameters": {
    "type": "object",
    "required": ["title", "body"],
    "properties": {
      "title": { "type": "string", "description": "Short notification title (max 60 chars)." },
      "body": { "type": "string", "description": "Notification body text." },
      "priority": {
        "type": "string",
        "enum": ["low", "default", "high", "urgent"],
        "description": "Notification priority. Default: 'default'."
      }
    }
  }
}
```
