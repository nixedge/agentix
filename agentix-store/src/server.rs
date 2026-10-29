//! HTTP router for a single shared local Store; only the server binary binds a port.
use crate::{
    transport::{
        ApiError, Info, Operation, Request as StoreRequest, Response as StoreResponse, PROTOCOL,
    },
    Store,
};
use anyhow::{ensure, Result};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
struct ApiState {
    store: Store,
    authorization: Option<Arc<Vec<u8>>>,
    active: Arc<Semaphore>,
}
fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
        .into_response()
}
/// The token grants full trusted access, including index writes and deletion.
pub fn router(store: Store, token: Option<String>) -> Result<Router> {
    ensure!(
        store.remote.is_none(),
        "storage server requires a local Store"
    );
    ensure!(
        token
            .as_ref()
            .is_none_or(|token| token.len() >= 32 && token.bytes().all(|b| b.is_ascii_graphic())),
        "invalid storage credential"
    );
    let state = ApiState {
        store,
        authorization: token.map(|token| Arc::new(format!("Bearer {token}").into_bytes())),
        active: Arc::new(Semaphore::new(64)),
    };
    Ok(Router::new()
        .route("/v1/info", get(info))
        .route("/v1/store", post(execute))
        .layer(DefaultBodyLimit::max(32 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), authorize))
        .with_state(state))
}
async fn authorize(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    if let Some(authorization) = &state.authorization {
        let supplied = request
            .headers()
            .get("authorization")
            .map(|h| h.as_bytes())
            .unwrap_or_default();
        // Compare every byte of equal-length credentials; never log either credential.
        let same = supplied.len() == authorization.len()
            && supplied
                .iter()
                .zip(authorization.iter())
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                == 0;
        if !same {
            return error(StatusCode::UNAUTHORIZED, "storage authorization required");
        }
    }
    let Ok(_permit) = state.active.try_acquire() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "storage server is at capacity; retry later",
        );
    };
    next.run(request).await
}
async fn info(State(state): State<ApiState>) -> Json<Info> {
    Json(Info {
        protocol: PROTOCOL,
        schema: crate::VERSION.into(),
        model: state.store.model.clone(),
        dimensions: state.store.dimensions,
    })
}
async fn execute(
    State(state): State<ApiState>,
    payload: Result<Json<StoreRequest>, JsonRejection>,
) -> Response {
    let request = match payload {
        Ok(Json(request)) => request,
        Err(e) => return error(e.status(), e.body_text()),
    };
    if request.protocol != PROTOCOL
        || request.model != state.store.model
        || request.dimensions != state.store.dimensions
    {
        return error(
            StatusCode::CONFLICT,
            "storage protocol/model/dimensions mismatch",
        );
    }
    match dispatch(&state.store, request.operation).await {
        Ok(value) => Json(StoreResponse { value }).into_response(),
        Err(e) => error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
    }
}
async fn dispatch(store: &Store, operation: Operation) -> Result<Value> {
    macro_rules! result {
        ($call:expr) => {
            serde_json::to_value($call.await?).map_err(Into::into)
        };
    }
    match operation {
        Operation::Repositories { projects } => result!(store.repositories(&projects)),
        Operation::Rows { name, filter } => result!(store.rows(&name, &filter)),
        Operation::Count { name, filter } => result!(store.count(&name, &filter)),
        Operation::Put { name, records } => result!(store.put(&name, records)),
        Operation::ReplaceFile {
            name,
            repo,
            file,
            records,
        } => result!(store.replace_file(&name, &repo, &file, records)),
        Operation::RetagFile {
            name,
            repo,
            file,
            project,
            mtime,
        } => result!(store.retag_file(&name, &repo, &file, project.as_deref(), mtime)),
        Operation::TagRepository { repo, project } => {
            result!(store.tag_repository(&repo, project.as_deref()))
        }
        Operation::Delete { name, filter } => result!(store.delete(&name, &filter)),
        Operation::Maintain { name } => result!(store.maintain(&name)),
        Operation::MaintainIfNeeded { name } => result!(store.maintain_if_needed(&name)),
        Operation::Keyword {
            name,
            text,
            filter,
            limit,
        } => result!(store.keyword(&name, &text, &filter, limit)),
        Operation::Hybrid {
            name,
            text,
            vector,
            filter,
            limit,
        } => result!(store.hybrid(&name, &text, &vector, &filter, limit)),
        Operation::Metadata { namespace, key } => result!(store.metadata(&namespace, &key)),
        Operation::MetadataRows { namespace } => result!(store.metadata_rows(&namespace)),
        Operation::SetMetadata {
            namespace,
            key,
            value,
        } => result!(store.set_metadata(&namespace, &key, value)),
        Operation::CompareMetadata {
            namespace,
            key,
            expected,
            value,
        } => result!(store.compare_metadata(&namespace, &key, expected, value)),
    }
}
