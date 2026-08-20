use super::{proxy, AppState};
use axum::{body::Body, extract::State, http::HeaderMap, response::Response};

// ── /api/pull ─────────────────────────────────────────────────────────────────

pub async fn pull_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/api/pull",
        headers,
        Body::from(body),
    )
    .await
}

// ── /api/delete ───────────────────────────────────────────────────────────────

pub async fn delete_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::DELETE,
        "/api/delete",
        headers,
        Body::from(body),
    )
    .await
}

// ── /api/tags ─────────────────────────────────────────────────────────────────

pub async fn tags_handler(State(state): State<AppState>) -> Response {
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::GET,
        "/api/tags",
        HeaderMap::new(),
        Body::empty(),
    )
    .await
}

// ── /api/show ─────────────────────────────────────────────────────────────────

pub async fn show_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy::forward(
        &state.llama_socket,
        axum::http::Method::POST,
        "/api/show",
        headers,
        Body::from(body),
    )
    .await
}
