use agentix_harness::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::store::{todos::TodoSource, TodoStore};

pub struct TodoListTool(pub TodoStore);
pub struct TodoCreateTool(pub TodoStore);
pub struct TodoCompleteTool(pub TodoStore);
pub struct TodoUpdateTool(pub TodoStore);

#[async_trait]
impl Tool for TodoListTool {
    fn name(&self) -> &str {
        "todo_list"
    }

    fn description(&self) -> &str {
        "List todos. Optionally filter by completion status and search query."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_completed": {
                    "type": "boolean",
                    "description": "Whether to include completed todos (default false)"
                },
                "query": {
                    "type": "string",
                    "description": "Filter todos by title substring"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default 50)"
                }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let include_completed = args["include_completed"].as_bool().unwrap_or(false);
        let query = args["query"].as_str().map(str::to_owned);
        let limit = args["limit"].as_u64().unwrap_or(50) as u32;
        let todos = self.0.list(include_completed, query, limit).await?;
        Ok(serde_json::to_string(&todos)?)
    }
}

#[async_trait]
impl Tool for TodoCreateTool {
    fn name(&self) -> &str {
        "todo_create"
    }

    fn description(&self) -> &str {
        "Create a new todo item."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["title"],
            "properties": {
                "title": {
                    "type": "string",
                    "description": "The todo title"
                },
                "due_at": {
                    "type": "string",
                    "description": "Optional ISO 8601 due date/time"
                },
                "notes": {
                    "type": "string",
                    "description": "Optional notes"
                }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let title = args["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("title is required"))?
            .to_owned();
        let due_at = args["due_at"]
            .as_str()
            .map(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .map_err(|e| anyhow::anyhow!("invalid due_at: {e}"))
            })
            .transpose()?;
        let notes = args["notes"].as_str().map(str::to_owned);
        let todo = self
            .0
            .insert(title, due_at, TodoSource::Manual, None, notes)
            .await?;
        Ok(serde_json::to_string(&todo)?)
    }
}

#[async_trait]
impl Tool for TodoCompleteTool {
    fn name(&self) -> &str {
        "todo_complete"
    }

    fn description(&self) -> &str {
        "Mark a todo as completed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Todo UUID"
                }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let id_str = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("id is required"))?;
        let id = uuid::Uuid::parse_str(id_str).map_err(|e| anyhow::anyhow!("invalid UUID: {e}"))?;
        self.0.complete(id).await?;
        Ok(format!("{{\"ok\":true,\"id\":\"{id}\"}}"))
    }
}

#[async_trait]
impl Tool for TodoUpdateTool {
    fn name(&self) -> &str {
        "todo_update"
    }

    fn description(&self) -> &str {
        "Update a todo's due date or notes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Todo UUID"
                },
                "due_at": {
                    "type": ["string", "null"],
                    "description": "New due date (ISO 8601), or null to clear"
                },
                "notes": {
                    "type": ["string", "null"],
                    "description": "New notes, or null to clear"
                }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let id_str = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("id is required"))?;
        let id = uuid::Uuid::parse_str(id_str).map_err(|e| anyhow::anyhow!("invalid UUID: {e}"))?;

        let due_at = if args["due_at"].is_null() {
            Some(None)
        } else if let Some(s) = args["due_at"].as_str() {
            let dt = chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&chrono::Utc))
                .map_err(|e| anyhow::anyhow!("invalid due_at: {e}"))?;
            Some(Some(dt))
        } else {
            None
        };

        let notes = if args["notes"].is_null() {
            Some(None)
        } else {
            args["notes"].as_str().map(|s| Some(s.to_owned()))
        };

        let todo = self.0.update(id, due_at, notes).await?;
        Ok(serde_json::to_string(&todo)?)
    }
}
