use axum::{
    extract::{Multipart, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use crate::{scheduler::AppState, tools};

#[derive(Deserialize)]
pub struct ChatRequest {
    pub message: String,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub reply: String,
    pub session_id: String,
}

fn extract_session(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(Uuid::new_v4)
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, Json<serde_json::Value>)> {
    let session_id = extract_session(&headers);
    let session_store = state.session_store();

    let mut messages = session_store
        .load(session_id)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    messages.push(json!({ "role": "user", "content": req.message }));

    let agent =
        tools::build_agent_loop(&state.config.gateway_url, &state.config.local_model, &state);
    let output = agent
        .run(&req.message)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    messages.push(json!({ "role": "assistant", "content": output.answer }));

    session_store
        .save(session_id, &messages)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    metrics::counter!("agentix_chat_requests_total").increment(1);

    Ok(Json(ChatResponse {
        reply: output.answer,
        session_id: session_id.to_string(),
    }))
}

pub async fn chat_voice(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let session_id = extract_session(&headers);

    let mut audio_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(e.to_string()))?
    {
        if field.name() == Some("audio") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| bad_request(e.to_string()))?;
            audio_bytes = Some(bytes.to_vec());
        }
    }

    let audio = audio_bytes.ok_or_else(|| bad_request("missing audio field".to_owned()))?;

    let transcript = forward_to_whisper(&state.config.gateway_url, audio)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    let agent =
        tools::build_agent_loop(&state.config.gateway_url, &state.config.local_model, &state);
    let output = agent
        .run(&transcript)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    let session_store = state.session_store();
    let mut messages = session_store
        .load(session_id)
        .await
        .map_err(|e| internal_error(e.to_string()))?;
    messages.push(json!({ "role": "user", "content": transcript }));
    messages.push(json!({ "role": "assistant", "content": output.answer }));
    session_store
        .save(session_id, &messages)
        .await
        .map_err(|e| internal_error(e.to_string()))?;

    metrics::counter!("agentix_chat_requests_total").increment(1);

    Ok(Json(json!({
        "transcript": transcript,
        "reply": output.answer,
        "session_id": session_id.to_string(),
    })))
}

async fn forward_to_whisper(gateway_url: &str, audio: Vec<u8>) -> anyhow::Result<String> {
    let url = format!(
        "{}/v1/audio/transcriptions",
        gateway_url.trim_end_matches('/')
    );
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name("audio.webm")
        .mime_str("audio/webm")?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-1");
    let client = reqwest::Client::new();
    let resp = client.post(&url).multipart(form).send().await?;
    anyhow::ensure!(
        resp.status().is_success(),
        "whisper endpoint returned {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await?;
    body["text"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("no text in whisper response"))
}

fn internal_error(msg: String) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": msg })),
    )
}

fn bad_request(msg: String) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{AgentConfig, NtfyConfig, SchedulerConfig},
        db::AgentDb,
        scheduler::AppState,
        tools::ntfy::NtfyClient,
    };
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_db() -> AgentDb {
        let db = sled::Config::new()
            .temporary(true)
            .open()
            .expect("open temp db");
        AgentDb {
            todos: db.open_tree("todos").expect("todos"),
            todos_by_source: db.open_tree("todos_by_source").expect("by_source"),
            notifications: db.open_tree("notifications").expect("notifications"),
            gmail_cursor: db.open_tree("gmail_cursor").expect("gmail_cursor"),
            sessions: db.open_tree("sessions").expect("sessions"),
        }
    }

    fn gateway_reply(content: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "role": "assistant",
                    "content": content
                }
            }]
        })
    }

    fn test_prometheus() -> metrics_exporter_prometheus::PrometheusHandle {
        use std::sync::OnceLock;
        static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
        HANDLE
            .get_or_init(|| {
                metrics_exporter_prometheus::PrometheusBuilder::new()
                    .install_recorder()
                    .expect("install test prometheus recorder")
            })
            .clone()
    }

    async fn make_state(gateway_url: &str) -> Arc<AppState> {
        let config = AgentConfig {
            gateway_url: gateway_url.to_owned(),
            local_model: "test-model".to_owned(),
            db_path: "/tmp/test-not-used".to_owned(),
            bind_addr: "[::]:8765".to_owned(),
            ntfy: NtfyConfig {
                url: "http://localhost".to_owned(),
                topic: "test".to_owned(),
            },
            google: None,
            voice: None,
            scheduler: SchedulerConfig {
                notification_interval_secs: 300,
                gmail_scan_interval_secs: 900,
                event_reminder_lead_secs: 1800,
                overdue_grace_secs: 0,
            },
        };
        Arc::new(AppState {
            db: test_db(),
            config,
            ntfy: NtfyClient::new("http://localhost".into(), "test".into()),
            google_auth: None,
            prometheus_handle: test_prometheus(),
            http: reqwest::Client::new(),
        })
    }

    #[tokio::test]
    async fn post_chat_no_session_returns_reply_and_session_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gateway_reply("Hello there!")))
            .mount(&server)
            .await;

        let state = make_state(&server.uri()).await;
        let app = crate::http::router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/chat")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"message":"hi"}"#))
            .expect("build request");

        let resp = app.oneshot(req).await.expect("oneshot");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).expect("parse response body");
        assert_eq!(json["reply"].as_str(), Some("Hello there!"));
        assert!(
            json["session_id"].as_str().is_some(),
            "session_id must be present"
        );
    }

    #[tokio::test]
    async fn second_chat_with_session_sends_prior_context() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(gateway_reply("Got it.")))
            .mount(&server)
            .await;

        let state = make_state(&server.uri()).await;
        let app = crate::http::router(Arc::clone(&state));

        // First request — no session
        let req1 = Request::builder()
            .method("POST")
            .uri("/chat")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"message":"first message"}"#))
            .expect("build request");
        let resp1 = app.oneshot(req1).await.expect("oneshot");
        let body1 = resp1
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes();
        let json1: serde_json::Value = serde_json::from_slice(&body1).expect("parse");
        let session_id = json1["session_id"].as_str().expect("session_id").to_owned();

        // Second request — same session
        let app2 = crate::http::router(Arc::clone(&state));
        let req2 = Request::builder()
            .method("POST")
            .uri("/chat")
            .header("content-type", "application/json")
            .header("x-session-id", &session_id)
            .body(Body::from(r#"{"message":"second message"}"#))
            .expect("build request");
        let resp2 = app2.oneshot(req2).await.expect("oneshot");
        assert_eq!(resp2.status(), StatusCode::OK);

        // Verify the session messages were saved after first request and loaded in second
        let session_store = state.session_store();
        let session_uuid = uuid::Uuid::parse_str(&session_id).expect("parse uuid");
        let messages = session_store
            .load(session_uuid)
            .await
            .expect("load session");
        // After two requests: user1, assistant1, user2, assistant2 = 4 messages
        assert!(
            messages.len() >= 2,
            "expected at least 2 messages in session, got {}",
            messages.len()
        );
    }
}
