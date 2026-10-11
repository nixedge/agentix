use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::{scheduler::AppState, store::todos::TodoSource};

#[derive(Deserialize)]
pub struct CreateTodoRequest {
    pub title: String,
    pub due_at: Option<DateTime<Utc>>,
    pub notes: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateTodoRequest {
    pub due_at: Option<serde_json::Value>,
    pub notes: Option<serde_json::Value>,
    pub completed: Option<bool>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub completed: Option<bool>,
    pub q: Option<String>,
}

pub async fn list_todos(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let store = state.todo_store();
    let todos = store
        .list(q.completed.unwrap_or(false), q.q, 100)
        .await
        .map_err(|e| server_err(e.to_string()))?;
    Ok(Json(json!(todos)))
}

pub async fn create_todo(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTodoRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let store = state.todo_store();
    let todo = store
        .insert(req.title, req.due_at, TodoSource::Manual, None, req.notes)
        .await
        .map_err(|e| server_err(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(json!(todo))))
}

pub async fn update_todo(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateTodoRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let store = state.todo_store();

    if req.completed == Some(true) {
        store
            .complete(id)
            .await
            .map_err(|e| server_err(e.to_string()))?;
    }

    let due_at = parse_optional_datetime(&req.due_at).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
    })?;

    let notes = req.notes.as_ref().map(|v| {
        if v.is_null() {
            None
        } else {
            v.as_str().map(str::to_owned)
        }
    });

    if due_at.is_some() || notes.is_some() {
        let todo = store
            .update(id, due_at, notes)
            .await
            .map_err(|e| server_err(e.to_string()))?;
        return Ok(Json(json!(todo)));
    }

    let todos = store
        .list(true, None, 1000)
        .await
        .map_err(|e| server_err(e.to_string()))?;
    let todo = todos
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))))?;
    Ok(Json(json!(todo)))
}

fn parse_optional_datetime(
    v: &Option<serde_json::Value>,
) -> anyhow::Result<Option<Option<DateTime<Utc>>>> {
    match v {
        None => Ok(None),
        Some(val) if val.is_null() => Ok(Some(None)),
        Some(val) => {
            let s = val
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("due_at must be a string or null"))?;
            let dt = DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| anyhow::anyhow!("invalid due_at: {e}"))?;
            Ok(Some(Some(dt)))
        }
    }
}

fn server_err(msg: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}
