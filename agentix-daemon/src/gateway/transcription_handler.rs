use super::{proxy, AppState};
use axum::{body::Body, extract::State, http::HeaderMap, response::Response};

pub async fn handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    proxy::forward(
        &state.whisper_socket,
        axum::http::Method::POST,
        "/v1/audio/transcriptions",
        headers,
        Body::from(body),
    )
    .await
}
