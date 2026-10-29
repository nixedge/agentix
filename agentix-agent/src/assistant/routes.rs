use agentix_assistant::Assistant;
use agentix_assistant_core::{Error, TellRequest};
use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use subtle::ConstantTimeEq;

#[derive(Clone)]
struct HttpState {
    service: Arc<Assistant>,
    token: Arc<str>,
}

pub fn validate_token(token: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        (32..=512).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_graphic()),
        "assistant owner token must be 32..512 visible ASCII bytes"
    );
    Ok(())
}

pub fn routes(service: Arc<Assistant>, owner_token: String) -> anyhow::Result<Router> {
    validate_token(&owner_token)?;
    Ok(Router::new()
        .route("/v1/assistant/tell", post(tell))
        .layer(DefaultBodyLimit::max(16_384))
        .with_state(HttpState {
            service,
            token: owner_token.into(),
        }))
}

type HttpError = (StatusCode, Json<Value>);
fn error(status: StatusCode, code: &str, retry_same_request: bool) -> HttpError {
    (
        status,
        Json(json!({"error":code,"retry_same_request":retry_same_request})),
    )
}

async fn tell(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: std::result::Result<Bytes, BytesRejection>,
) -> std::result::Result<Json<agentix_assistant_core::TellResponse>, HttpError> {
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if !bool::from(supplied.as_bytes().ct_eq(state.token.as_bytes())) {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "owner_auth_required",
            false,
        ));
    }
    let body =
        body.map_err(|rejection| error(rejection.status(), "invalid_or_oversized_body", false))?;
    let request: TellRequest = serde_json::from_slice(&body)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_request", false))?;
    let response = tokio::time::timeout(Duration::from_secs(130), state.service.tell(request))
        .await
        .map_err(|_| {
            error(
                StatusCode::GATEWAY_TIMEOUT,
                "request_outcome_uncertain",
                true,
            )
        })?
        .map_err(|failure| match failure {
            Error::Invalid(_) => error(StatusCode::BAD_REQUEST, "invalid_request", false),
            Error::RequestConflict => error(StatusCode::CONFLICT, "request_id_conflict", false),
            Error::Busy => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "assistant_busy_retry_original_request",
                true,
            ),
            _ => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "request_outcome_uncertain",
                true,
            ),
        })?;
    Ok(Json(response))
}
