use agentix_assistant::Assistant;
use agentix_assistant_core::{DeviceTurnRequest, Error, Reminder, TellRequest, TellResponse};
use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[derive(Clone)]
struct HttpState {
    service: Arc<Assistant>,
    token: Option<Arc<str>>,
}

pub fn validate_token(token: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        (32..=512).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_graphic()),
        "assistant owner token must be 32..512 visible ASCII bytes"
    );
    Ok(())
}

pub fn routes(service: Arc<Assistant>, owner_token: Option<String>) -> anyhow::Result<Router> {
    if let Some(token) = &owner_token {
        validate_token(token)?;
    }
    Ok(Router::new()
        .route("/v1/assistant/tell", post(tell))
        .route("/v1/assistant/reminders/enroll", post(enroll))
        .route("/v1/assistant/reminders/snapshot", get(snapshot))
        .route("/v1/assistant/reminders/report", post(report))
        .layer(DefaultBodyLimit::max(16_384))
        .with_state(HttpState {
            service,
            token: owner_token.map(Arc::from),
        }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnRequest {
    device_id: Option<Uuid>,
    request: TellRequest,
}
#[derive(Serialize)]
struct TurnResponse {
    device_id: Option<Uuid>,
    response: TellResponse,
    reminder_receipts: Vec<Reminder>,
}

type HttpError = (StatusCode, Json<Value>);
fn error(status: StatusCode, code: &str, retry_same_request: bool) -> HttpError {
    (
        status,
        Json(json!({"error":code,"retry_same_request":retry_same_request})),
    )
}
fn authorized(state: &HttpState, headers: &HeaderMap) -> std::result::Result<(), HttpError> {
    let supplied = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if state
        .token
        .as_ref()
        .is_some_and(|token| !bool::from(supplied.as_bytes().ct_eq(token.as_bytes())))
    {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "owner_auth_required",
            false,
        ));
    }
    Ok(())
}
fn service_error(failure: Error) -> HttpError {
    match failure {
        Error::Invalid(_) => error(StatusCode::BAD_REQUEST, "invalid_request", false),
        Error::RequestConflict | Error::RevisionConflict { .. } | Error::AlreadyExists(_) => {
            error(StatusCode::CONFLICT, "device_or_request_conflict", false)
        }
        Error::NotFound(_) => error(StatusCode::NOT_FOUND, "reminder_not_found", false),
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
    }
}
fn parse_body<T: serde::de::DeserializeOwned>(
    body: std::result::Result<Bytes, BytesRejection>,
) -> std::result::Result<T, HttpError> {
    let body = body.map_err(|r| error(r.status(), "invalid_or_oversized_body", false))?;
    serde_json::from_slice(&body)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_request", false))
}
async fn tell(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: std::result::Result<Bytes, BytesRejection>,
) -> std::result::Result<Json<TurnResponse>, HttpError> {
    authorized(&state, &headers)?;
    let request: TurnRequest = parse_body(body)?;
    let turn = async {
        match request.device_id {
            Some(device_id) => {
                let result = state
                    .service
                    .tell_device(DeviceTurnRequest {
                        device_id,
                        request: request.request,
                    })
                    .await?;
                Ok(TurnResponse {
                    device_id: Some(device_id),
                    response: result.response,
                    reminder_receipts: result.reminder_receipts,
                })
            }
            None => Ok(TurnResponse {
                device_id: None,
                response: state.service.tell(request.request).await?,
                reminder_receipts: vec![],
            }),
        }
    };
    let response = tokio::time::timeout(Duration::from_secs(130), turn)
        .await
        .map_err(|_| {
            error(
                StatusCode::GATEWAY_TIMEOUT,
                "request_outcome_uncertain",
                true,
            )
        })?
        .map_err(service_error)?;
    Ok(Json(response))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Device {
    device_id: Uuid,
}
async fn enroll(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: std::result::Result<Bytes, BytesRejection>,
) -> std::result::Result<Json<agentix_assistant_core::ReminderSnapshot>, HttpError> {
    authorized(&state, &headers)?;
    let device: Device = parse_body(body)?;
    Ok(Json(
        state
            .service
            .reminder_store()
            .map_err(service_error)?
            .enroll(device.device_id)
            .await
            .map_err(service_error)?,
    ))
}
async fn snapshot(
    State(state): State<HttpState>,
    headers: HeaderMap,
    axum::extract::Query(device): axum::extract::Query<Device>,
) -> std::result::Result<Json<agentix_assistant_core::ReminderSnapshot>, HttpError> {
    authorized(&state, &headers)?;
    Ok(Json(
        state
            .service
            .reminder_store()
            .map_err(service_error)?
            .snapshot(device.device_id)
            .await
            .map_err(service_error)?,
    ))
}
async fn report(
    State(state): State<HttpState>,
    headers: HeaderMap,
    body: std::result::Result<Bytes, BytesRejection>,
) -> std::result::Result<Json<Value>, HttpError> {
    authorized(&state, &headers)?;
    let reports: agentix_assistant_core::ReminderReports = parse_body(body)?;
    Ok(Json(
        json!({"accepted":state.service.reminder_store().map_err(service_error)?.report(reports).await.map_err(service_error)?}),
    ))
}
