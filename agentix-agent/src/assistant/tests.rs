use super::{gateway::LocalGateway, routes};
use agentix_assistant::Assistant;
use agentix_assistant_core::{
    Conversations, Error, Inference, InferenceContext, Intent, RecordBody, Repository, TellRequest,
    TellResponse, TellStatus,
};
use agentix_assistant_sled::SledRepository;
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    matchers::{body_partial_json, method, path},
    Mock, MockServer, ResponseTemplate,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const TOKEN: &str = "synthetic-test-owner-token-0123456789";
fn request(conversation: Uuid, text: &str) -> TellRequest {
    TellRequest {
        request_id: Uuid::new_v4(),
        conversation_id: conversation,
        text: text.into(),
        timezone: "America/New_York".into(),
    }
}
fn completion(intent: Value) -> Value {
    json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":intent.to_string()}}]})
}
async fn post(
    app: &Router,
    request: &TellRequest,
    token: Option<&str>,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/assistant/tell")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(serde_json::to_vec(request)?))?)
        .await?;
    let status = response.status();
    let body = response.into_body().collect().await?.to_bytes();
    Ok((status, serde_json::from_slice(&body)?))
}

#[tokio::test]
async fn authenticated_http_grocery_save_retry_conflict_and_actual_history() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(
            json!({"response_format":{"json_schema":{"name":"personal_assistant_operation"}}}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(completion(json!({"operation":"add_list_item"}))),
        )
        .expect(1)
        .mount(&gateway)
        .await;
    Mock::given(method("POST")).and(path("/v1/chat/completions"))
        .and(body_partial_json(json!({"response_format":{"json_schema":{"name":"personal_assistant_action"}}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion(json!({"action":"add_list_item","list_name":"groceries","list_id":null,"text":"pineapple"}))))
        .expect(1).mount(&gateway).await;
    let shared = Arc::new(store.clone());
    let service = Arc::new(Assistant::new(
        shared.clone(),
        shared,
        Arc::new(LocalGateway::new(&gateway.uri(), "qwen-test")?),
    ));
    let app = routes(service, Some(TOKEN.into()))?;
    let original = request(Uuid::new_v4(), "Add pineapple to my grocery list.");
    assert_eq!(
        post(&app, &original, None).await?.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&app, &original, Some("wrong-owner-token")).await?.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(store.records().await?.is_empty());
    let (status, body) = post(&app, &original, Some(TOKEN)).await?;
    assert_eq!(status, StatusCode::OK);
    let saved: TellResponse = serde_json::from_value(body.clone())?;
    assert_eq!(saved.status, TellStatus::Saved);
    assert_eq!(saved.outcomes.len(), 2);
    assert_eq!(post(&app, &original, Some(TOKEN)).await?.1, body);
    let mut changed = original.clone();
    changed.text = "changed envelope".into();
    assert_eq!(
        post(&app, &changed, Some(TOKEN)).await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(store.records().await?.len(), 2);
    assert_eq!(
        store
            .recent_messages(original.conversation_id, 100)
            .await?
            .len(),
        2
    );
    let captured = gateway
        .received_requests()
        .await
        .ok_or("missing gateway requests")?;
    assert_eq!(captured.len(), 2);
    let sent: Value = serde_json::from_slice(&captured[1].body)?;
    assert_eq!(sent["response_format"]["type"], "json_schema");
    assert_eq!(sent["temperature"], 0.0);
    assert_eq!(sent["max_tokens"], 512);
    assert!(sent["messages"][0]["content"]
        .as_str()
        .ok_or("missing context")?
        .contains("America/New_York"));
    assert_eq!(sent["messages"][1]["content"], original.text);
    Ok(())
}

struct MustNotInfer;
#[async_trait]
impl Inference for MustNotInfer {
    async fn propose(&self, _: InferenceContext) -> agentix_assistant_core::Result<Intent> {
        Err(Error::Inference)
    }
}

#[tokio::test]
async fn invalid_auth_body_envelopes_and_unsafe_configuration_never_write() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let shared = Arc::new(store.clone());
    let service = Arc::new(Assistant::new(
        shared.clone(),
        shared,
        Arc::new(MustNotInfer),
    ));
    assert!(routes(service.clone(), Some("short".into())).is_err());
    let app = routes(service, Some(TOKEN.into()))?;
    let mut invalid = request(Uuid::new_v4(), " ");
    assert_eq!(
        post(&app, &invalid, Some(TOKEN)).await?.0,
        StatusCode::BAD_REQUEST
    );
    invalid.text = "valid".into();
    invalid.timezone = "invalid-timezone".into();
    assert_eq!(
        post(&app, &invalid, Some(TOKEN)).await?.0,
        StatusCode::BAD_REQUEST
    );
    let oversized = Request::builder()
        .method("POST")
        .uri("/v1/assistant/tell")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::from("x".repeat(20_000)))?;
    assert_eq!(
        app.oneshot(oversized).await?.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(store.records().await?.is_empty());
    assert!(LocalGateway::new("http://localhost", "gpt-4o").is_err());
    assert!(LocalGateway::new("http://owner:secret@localhost", "qwen-test").is_err());
    Ok(())
}

#[tokio::test]
async fn malformed_or_truncated_model_output_cannot_claim_a_save() -> TestResult {
    for (content, finish_reason) in [
        ("I saved it!", "stop"),
        ("{}", "length"),
        ("{\"action\":\"invented_tool\"}", "stop"),
    ] {
        let directory = tempfile::tempdir()?;
        let store = SledRepository::open(directory.path()).await?;
        let gateway = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"choices":[{"finish_reason":finish_reason,"message":{"content":content}}]}),
            ))
            .mount(&gateway)
            .await;
        let shared = Arc::new(store.clone());
        let service = Arc::new(Assistant::new(
            shared.clone(),
            shared,
            Arc::new(LocalGateway::new(&gateway.uri(), "qwen-test")?),
        ));
        let app = routes(service, Some(TOKEN.into()))?;
        let (_, body) = post(
            &app,
            &request(Uuid::new_v4(), "Add pineapple."),
            Some(TOKEN),
        )
        .await?;
        assert_eq!(body["status"], "failed");
        assert_eq!(body["outcomes"], json!([]));
        assert!(store.records().await?.is_empty());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "explicit synthetic acceptance against the owner's already-installed Qwen model; not a hermetic fixture test"]
async fn installed_qwen_acceptance() -> TestResult {
    struct SyntheticTrace(LocalGateway);
    #[async_trait]
    impl Inference for SyntheticTrace {
        async fn propose(
            &self,
            context: InferenceContext,
        ) -> agentix_assistant_core::Result<Intent> {
            let decision = self.0.propose(context).await?;
            println!("{}", json!({"synthetic_decision":decision}));
            Ok(decision)
        }
    }
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let url = std::env::var("AGENTIX_ACCEPTANCE_GATEWAY")?;
    let model = std::env::var("AGENTIX_ACCEPTANCE_MODEL")?;
    let shared = Arc::new(store.clone());
    let service = Arc::new(
        Assistant::new(
            shared.clone(),
            shared,
            Arc::new(SyntheticTrace(LocalGateway::new(&url, &model)?)),
        )
        .with_inference_timeout(std::time::Duration::from_secs(90))?,
    );
    let app = routes(service, Some(TOKEN.into()))?;
    let conversation = Uuid::new_v4();
    let cases = [
        (
            "grocery_add",
            "Add pineapple to my grocery list.",
            TellStatus::Saved,
        ),
        (
            "grocery_read",
            "What is on my grocery list?",
            TellStatus::Answer,
        ),
        (
            "correction",
            "No, make that mango instead of pineapple.",
            TellStatus::Saved,
        ),
        (
            "note",
            "Save a note: the spare key is in the garage.",
            TellStatus::Saved,
        ),
        (
            "task",
            "Add a task to replace the air filter. No due date.",
            TellStatus::Saved,
        ),
        (
            "task_completion",
            "Mark the task to replace the air filter as complete.",
            TellStatus::Saved,
        ),
        (
            "ambiguous_time",
            "Save a task to call Pat next Friday at eight.",
            TellStatus::Clarification,
        ),
        (
            "unsupported_email",
            "Email my grocery list to Alex.",
            TellStatus::Answer,
        ),
        (
            "unsupported_email_address",
            "Alex's address is alex@example.invalid. Send it there.",
            TellStatus::Answer,
        ),
        (
            "unsupported_email_confirm",
            "Yes, send the email now.",
            TellStatus::Answer,
        ),
        (
            "unsupported_email_status",
            "Did you send the email?",
            TellStatus::Answer,
        ),
        (
            "unsupported_calendar",
            "Add a meeting with Pat to my calendar tomorrow at noon.",
            TellStatus::Answer,
        ),
        (
            "unsupported_reminder",
            "Send me a reminder tomorrow at noon to call Pat.",
            TellStatus::Answer,
        ),
    ];
    for (case, text, expected) in cases {
        let before = store.records().await?;
        let request = request(conversation, text);
        let started = std::time::Instant::now();
        let (http_status, body) = post(&app, &request, Some(TOKEN)).await?;
        let response: TellResponse = serde_json::from_value(body.clone())?;
        println!(
            "{}",
            json!({"case":case,"elapsed_ms":started.elapsed().as_millis(),"http_status":http_status.as_u16(),"status":response.status,"reply":response.reply,"saved_outcomes":response.outcomes.len()})
        );
        assert_eq!(response.status, expected, "unexpected outcome for {case}");
        if case.starts_with("unsupported_email") {
            assert_eq!(response.reply, "I can't send email yet.");
        } else if case == "unsupported_calendar" {
            assert_eq!(
                response.reply,
                "I can't access or change your calendar yet."
            );
        } else if case == "unsupported_reminder" {
            assert_eq!(
                response.reply,
                "I can't deliver reminders yet. I can save a task with a due date."
            );
        }
        assert_eq!(
            post(&app, &request, Some(TOKEN)).await?.1,
            body,
            "request replay changed for {case}"
        );
        let records = store.records().await?;
        match case {
            "grocery_add" => assert_eq!(records.len(), 2),
            "grocery_read" => assert!(response.records.iter().any(|record| matches!(&record.body, RecordBody::ListItem { text, .. } if text.to_lowercase().contains("pineapple")))),
            "correction" => {
                assert_eq!(records.len(), 2);
                assert!(response.outcomes.iter().all(|outcome| matches!(outcome.record.body, RecordBody::ListItem { completed: false, .. }) && outcome.record.revision == 2));
                assert!(records.iter().filter(|record| matches!(record.body, RecordBody::List { .. })).all(|list| before.contains(list)));
                assert!(records.iter().any(|record| matches!(&record.body, RecordBody::ListItem { text, .. } if text.to_lowercase().contains("mango"))));
                assert!(!records.iter().any(|record| matches!(&record.body, RecordBody::ListItem { text, .. } if text.to_lowercase().contains("pineapple"))));
            }
            "note" => assert!(records.iter().any(|record| matches!(&record.body, RecordBody::Note { text } if text.to_lowercase().contains("garage")))),
            "task" => assert!(records.iter().any(|record| matches!(&record.body, RecordBody::Task { title, due_at: None, .. } if title.to_lowercase().contains("air filter")))),
            "task_completion" => assert!(records.iter().any(|record| matches!(&record.body, RecordBody::Task { title, due_at: None, completed: true } if title.to_lowercase().contains("air filter")) && record.revision == 2)),
            _ => {
                assert!(response.outcomes.is_empty());
                assert_eq!(records, before);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn unsupported_http_answer_replays_without_effects_or_repeat_inference() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let gateway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(completion(json!({
            "operation":"unsupported_email"
        }))))
        .expect(1)
        .mount(&gateway)
        .await;
    let shared = Arc::new(store.clone());
    let app = routes(
        Arc::new(Assistant::new(
            shared.clone(),
            shared,
            Arc::new(LocalGateway::new(&gateway.uri(), "qwen-test")?),
        )),
        Some(TOKEN.into()),
    )?;
    let input = request(Uuid::new_v4(), "Email my grocery list to Alex.");
    let (status, body) = post(&app, &input, Some(TOKEN)).await?;
    assert_eq!(status, StatusCode::OK);
    let response: TellResponse = serde_json::from_value(body.clone())?;
    assert_eq!(response.status, TellStatus::Answer);
    assert_eq!(response.reply, "I can't send email yet.");
    assert!(response.outcomes.is_empty());
    assert!(response.records.is_empty());
    assert_eq!(post(&app, &input, Some(TOKEN)).await?.1, body);
    assert!(store.records().await?.is_empty());
    assert!(store.pending_projections(20).await?.is_empty());
    assert_eq!(
        store
            .recent_messages(input.conversation_id, 100)
            .await?
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn two_step_selection_and_filling_disagreements_never_write() -> TestResult {
    for (selection, fill) in [
        (json!({"operation":"unknown"}), None),
        (json!({"operation":"save_note","extra":"not allowed"}), None),
        (
            json!({"operation":"save_note"}),
            Some(
                json!({"action":"add_list_item","list_name":"groceries","list_id":null,"text":"pineapple"}),
            ),
        ),
        (
            json!({"operation":"save_note"}),
            Some(json!({"action":"unsupported","capability":"email"})),
        ),
        (
            json!({"operation":"save_note"}),
            Some(json!({"action":"save_note","text":"ok","extra":true})),
        ),
    ] {
        let directory = tempfile::tempdir()?;
        let store = SledRepository::open(directory.path()).await?;
        let gateway = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"response_format":{"json_schema":{"name":"personal_assistant_operation"}}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion(selection)))
            .expect(1)
            .mount(&gateway)
            .await;
        if let Some(fill) = fill {
            Mock::given(method("POST"))
                .and(body_partial_json(
                    json!({"response_format":{"json_schema":{"name":"personal_assistant_action"}}}),
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(completion(fill)))
                .expect(1)
                .mount(&gateway)
                .await;
        }
        let shared = Arc::new(store.clone());
        let app = routes(
            Arc::new(Assistant::new(
                shared.clone(),
                shared,
                Arc::new(LocalGateway::new(&gateway.uri(), "qwen-test")?),
            )),
            Some(TOKEN.into()),
        )?;
        let input = request(Uuid::new_v4(), "Save a note.");
        let (_, body) = post(&app, &input, Some(TOKEN)).await?;
        assert_eq!(body["status"], "failed");
        assert_eq!(body["outcomes"], json!([]));
        assert!(store.records().await?.is_empty());
        let requests = gateway
            .received_requests()
            .await
            .ok_or("missing requests")?;
        if requests.len() == 2 {
            let fill: Value = serde_json::from_slice(&requests[1].body)?;
            let allowed = fill["response_format"]["json_schema"]["schema"]["oneOf"]
                .as_array()
                .ok_or("missing schema")?;
            assert_eq!(allowed.len(), 2);
            assert!(allowed.iter().all(|variant| matches!(
                variant["properties"]["action"]["const"].as_str(),
                Some("save_note" | "clarify")
            )));
        }
    }
    Ok(())
}

#[test]
fn optional_owner_token_resolution_preserves_configured_auth_and_redacts_errors() -> TestResult {
    let mut settings: crate::config::AssistantConfig =
        toml::from_str("db_path = 'personal'\nindex_path = 'index'\n")?;
    assert!(settings.token_env.is_none());
    assert_eq!(
        super::resolve_owner_token(&settings, |name| {
            assert_eq!(name, "AGENTIX_ASSISTANT_TOKEN");
            Err(std::env::VarError::NotPresent)
        })?,
        None
    );
    assert_eq!(
        super::resolve_owner_token(&settings, |_| Ok(TOKEN.into()))?,
        Some(TOKEN.into())
    );
    for value in ["", "short", "                                "] {
        assert!(super::resolve_owner_token(&settings, |_| Ok(value.into())).is_err());
    }
    let error = super::resolve_owner_token(&settings, |_| {
        Err(std::env::VarError::NotUnicode(
            "sensitive-test-value".into(),
        ))
    })
    .err()
    .ok_or("invalid token accepted")?;
    assert!(!format!("{error:#}").contains("sensitive-test-value"));
    settings.token_env = Some("CUSTOM_OWNER_TOKEN".into());
    assert!(
        super::resolve_owner_token(&settings, |_| Err(std::env::VarError::NotPresent)).is_err()
    );
    assert_eq!(
        super::resolve_owner_token(&settings, |name| {
            assert_eq!(name, "CUSTOM_OWNER_TOKEN");
            Ok(TOKEN.into())
        })?,
        Some(TOKEN.into())
    );
    settings.token_env = Some(String::new());
    assert!(super::resolve_owner_token(&settings, |_| Ok(TOKEN.into())).is_err());
    Ok(())
}

#[tokio::test]
async fn tokenless_http_save_and_replay_use_actual_storage() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let gateway = MockServer::start().await;
    for (name, decision) in [
        (
            "personal_assistant_operation",
            json!({"operation":"save_note"}),
        ),
        (
            "personal_assistant_action",
            json!({"action":"save_note","text":"synthetic tokenless note"}),
        ),
    ] {
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"response_format":{"json_schema":{"name":name}}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion(decision)))
            .expect(1)
            .mount(&gateway)
            .await;
    }
    let shared = Arc::new(store.clone());
    let app = routes(
        Arc::new(Assistant::new(
            shared.clone(),
            shared,
            Arc::new(LocalGateway::new(&gateway.uri(), "qwen-test")?),
        )),
        None,
    )?;
    let input = request(Uuid::new_v4(), "Save a note.");
    let (status, body) = post(&app, &input, None).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "saved");
    assert_eq!(post(&app, &input, None).await?.1, body);
    let records = store.records().await?;
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].body,
        RecordBody::Note {
            text: "synthetic tokenless note".into()
        }
    );
    assert_eq!(
        store
            .recent_messages(input.conversation_id, 100)
            .await?
            .len(),
        2
    );
    Ok(())
}
