#[path = "../src/config.rs"]
#[allow(dead_code)] // The route fixture does not bind Config's listener or load host environment.
mod config;
#[path = "../src/gateway/mod.rs"]
mod gateway;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tower::ServiceExt;

fn config(socket: std::path::PathBuf, provider: String) -> config::Config {
    config::Config {
        gateway_host: "127.0.0.1".into(),
        gateway_port: 0,
        ollama_base_url: provider.clone(),
        llama_socket: socket,
        whisper_socket: "/nonexistent/whisper.sock".into(),
        piper_bin: "/nonexistent/piper".into(),
        piper_voices_dir: "/nonexistent/voices".into(),
        allow_cloud_escalation: false,
        agentix_api_key: None,
        anthropic_api_key: Some("fixture-only".into()),
        openai_api_key: Some("fixture-only".into()),
        openrouter_api_key: Some("fixture-only".into()),
        anthropic_base_url: Some(provider.clone()),
        openai_base_url: Some(provider),
    }
}

async fn call(
    app: &Router,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

#[tokio::test]
async fn voice_routes_remain_available_with_coding_gateway(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    std::fs::write(temp.path().join("fixture.onnx"), [])?;
    let mut cfg = config(
        "/nonexistent/llama.sock".into(),
        "http://127.0.0.1:1".into(),
    );
    cfg.piper_voices_dir = temp.path().to_owned();
    let app = gateway::router(Arc::new(agentix_router::Router::new()), cfg)?;
    let response = app
        .clone()
        .oneshot(Request::get("/v1/audio/voices").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), 1024).await?)?;
    assert_eq!(body, json!({"voices":["fixture"]}));
    let (status, body) = call(
        &app,
        "/v1/audio/speech",
        json!({"input":"Hello","model":"missing"}),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]
        .as_str()
        .is_some_and(|error| error.starts_with("voice model not found:")));
    let (status, _) = call(&app, "/v1/audio/voices", json!({"model":"invalid"})).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn local_aliases_win_and_every_legacy_cloud_route_has_zero_provider_traffic(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("llama.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let worker = Router::new()
        .route(
            "/v1/models",
            get(|| async {
                Json(json!({"data":[{"id":"org/gpt-local"},{"id":"org/claude-local"}]}))
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|Json(value): Json<Value>| async move { Json(value) }),
        );
    let worker_task = tokio::spawn(async move { axum::serve(listener, worker).await });
    let counter = Arc::new(AtomicUsize::new(0));
    let provider_counter = Arc::clone(&counter);
    let provider = Router::new().fallback(move || {
        let counter = Arc::clone(&provider_counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Json(json!({"choices":[{"message":{"content":"cloud answer"}}]}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let provider_url = format!("http://{}", listener.local_addr()?);
    let provider_task = tokio::spawn(async move { axum::serve(listener, provider).await });
    let mut cfg = config(socket, provider_url);
    let app = gateway::router(Arc::new(agentix_router::Router::new()), cfg.clone())?;
    for model in [
        "gpt-local",
        "org/gpt-local",
        "claude-local",
        "local/org/gpt-local",
    ] {
        let (status, value) = call(
            &app,
            "/v1/chat/completions",
            json!({"model":model,"messages":[{"role":"user","content":"hi"}]}),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert!(value["model"]
            .as_str()
            .is_some_and(|v| v.starts_with("org/")));
    }
    for path in [
        "/v1/chat/completions",
        "/v1/responses",
        "/v1/messages",
        "/v1/embeddings",
        "/api/embed",
    ] {
        for model in ["gpt-4o", "claude-test", "openai/remote"] {
            let (status, value) = call(
                &app,
                path,
                json!({"model":model,"messages":[{"role":"user","content":"hi"}]}),
            )
            .await?;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path} {model}");
            assert_eq!(value["error"]["code"], "structured_escalation_required");
        }
    }
    let request = json!({"schema_version":1,"knowledge":"Local evidence","question":"Specific question","local_insufficiency":"No local source has the answer","target":{"provider":"open_ai","model":"gpt-test"},"context":{"session_id":"00000000-0000-4000-8000-000000000001","turn_id":"00000000-0000-4000-8000-000000000002","invocation_id":"a"}});
    assert_eq!(
        call(&app, "/v1/agentix/escalations", request.clone())
            .await?
            .0,
        StatusCode::FORBIDDEN
    );
    cfg.allow_cloud_escalation = true;
    let enabled = gateway::router(Arc::new(agentix_router::Router::new()), cfg)?;
    let mut malformed = request.clone();
    malformed["question"] = json!("");
    assert_eq!(
        call(&enabled, "/v1/agentix/escalations", malformed)
            .await?
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    let (status, value) = call(&enabled, "/v1/agentix/escalations", request).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["answer"], "cloud answer");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    worker_task.abort();
    provider_task.abort();
    Ok(())
}
