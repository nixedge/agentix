// Fixture assertions deliberately panic so protocol failures are visible.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex as StdMutex,
};
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, Request, Respond, ResponseTemplate,
};

#[derive(Clone)]
struct LogCapture(Arc<StdMutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn diagnostics<F: std::future::Future>(future: F) -> (F::Output, String) {
    use tracing::instrument::WithSubscriber;
    let bytes = Arc::new(StdMutex::new(Vec::new()));
    let capture = LogCapture(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || capture.clone())
        .finish();
    let result = future.with_subscriber(subscriber).await;
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    (result, text)
}

#[tokio::test]
async fn failures_identify_stage_without_logging_remote_data() {
    for (stage, reason, sign_in_status, page, rpc) in [
        ("sign_in", "status=401", 401, "", json!({})),
        (
            "household_page",
            "invalid_or_missing_team_id",
            200,
            "secret-page-content",
            json!({}),
        ),
        (
            "getList",
            "invalid_list_fields",
            200,
            "g_teamId = \"private-team-id\";",
            json!({"list":{"items":"secret-list-content"}}),
        ),
        (
            "getList",
            "remote_error_or_invalid_envelope",
            200,
            "g_teamId = \"private-team-id\";",
            json!({"error":"secret-remote-error"}),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sign-in"))
            .respond_with(
                ResponseTemplate::new(sign_in_status)
                    .insert_header("Set-Cookie", "ourgroceries-auth=private-session-cookie"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/your-lists/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(page))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/your-lists/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rpc))
            .mount(&server)
            .await;
        let client = Client::at_origin(
            &server.uri(),
            "private-email@example.org".into(),
            "private-password".into(),
            Some("private-list-id".into()),
        )
        .unwrap();
        let (result, log) = diagnostics(client.snapshot()).await;
        assert!(result.is_err());
        assert!(log.contains(stage), "{log}");
        assert!(log.contains(reason), "{log}");
        for secret in [
            "private-email",
            "private-password",
            "private-session-cookie",
            "private-team-id",
            "private-list-id",
            "secret-page-content",
            "secret-list-content",
            "secret-remote-error",
        ] {
            assert!(!log.contains(secret), "{log}");
        }
        assert!(server.received_requests().await.unwrap().len() <= 3);
    }
}

#[derive(Clone)]
struct Api {
    state: Arc<StdMutex<Value>>,
    writes: Arc<AtomicUsize>,
    lose_reply: Arc<AtomicBool>,
    ignore_write: Arc<AtomicBool>,
}

impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["teamId"], "household-1");
        match body["command"].as_str().unwrap() {
            "getOverview" => {
                return ResponseTemplate::new(200)
                    .set_body_json(json!({"shoppingLists":[{"id":"list-1","name":"Kitchen"}]}))
            }
            "getList" => {
                assert_eq!(body["listId"], "list-1");
                return ResponseTemplate::new(200)
                    .set_body_json(json!({"list":self.state.lock().unwrap().clone()}));
            }
            _ => {}
        }
        assert_eq!(body["listId"], "list-1");
        self.writes.fetch_add(1, Ordering::SeqCst);
        if !self.ignore_write.load(Ordering::SeqCst) {
            let mut state = self.state.lock().unwrap();
            let items = state["items"].as_array_mut().unwrap();
            match body["command"].as_str().unwrap() {
                "insertItem" => {
                    assert_eq!(body["categoryId"], "uncategorized");
                    items.push(
                        json!({"id":"new-2","name":body["value"],"value":body["value"],"categoryId":"uncategorized"}),
                    );
                }
                "changeItemValue" => {
                    assert_eq!(body["categoryId"], "produce");
                    assert!(body.get("note").is_none());
                    items[0]["name"] = body["newValue"].clone();
                    items[0]["value"] = body["newValue"].clone();
                }
                "setItemCrossedOff" => {
                    assert!(body["crossedOff"].is_boolean());
                    items[0]["crossedOff"] = body["crossedOff"].clone();
                    items[0]["crossedOffAt"] = if body["crossedOff"] == true {
                        json!(1234)
                    } else {
                        Value::Null
                    };
                }
                "deleteItem" => {
                    items.remove(0);
                }
                other => panic!("unexpected command: {other}"),
            }
        }
        ResponseTemplate::new(if self.lose_reply.load(Ordering::SeqCst) {
            500
        } else {
            200
        })
        .set_body_json(json!({}))
    }
}

async fn fixture() -> (MockServer, Client, Api) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sign-in"))
        .respond_with(|request: &Request| {
            let form = std::str::from_utf8(&request.body).unwrap();
            assert!(form.contains("emailAddress=fixture%40example.org"));
            assert!(form.contains("password=fixture-password"));
            assert!(form.contains("action=sign-in"));
            ResponseTemplate::new(302)
                .insert_header(
                    "Set-Cookie",
                    "ourgroceries-auth=fixture-cookie; Secure; HttpOnly",
                )
                .insert_header("Location", "/your-lists/")
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/your-lists/"))
        .and(header("cookie", "ourgroceries-auth=fixture-cookie"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<script>var g_teamId = \"household-1\";</script>"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let api = Api {
        state: Arc::new(StdMutex::new(
            json!({"id":"list-1","name":"Kitchen","items":[
                {"id":"item-1","name":"onions","value":"onions","categoryId":"produce","note":"red onions","crossedOff":false}
            ]}),
        )),
        writes: Arc::new(AtomicUsize::new(0)),
        lose_reply: Arc::new(AtomicBool::new(false)),
        ignore_write: Arc::new(AtomicBool::new(false)),
    };
    Mock::given(method("POST"))
        .and(path("/your-lists/"))
        .and(header("cookie", "ourgroceries-auth=fixture-cookie"))
        .respond_with(api.clone())
        .mount(&server)
        .await;
    let client = Client::at_origin(
        &server.uri(),
        "fixture@example.org".into(),
        "fixture-password".into(),
        Some("list-1".into()),
    )
    .unwrap();
    (server, client, api)
}

fn plan(mutation: GroceryMutation) -> GroceryPlan {
    GroceryPlan {
        team_id: "household-1".into(),
        list_id: "list-1".into(),
        list_name: "Kitchen".into(),
        mutation,
    }
}

#[tokio::test]
async fn discovery_and_reads_use_current_remote_authority() {
    let (_server, client, api) = fixture().await;
    assert_eq!(client.lists().await.unwrap()[0].name, "Kitchen");
    let before = client.snapshot().await.unwrap();
    assert!(!before.items[0].completed);
    api.state.lock().unwrap()["items"][0]["crossedOffAt"] = json!(9876);
    assert!(client.snapshot().await.unwrap().items[0].completed);
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn item_text_accepts_both_wire_fields_and_legacy_single_fields() {
    let (_server, client, api) = fixture().await;
    assert_eq!(client.snapshot().await.unwrap().items[0].text, "onions");
    api.state.lock().unwrap()["items"][0]["name"] = json!("display name");
    assert_eq!(client.snapshot().await.unwrap().items[0].text, "onions");
    api.state.lock().unwrap()["items"][0]
        .as_object_mut()
        .unwrap()
        .remove("name");
    assert_eq!(client.snapshot().await.unwrap().items[0].text, "onions");
    {
        let mut state = api.state.lock().unwrap();
        state["items"][0].as_object_mut().unwrap().remove("value");
        state["items"][0]["name"] = json!("legacy name");
    }
    assert_eq!(
        client.snapshot().await.unwrap().items[0].text,
        "legacy name"
    );
    api.state.lock().unwrap()["items"][0]["value"] = Value::Null;
    assert_eq!(
        client.snapshot().await.unwrap().items[0].text,
        "legacy name"
    );
    api.state.lock().unwrap()["items"][0]["name"] = Value::Null;
    let (result, log) = diagnostics(client.snapshot()).await;
    assert!(result.is_err());
    assert!(log.contains("list.items[].text"));
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "explicit local inspect JSON validation; no account access"]
async fn local_inspect_snapshot_passes_protocol_and_core_validation() {
    let path = std::env::var("AGENTIX_OURGROCERIES_INSPECT").unwrap();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
        .await
        .unwrap()
        .unwrap();
    let mut raw: Value = serde_json::from_slice(&bytes).unwrap();
    grocery_id(raw["id"].as_str().unwrap()).unwrap();
    let count = raw["items"].as_array().unwrap().len();
    let (_server, client, api) = fixture().await;
    // Bind the supplied response to the synthetic HTTP fixture's selected list.
    raw["id"] = json!("list-1");
    *api.state.lock().unwrap() = raw;
    let list = client.snapshot().await.unwrap();
    assert_eq!(list.items.len(), count);
    list.validate().unwrap();
    println!(
        "Validated {count} supplied items; {} needed, {} purchased",
        list.items.iter().filter(|item| !item.completed).count(),
        list.items.iter().filter(|item| item.completed).count()
    );
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn inspect_reads_malformed_list_without_decoding_or_writing() {
    let (_server, client, api) = fixture().await;
    api.state.lock().unwrap()["items"][0]["note"] =
        json!({"text":"groceries-visible-for-debugging"});
    let raw = client.inspect().await.unwrap();
    assert_eq!(
        raw["items"][0]["note"]["text"],
        "groceries-visible-for-debugging"
    );
    assert!(raw.get("list").is_none());
    let (result, log) = diagnostics(client.snapshot()).await;
    assert!(result.is_err());
    assert!(log.contains("list.items[].note"), "{log}");
    assert!(log.contains("string_or_null"), "{log}");
    assert!(log.contains("object"), "{log}");
    assert!(!log.contains("groceries-visible-for-debugging"));
    assert!(!serde_json::to_string(&raw)
        .unwrap()
        .contains("fixture-password"));
    assert!(!serde_json::to_string(&raw)
        .unwrap()
        .contains("fixture-cookie"));
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn specific_mutations_preserve_other_fields_and_verify_readback() {
    let (_server, client, api) = fixture().await;
    let original = client.snapshot().await.unwrap().items[0].clone();
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::Rename {
                original: original.clone(),
                text: "shallots".into()
            }))
            .await,
        GroceryOutcome::Confirmed
    );
    let renamed = client.snapshot().await.unwrap().items[0].clone();
    assert_eq!(renamed.note, original.note);
    assert_eq!(renamed.category_id, original.category_id);
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::SetCompleted {
                original: renamed,
                completed: true
            }))
            .await,
        GroceryOutcome::Confirmed
    );
    let done = client.snapshot().await.unwrap().items[0].clone();
    assert!(done.completed);
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::SetCompleted {
                original: done,
                completed: false
            }))
            .await,
        GroceryOutcome::Confirmed
    );
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::Add {
                text: "milk".into()
            }))
            .await,
        GroceryOutcome::Confirmed
    );
    let first = client.snapshot().await.unwrap().items[0].clone();
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::Delete { original: first }))
            .await,
        GroceryOutcome::Confirmed
    );
    assert_eq!(api.writes.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn stale_targets_and_wrong_list_never_send_writes() {
    let (_server, client, api) = fixture().await;
    let original = client.snapshot().await.unwrap().items[0].clone();
    api.state.lock().unwrap()["items"][0]["note"] = json!("household edit");
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::Rename {
                original,
                text: "shallots".into()
            }))
            .await,
        GroceryOutcome::Rejected
    );
    let mut wrong = plan(GroceryMutation::Add {
        text: "milk".into(),
    });
    wrong.list_id = "other".into();
    assert_eq!(client.execute(&wrong).await, GroceryOutcome::Rejected);
    wrong.list_id = "list-1".into();
    wrong.team_id = "other-household".into();
    assert_eq!(client.execute(&wrong).await, GroceryOutcome::Rejected);
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_write_response_is_uncertain_without_resending() {
    let (_server, client, api) = fixture().await;
    api.lose_reply.store(true, Ordering::SeqCst);
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::Add {
                text: "milk".into()
            }))
            .await,
        GroceryOutcome::Uncertain
    );
    assert_eq!(api.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        api.state.lock().unwrap()["items"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn successful_status_without_postcondition_is_not_confirmation() {
    let (_server, client, api) = fixture().await;
    api.ignore_write.store(true, Ordering::SeqCst);
    assert_eq!(
        client
            .execute(&plan(GroceryMutation::Add {
                text: "milk".into()
            }))
            .await,
        GroceryOutcome::Uncertain
    );
    assert_eq!(api.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn login_does_not_follow_redirects_or_accept_missing_cookie() {
    let destination = MockServer::start().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", destination.uri()))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::at_origin(
        &server.uri(),
        "fixture@example.org".into(),
        "fixture-password".into(),
        Some("list-1".into()),
    )
    .unwrap();
    assert!(client.snapshot().await.is_err());
    assert!(destination.received_requests().await.unwrap().is_empty());
}

#[test]
fn team_id_uses_json_strings_and_rejects_unsafe_ids() {
    assert_eq!(
        team_id("g_teamId = \"household-1\";").unwrap(),
        "household-1"
    );
    assert!(team_id("g_teamId = \"with space\";").is_err());
    assert!(team_id("g_teamId = broken;").is_err());
}

#[tokio::test]
async fn wrong_list_and_duplicate_remote_ids_are_rejected() {
    let (_server, client, api) = fixture().await;
    api.state.lock().unwrap()["id"] = json!("another-list");
    assert!(client.snapshot().await.is_err());
    api.state.lock().unwrap()["id"] = json!("list-1");
    api.state.lock().unwrap()["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"item-1","name":"duplicate"}));
    assert!(client.snapshot().await.is_err());
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_login_page_is_rejected_without_a_write() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sign-in"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Set-Cookie", "ourgroceries-auth=fixture-cookie"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/your-lists/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(MAX_RESPONSE + 1)))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::at_origin(
        &server.uri(),
        "fixture@example.org".into(),
        "fixture-password".into(),
        Some("list-1".into()),
    )
    .unwrap();
    assert!(client.snapshot().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
