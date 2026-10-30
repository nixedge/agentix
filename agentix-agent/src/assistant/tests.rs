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
        .oneshot(builder.body(Body::from(serde_json::to_vec(
            &json!({"device_id":null,"request":request}),
        )?))?)
        .await?;
    let status = response.status();
    let body = response.into_body().collect().await?.to_bytes();
    let body: Value = serde_json::from_slice(&body)?;
    if status == StatusCode::OK {
        assert!(body["device_id"].is_null());
        assert_eq!(body["reminder_receipts"], json!([]));
        Ok((status, body["response"].clone()))
    } else {
        Ok((status, body))
    }
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
    for captured in &captured {
        let body = std::str::from_utf8(&captured.body)?;
        assert!(!body.contains("rename_grocery_item"));
        assert!(!body.contains("current_groceries"));
    }
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
    let current = request(Uuid::new_v4(), "Save a note.");
    for (path, body, expected) in [
        (
            "/v1/assistant/tell",
            serde_json::to_value(&current)?,
            StatusCode::BAD_REQUEST,
        ),
        (
            "/v1/assistant/tell",
            json!({"protocol_version":2,"device_id":null,"request":current}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/v2/assistant/tell",
            json!({"device_id":null,"request":current}),
            StatusCode::NOT_FOUND,
        ),
        (
            "/v1/assistant/capabilities",
            json!({}),
            StatusCode::NOT_FOUND,
        ),
        (
            "/v2/assistant/capabilities",
            json!({}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let input = Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(Body::from(body.to_string()))?;
        assert_eq!(app.clone().oneshot(input).await?.status(), expected);
    }
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

#[tokio::test]
async fn grocery_gateway_tools_use_remote_ids_and_selected_action_schemas() -> TestResult {
    for action in [
        json!({"action":"show_groceries"}),
        json!({"action":"add_grocery_item","text":"milk"}),
        json!({"action":"rename_grocery_item","item_id":"remote-item-1","text":"shallots"}),
        json!({"action":"set_grocery_item_completed","item_id":"remote-item-1","completed":true}),
        json!({"action":"delete_grocery_item","item_id":"remote-item-1"}),
    ] {
        let gateway = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"response_format":{"json_schema":{"name":"personal_assistant_operation"}}}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(completion(json!({"operation":action["action"]}))),
            )
            .expect(1)
            .mount(&gateway)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"response_format":{"json_schema":{"name":"personal_assistant_action"}}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion(action.clone())))
            .expect(1)
            .mount(&gateway)
            .await;
        let context = InferenceContext {
            request: request(Uuid::new_v4(), "synthetic grocery instruction"),
            accepted_at: chrono::Utc::now(),
            messages: vec![],
            records: vec![],
            context_incomplete: false,
            groceries_configured: true,
            groceries: Some(agentix_assistant_core::GroceryList {
                team_id: "household-1".into(),
                id: "list-1".into(),
                name: "Kitchen".into(),
                items: vec![agentix_assistant_core::GroceryItem {
                    id: "remote-item-1".into(),
                    text: "onions".into(),
                    completed: false,
                    category_id: None,
                    note: None,
                }],
            }),
        };
        assert_eq!(
            LocalGateway::new(&gateway.uri(), "qwen-test")?
                .propose(context)
                .await?,
            serde_json::from_value::<Intent>(action.clone())?
        );
        let requests = gateway
            .received_requests()
            .await
            .ok_or("missing model requests")?;
        let selected: Value = serde_json::from_slice(&requests[0].body)?;
        assert!(selected["messages"][0]["content"]
            .as_str()
            .is_some_and(
                |text| text.contains("remote-item-1") && text.contains("groceries_configured")
            ));
        let filled: Value = serde_json::from_slice(&requests[1].body)?;
        let variants = filled["response_format"]["json_schema"]["schema"]["oneOf"]
            .as_array()
            .ok_or("missing action variants")?;
        assert_eq!(variants.len(), 2);
        assert!(variants
            .iter()
            .all(
                |variant| variant["properties"]["action"]["const"] == "clarify"
                    || variant["properties"]["action"]["const"] == action["action"]
            ));
    }
    Ok(())
}

#[test]
fn grocery_config_is_opt_in_and_runtime_credentials_are_redacted() -> TestResult {
    let disabled: crate::config::AssistantConfig =
        toml::from_str("db_path='personal'\nindex_path='index'\n")?;
    assert!(disabled.ourgroceries.is_none());
    let configured: crate::config::AssistantConfig = toml::from_str(
        "db_path='personal'\nindex_path='index'\n[ourgroceries]\nlist_id='list-1'\n",
    )?;
    let settings = configured.ourgroceries.ok_or("missing grocery settings")?;
    assert_eq!(settings.email_env, "OURGROCERIES_EMAIL");
    assert_eq!(settings.password_env, "OURGROCERIES_PASSWORD");
    let missing =
        super::resolve_grocery_credentials(&settings, |_| Err(std::env::VarError::NotPresent));
    assert!(missing.is_err());
    let invalid = super::resolve_grocery_credentials(&settings, |_| {
        Err(std::env::VarError::NotUnicode(std::ffi::OsString::from(
            "synthetic-secret",
        )))
    });
    assert!(invalid.is_err());
    assert!(!format!("{invalid:?}").contains("synthetic-secret"));
    Ok(())
}

#[tokio::test]
#[ignore = "explicit synthetic grocery selection against the installed local Qwen model; no OurGroceries account access"]
async fn installed_qwen_grocery_selection() -> TestResult {
    let gateway = LocalGateway::new(
        &std::env::var("AGENTIX_ACCEPTANCE_GATEWAY")?,
        &std::env::var("AGENTIX_ACCEPTANCE_MODEL")?,
    )?;
    let cases = [
        (
            "Add milk to OurGroceries.",
            Intent::AddGroceryItem {
                text: "milk".into(),
            },
        ),
        ("What's on our grocery list?", Intent::ShowGroceries),
        (
            "Change the onions on OurGroceries to shallots.",
            Intent::RenameGroceryItem {
                item_id: "remote-item-1".into(),
                text: "shallots".into(),
            },
        ),
        (
            "I bought the onions on OurGroceries.",
            Intent::SetGroceryItemCompleted {
                item_id: "remote-item-1".into(),
                completed: true,
            },
        ),
        (
            "We need onions again; reopen that item on OurGroceries.",
            Intent::SetGroceryItemCompleted {
                item_id: "remote-item-1".into(),
                completed: false,
            },
        ),
        (
            "Delete the onions item from OurGroceries.",
            Intent::DeleteGroceryItem {
                item_id: "remote-item-1".into(),
            },
        ),
    ];
    for (text, expected) in cases {
        let request = request(Uuid::new_v4(), text);
        let accepted_at = chrono::Utc::now();
        let context = InferenceContext {
            messages: vec![agentix_assistant_core::Message {
                conversation_id: request.conversation_id,
                sequence: 1,
                request_id: request.request_id,
                role: agentix_assistant_core::MessageRole::User,
                text: text.into(),
                created_at: accepted_at,
            }],
            request,
            accepted_at,
            records: vec![],
            context_incomplete: false,
            groceries_configured: true,
            groceries: Some(agentix_assistant_core::GroceryList {
                team_id: "household-1".into(),
                id: "list-1".into(),
                name: "Kitchen".into(),
                items: vec![agentix_assistant_core::GroceryItem {
                    id: "remote-item-1".into(),
                    text: "onions".into(),
                    completed: matches!(
                        expected,
                        Intent::SetGroceryItemCompleted {
                            completed: false,
                            ..
                        }
                    ),
                    category_id: None,
                    note: None,
                }],
            }),
        };
        let started = std::time::Instant::now();
        let actual = gateway.propose(context).await?;
        println!(
            "{}",
            json!({"synthetic_instruction":text,"decision":actual,"elapsed_ms":started.elapsed().as_millis()})
        );
        assert_eq!(actual, expected);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "explicit synthetic reminder selection against installed local Qwen; no live schedules or notifications"]
async fn installed_qwen_reminder_selection() -> TestResult {
    use agentix_assistant_core::{
        Reminder, ReminderContext, ReminderInference, ReminderIntent, ReminderTime,
    };
    let gateway = LocalGateway::new(
        &std::env::var("AGENTIX_ACCEPTANCE_GATEWAY")?,
        &std::env::var("AGENTIX_ACCEPTANCE_MODEL")?,
    )?;
    let id = Uuid::parse_str("11111111-2222-4333-8444-555555555555")?;
    let accepted_at = "2026-10-30T14:00:00Z".parse::<chrono::DateTime<chrono::Utc>>()?;
    for (text, operation) in [
        (
            "Remind me in twenty minutes to take the laundry out.",
            "create",
        ),
        ("What reminders do I have?", "list"),
        ("Cancel the laundry reminder.", "cancel"),
        (
            "Move the laundry reminder to tomorrow at 3 PM.",
            "reschedule",
        ),
        ("Remind me to call Sam at eight.", "clarify"),
        ("Remind me every day at noon to take vitamins.", "clarify"),
        (
            "Save a task to replace the air filter tomorrow at 3 PM.",
            "other",
        ),
        ("Add onions to our grocery list.", "other"),
        ("Remember that the spare key is in the garage.", "other"),
    ] {
        let request = request(Uuid::new_v4(), text);
        let context = InferenceContext {
            messages: vec![agentix_assistant_core::Message {
                conversation_id: request.conversation_id,
                sequence: 1,
                request_id: request.request_id,
                role: agentix_assistant_core::MessageRole::User,
                text: text.into(),
                created_at: accepted_at,
            }],
            request,
            accepted_at,
            records: vec![],
            context_incomplete: false,
            groceries_configured: false,
            groceries: None,
        };
        let result = gateway
            .propose_reminder(ReminderContext {
                context,
                incomplete: false,
                reminders: vec![Reminder {
                    id,
                    revision: 3,
                    text: "take the laundry out".into(),
                    due_at: accepted_at + chrono::Duration::hours(1),
                    timezone: "UTC".into(),
                    cancelled: false,
                }],
            })
            .await?;
        println!("{}", serde_json::json!({"case":text,"decision":result}));
        assert_eq!(serde_json::to_value(&result)?["operation"], operation);
        match result {
            ReminderIntent::Create { time, .. } => {
                assert_eq!(time, ReminderTime::After { seconds: 1200 })
            }
            ReminderIntent::Cancel {
                id: target,
                expected_revision,
            } => {
                assert_eq!(target, id);
                assert_eq!(expected_revision, 3);
            }
            ReminderIntent::Reschedule {
                id: target,
                expected_revision,
                time,
            } => {
                assert_eq!(target, id);
                assert_eq!(expected_revision, 3);
                assert_eq!(
                    time,
                    ReminderTime::Local {
                        date: "2026-10-31".parse()?,
                        time: "15:00:00".parse()?
                    }
                );
            }
            ReminderIntent::Clarify { question } if text.contains("every day") => {
                assert!(question.contains("one-off"))
            }
            _ => {}
        }
    }
    Ok(())
}

#[tokio::test]
async fn reminder_gateway_rejects_invented_revision_and_time_kind_disagreement() -> TestResult {
    use agentix_assistant_core::{Reminder, ReminderContext, ReminderInference};
    let id = Uuid::new_v4();
    for action in [
        json!({"operation":"reschedule","id":id,"expected_revision":4,"time":{"kind":"local","date":"2026-10-31","time":"15:00:00"}}),
        json!({"operation":"reschedule","id":id,"expected_revision":3,"time":{"kind":"after","seconds":86400}}),
    ] {
        let server = MockServer::start().await;
        for (name, output) in [
            ("reminder_operation", json!({"operation":"reschedule"})),
            ("reminder_time_kind", json!({"kind":"local"})),
            ("reminder_action", action),
        ] {
            Mock::given(method("POST"))
                .and(body_partial_json(
                    json!({"response_format":{"json_schema":{"name":name}}}),
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(completion(output)))
                .expect(1)
                .mount(&server)
                .await;
        }
        let gateway = LocalGateway::new(&server.uri(), "synthetic")?;
        let accepted_at = "2026-10-30T14:00:00Z".parse()?;
        let result = gateway
            .propose_reminder(ReminderContext {
                context: InferenceContext {
                    request: request(
                        Uuid::new_v4(),
                        "Move the laundry reminder to tomorrow at 3 PM.",
                    ),
                    accepted_at,
                    messages: vec![],
                    records: vec![],
                    context_incomplete: false,
                    groceries_configured: false,
                    groceries: None,
                },
                reminders: vec![Reminder {
                    id,
                    revision: 3,
                    text: "laundry".into(),
                    due_at: accepted_at,
                    timezone: "UTC".into(),
                    cancelled: false,
                }],
                incomplete: false,
            })
            .await;
        assert!(matches!(result, Err(Error::Inference)));
        let requests = server
            .received_requests()
            .await
            .ok_or("missing mock requests")?;
        let action: Value = serde_json::from_slice(&requests[2].body)?;
        let choices = action["response_format"]["json_schema"]["schema"]["oneOf"]
            .as_array()
            .ok_or("missing choices")?;
        let fields = &choices
            .iter()
            .find(|choice| choice["properties"]["operation"]["const"] == "reschedule")
            .ok_or("missing edit schema")?["properties"];
        assert_eq!(fields["id"]["const"], id.to_string());
        assert_eq!(fields["expected_revision"]["const"], 3);
        assert_eq!(fields["time"]["properties"]["kind"]["const"], "local");
    }
    Ok(())
}

#[tokio::test]
async fn device_http_auth_enrollment_reminder_replay_and_sync_use_real_store() -> TestResult {
    use agentix_assistant_core::{
        DeviceTurnRequest, DeviceTurnResponse, ReminderDeliveryState, ReminderReport, Reminders,
    };
    let directory = tempfile::tempdir()?;
    let store = Arc::new(SledRepository::open(directory.path()).await?);
    let gateway = MockServer::start().await;
    for (name, output) in [
        ("reminder_operation", json!({"operation":"create"})),
        ("reminder_time_kind", json!({"kind":"after"})),
        (
            "reminder_action",
            json!({"operation":"create","text":"laundry","time":{"kind":"after","seconds":1200}}),
        ),
    ] {
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"response_format":{"json_schema":{"name":name}}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(completion(output)))
            .expect(1)
            .mount(&gateway)
            .await;
    }
    let model = Arc::new(LocalGateway::new(&gateway.uri(), "qwen-test")?);
    let service = Arc::new(
        Assistant::new(store.clone(), store.clone(), model.clone())
            .with_reminders(store.clone(), model),
    );
    let app = routes(service, Some(TOKEN.into()))?;
    async fn call(
        app: &Router,
        path: &str,
        body: Value,
        auth: bool,
    ) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if auth {
            builder = builder.header("authorization", format!("Bearer {TOKEN}"));
        }
        let reply = app
            .clone()
            .oneshot(builder.body(Body::from(body.to_string()))?)
            .await?;
        let status = reply.status();
        Ok((
            status,
            serde_json::from_slice(&reply.into_body().collect().await?.to_bytes())?,
        ))
    }
    let device = Uuid::new_v4();
    assert_eq!(
        call(
            &app,
            "/v1/assistant/reminders/enroll",
            json!({"device_id":device}),
            false
        )
        .await?
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, enrolled) = call(
        &app,
        "/v1/assistant/reminders/enroll",
        json!({"device_id":device}),
        true,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        call(
            &app,
            "/v1/assistant/reminders/enroll",
            json!({"device_id":Uuid::new_v4()}),
            true
        )
        .await?
        .0,
        StatusCode::CONFLICT
    );
    let req = DeviceTurnRequest {
        device_id: device,
        request: request(
            Uuid::new_v4(),
            "Remind me in twenty minutes to take laundry out.",
        ),
    };
    let (status, body) = call(
        &app,
        "/v1/assistant/tell",
        serde_json::to_value(&req)?,
        true,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let response: DeviceTurnResponse = serde_json::from_value(body.clone())?;
    response.validate(&req)?;
    assert_eq!(response.reminder_receipts.len(), 1);
    assert!(response.response.outcomes.is_empty());
    assert_eq!(
        call(
            &app,
            "/v1/assistant/tell",
            serde_json::to_value(&req)?,
            true
        )
        .await?
        .1,
        body
    );
    assert_eq!(
        post(&app, &req.request, Some(TOKEN)).await?.0,
        StatusCode::CONFLICT
    );
    let item = &response.reminder_receipts[0];
    let report = ReminderReport {
        report_id: Uuid::new_v4(),
        reminder_id: item.id,
        revision: item.revision,
        sequence: 1,
        state: ReminderDeliveryState::Armed,
    };
    let (status, _) = call(
        &app,
        "/v1/assistant/reminders/report",
        json!({"server_id":enrolled["server_id"],"device_id":device,"reports":[report]}),
        true,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(store.snapshot(device).await?.observations.len(), 1);
    assert!(store.records().await?.is_empty());
    Ok(())
}
