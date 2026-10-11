use super::AppState;
use agentix_api::agent::CloudProvider;
use agentix_router::ValidatedEscalation;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub async fn handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let request = match agentix_api::wire::decode_request(&body) {
        Ok(request) => request,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid_escalation"),
    };
    if let Some(key) = &state.config.agentix_api_key {
        if headers.get("authorization").and_then(|v| v.to_str().ok())
            != Some(format!("Bearer {key}").as_str())
        {
            return failure(StatusCode::FORBIDDEN, "escalation_denied");
        }
    }
    match state
        .model_router
        .validate_escalation(request, state.config.allow_cloud_escalation)
    {
        Ok(validated) => execute(&state, &validated).await,
        Err(code) => failure(StatusCode::FORBIDDEN, code),
    }
}

async fn execute(state: &AppState, validated: &ValidatedEscalation) -> Response {
    let request = validated.request();
    let prompt = format!("Evidence already gathered locally:\n{}\n\nWhy local sources are insufficient:\n{}\n\nSpecific question:\n{}", request.knowledge.as_str(), request.local_insufficiency.as_str(), request.question.as_str());
    let (url, key) = match request.target.provider {
        CloudProvider::OpenAi => (
            format!(
                "{}/v1/chat/completions",
                state
                    .config
                    .openai_base_url
                    .as_deref()
                    .unwrap_or("https://api.openai.com")
                    .trim_end_matches('/')
            ),
            &state.config.openai_api_key,
        ),
        CloudProvider::OpenRouter => (
            "https://openrouter.ai/api/v1/chat/completions".into(),
            &state.config.openrouter_api_key,
        ),
        CloudProvider::Anthropic => (
            format!(
                "{}/v1/messages",
                state
                    .config
                    .anthropic_base_url
                    .as_deref()
                    .unwrap_or("https://api.anthropic.com")
                    .trim_end_matches('/')
            ),
            &state.config.anthropic_api_key,
        ),
    };
    let Some(key) = key else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, "provider_not_configured");
    };
    let body = json!({"model":request.target.model.as_str(),"messages":[{"role":"user","content":prompt}],"max_tokens":4096,"stream":false});
    let mut outgoing = state.http.post(url).json(&body);
    if request.target.provider == CloudProvider::Anthropic {
        outgoing = outgoing
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01");
    } else {
        outgoing = outgoing.bearer_auth(key);
    }
    let mut response = match outgoing.send().await {
        Ok(response) if response.status().is_success() => response,
        _ => return failure(StatusCode::BAD_GATEWAY, "provider_request_failed"),
    };
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len().saturating_add(chunk.len()) <= 1024 * 1024 => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => return failure(StatusCode::BAD_GATEWAY, "provider_response_limit"),
        }
    }
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return failure(StatusCode::BAD_GATEWAY, "provider_response_invalid"),
    };
    let answer = if request.target.provider == CloudProvider::Anthropic {
        value["content"].as_array().map(|parts| {
            parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<String>()
        })
    } else {
        value["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
    };
    match answer {
        Some(answer) if !answer.trim().is_empty() => {
            Json(json!({"schema_version":1,"answer":answer,"context":request.context}))
                .into_response()
        }
        _ => failure(StatusCode::BAD_GATEWAY, "provider_response_invalid"),
    }
}

pub fn failure(status: StatusCode, code: &str) -> Response {
    (
        status,
        Json(json!({"error":{"code":code,"message":code,"retryable":false}})),
    )
        .into_response()
}
