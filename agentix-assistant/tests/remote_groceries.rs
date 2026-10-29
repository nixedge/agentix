// Integration fixture invariants deliberately panic on failures.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use agentix_assistant::Assistant;
use agentix_assistant_core::*;
use agentix_assistant_sled::SledRepository;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, Request, Respond, ResponseTemplate,
};
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

struct Script {
    intents: Mutex<VecDeque<Intent>>,
    contexts: Mutex<Vec<InferenceContext>>,
}
impl Script {
    fn new(intents: Vec<Intent>) -> Self {
        Self {
            intents: Mutex::new(intents.into()),
            contexts: Mutex::new(vec![]),
        }
    }
}
#[async_trait]
impl Inference for Script {
    async fn propose(&self, context: InferenceContext) -> Result<Intent> {
        self.contexts.lock().unwrap().push(context);
        self.intents
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(Error::Inference)
    }
}
#[derive(Clone)]
struct Api {
    list: Arc<Mutex<Value>>,
    writes: Arc<AtomicUsize>,
    lose_reply: Arc<AtomicBool>,
}
impl Respond for Api {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        if body["command"] == "getList" {
            return ResponseTemplate::new(200)
                .set_body_json(json!({"list":self.list.lock().unwrap().clone()}));
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut list = self.list.lock().unwrap();
        match body["command"].as_str().unwrap() {
            "insertItem" => list["items"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"remote-new","name":body["value"],"categoryId":"uncategorized"})),
            "changeItemValue" => {
                assert_eq!(body["categoryId"], "produce");
                list["items"][0]["name"] = body["newValue"].clone();
            }
            "setItemCrossedOff" => list["items"][0]["crossedOff"] = body["crossedOff"].clone(),
            "deleteItem" => {
                list["items"].as_array_mut().unwrap().remove(0);
            }
            other => panic!("unexpected RPC {other}"),
        }
        ResponseTemplate::new(if self.lose_reply.load(Ordering::SeqCst) {
            500
        } else {
            200
        })
        .set_body_json(json!({}))
    }
}
async fn remote() -> (MockServer, Arc<agentix_ourgroceries::Client>, Api) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sign-in"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Set-Cookie", "ourgroceries-auth=fixture-cookie; Secure"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/your-lists/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("g_teamId = \"household-1\";"))
        .mount(&server)
        .await;
    let api = Api {
        list: Arc::new(Mutex::new(
            json!({"id":"list-1","name":"Kitchen","items":[{"id":"remote-1","name":"onions","categoryId":"produce","note":"red onions"}]}),
        )),
        writes: Arc::new(AtomicUsize::new(0)),
        lose_reply: Arc::new(AtomicBool::new(false)),
    };
    Mock::given(method("POST"))
        .and(path("/your-lists/"))
        .respond_with(api.clone())
        .mount(&server)
        .await;
    let client =
        Arc::new(agentix_ourgroceries::Client::for_local_test(&server.uri(), "list-1").unwrap());
    (server, client, api)
}
fn request(conversation: Uuid, text: &str) -> TellRequest {
    TellRequest {
        request_id: Uuid::new_v4(),
        conversation_id: conversation,
        text: text.into(),
        timezone: "UTC".into(),
    }
}

#[tokio::test]
async fn actual_http_and_sled_tools_keep_remote_authority_and_local_notes() -> TestResult {
    let (_server, provider, api) = remote().await;
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let script = Arc::new(Script::new(vec![
        Intent::AddListItem {
            list_name: "my grocery list".into(),
            list_id: None,
            text: "milk".into(),
        },
        Intent::ShowGroceries,
        Intent::RenameGroceryItem {
            item_id: "remote-1".into(),
            text: "shallots".into(),
        },
        Intent::SetGroceryItemCompleted {
            item_id: "remote-1".into(),
            completed: true,
        },
        Intent::SetGroceryItemCompleted {
            item_id: "remote-1".into(),
            completed: false,
        },
        Intent::DeleteGroceryItem {
            item_id: "remote-1".into(),
        },
        Intent::SaveNote {
            text: "local note remains local".into(),
        },
    ]));
    let shared = Arc::new(store.clone());
    let assistant =
        Arc::new(Assistant::new(shared.clone(), shared, script.clone()).with_groceries(provider));
    let conversation = Uuid::new_v4();
    let add = request(conversation, "add milk to groceries");
    let saved = assistant.tell(add.clone()).await?;
    assert_eq!(saved.status, TellStatus::Answer);
    assert!(saved.reply.starts_with("OurGroceries:"));
    assert!(saved.outcomes.is_empty());
    assert_eq!(assistant.tell(add).await?, saved);
    assert!(store.records().await?.is_empty());
    let shown = assistant
        .tell(request(conversation, "what is on groceries"))
        .await?;
    assert!(shown.reply.contains("onions") && shown.reply.contains("milk"));
    assistant
        .tell(request(conversation, "change onions to shallots"))
        .await?;
    assistant
        .tell(request(conversation, "bought shallots"))
        .await?;
    assistant
        .tell(request(conversation, "need shallots again"))
        .await?;
    assert_eq!(api.list.lock().unwrap()["items"][0]["note"], "red onions");
    assistant
        .tell(request(conversation, "delete shallots"))
        .await?;
    assert!(store.records().await?.is_empty());
    assert_eq!(api.writes.load(Ordering::SeqCst), 5);
    let note = assistant.tell(request(conversation, "save a note")).await?;
    assert_eq!(note.status, TellStatus::Saved);
    assert_eq!(store.records().await?.len(), 1);
    {
        let contexts = script.contexts.lock().unwrap();
        assert!(contexts.iter().all(|context| context.groceries_configured));
        assert!(contexts
            .iter()
            .filter_map(|context| context.groceries.as_ref())
            .flat_map(|list| &list.items)
            .all(|item| item.note.is_none()));
    }
    drop(assistant);
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn lost_write_response_is_definitive_uncertainty_and_never_blindly_replayed() -> TestResult {
    let (_server, provider, api) = remote().await;
    api.lose_reply.store(true, Ordering::SeqCst);
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let script = Arc::new(Script::new(vec![
        Intent::AddGroceryItem {
            text: "milk".into(),
        },
        Intent::ShowGroceries,
    ]));
    let shared = Arc::new(store.clone());
    let assistant =
        Arc::new(Assistant::new(shared.clone(), shared, script).with_groceries(provider));
    let request = request(Uuid::new_v4(), "add milk");
    let response = assistant.tell(request.clone()).await?;
    assert!(response.reply.contains("did not confirm"));
    assert_eq!(response.status, TellStatus::Answer);
    assert_eq!(assistant.tell(request.clone()).await?, response);
    assert_eq!(api.writes.load(Ordering::SeqCst), 1);
    let mut read = request;
    read.request_id = Uuid::new_v4();
    read.text = "show groceries".into();
    assert!(assistant.tell(read).await?.reply.contains("milk"));
    assert_eq!(api.writes.load(Ordering::SeqCst), 1);
    drop(assistant);
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn restart_after_dispatch_does_not_send_a_remote_change() -> TestResult {
    let (_server, provider, api) = remote().await;
    let directory = tempfile::tempdir()?;
    let request = request(Uuid::new_v4(), "add milk");
    let store = SledRepository::open(directory.path()).await?;
    store.begin(request.clone()).await?;
    store
        .prepare_grocery(
            request.request_id,
            GroceryPlan {
                team_id: "household-1".into(),
                list_id: "list-1".into(),
                list_name: "Kitchen".into(),
                mutation: GroceryMutation::Add {
                    text: "milk".into(),
                },
            },
        )
        .await?;
    store.claim_grocery_dispatch(request.request_id).await?;
    store.close().await?;
    let store = SledRepository::open(directory.path()).await?;
    let shared = Arc::new(store.clone());
    let assistant = Arc::new(
        Assistant::new(shared.clone(), shared, Arc::new(Script::new(vec![])))
            .with_groceries(provider),
    );
    let response = assistant.tell(request.clone()).await?;
    assert!(response.reply.contains("did not confirm"));
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
    assert_eq!(assistant.tell(request).await?, response);
    drop(assistant);
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn ambiguous_duplicate_targets_require_owner_disambiguation() -> TestResult {
    let (_server, provider, api) = remote().await;
    api.list.lock().unwrap()["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"remote-duplicate","name":"onions","categoryId":"produce"}));
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let script = Arc::new(Script::new(vec![Intent::RenameGroceryItem {
        item_id: "remote-1".into(),
        text: "shallots".into(),
    }]));
    let shared = Arc::new(store.clone());
    let assistant =
        Arc::new(Assistant::new(shared.clone(), shared, script).with_groceries(provider));
    let response = assistant
        .tell(request(Uuid::new_v4(), "change onions to shallots"))
        .await?;
    assert_eq!(response.status, TellStatus::Clarification);
    assert!(response.reply.contains("multiple matching"));
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
    drop(assistant);
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn old_local_groceries_are_retained_but_hidden_and_not_edited() -> TestResult {
    let (_server, provider, api) = remote().await;
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let list_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();
    store
        .apply(CommandRequest {
            request_id: Uuid::new_v4(),
            command: Command::Create {
                id: list_id,
                body: RecordBody::List {
                    name: "Our grocery list".into(),
                },
            },
        })
        .await?;
    store
        .apply(CommandRequest {
            request_id: Uuid::new_v4(),
            command: Command::Create {
                id: item_id,
                body: RecordBody::ListItem {
                    list_id,
                    text: "old rice".into(),
                    completed: false,
                },
            },
        })
        .await?;
    let before = store.records().await?;
    let script = Arc::new(Script::new(vec![
        Intent::ShowGroceries,
        Intent::EditListItem {
            id: item_id,
            expected_revision: 1,
            text: "new rice".into(),
        },
    ]));
    let shared = Arc::new(store.clone());
    let assistant =
        Arc::new(Assistant::new(shared.clone(), shared, script.clone()).with_groceries(provider));
    let conversation = Uuid::new_v4();
    assert!(assistant
        .tell(request(conversation, "show groceries"))
        .await?
        .reply
        .contains("onions"));
    assert_eq!(
        assistant
            .tell(request(conversation, "correct the old grocery item"))
            .await?
            .status,
        TellStatus::Clarification
    );
    assert_eq!(store.records().await?, before);
    assert!(script
        .contexts
        .lock()
        .unwrap()
        .iter()
        .all(|context| context.records.is_empty()));
    assert_eq!(api.writes.load(Ordering::SeqCst), 0);
    drop(assistant);
    store.close().await?;
    Ok(())
}

#[tokio::test]
async fn unicode_list_reply_stays_within_wire_bounds_and_clears_the_turn() -> TestResult {
    let (_server, provider, api) = remote().await;
    api.list.lock().unwrap()["items"] = Value::Array(
        (0..25)
            .map(|index| json!({"id":format!("item-{index}"),"name":"🧅".repeat(256)}))
            .collect(),
    );
    let directory = tempfile::tempdir()?;
    let store = SledRepository::open(directory.path()).await?;
    let shared = Arc::new(store.clone());
    let script = Arc::new(Script::new(vec![
        Intent::ShowGroceries,
        Intent::ShowGroceries,
    ]));
    let assistant =
        Arc::new(Assistant::new(shared.clone(), shared, script).with_groceries(provider));
    let conversation = Uuid::new_v4();
    let response = assistant
        .tell(request(conversation, "show groceries"))
        .await?;
    assert_eq!(response.status, TellStatus::Answer);
    assert!(response.reply.len() <= MAX_REPLY_BYTES);
    assert!(response.reply.contains("first twenty"));
    assistant
        .tell(request(conversation, "show groceries again"))
        .await?;
    drop(assistant);
    store.close().await?;
    Ok(())
}
