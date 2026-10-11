use agentix_infer::exchange::{Cancellation, Message, ToolCall, ToolCompletionRequest};

fn request(messages: Vec<Message>) -> ToolCompletionRequest {
    ToolCompletionRequest {
        messages,
        tools: vec![],
        max_tokens: 16,
        temperature: None,
    }
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "read_file".into(),
        arguments: serde_json::Map::new(),
    }
}

#[test]
fn exchange_requires_complete_unique_tool_pairs() -> Result<(), Box<dyn std::error::Error>> {
    let assistant = Message::Assistant {
        content: String::new(),
        tool_calls: vec![call("a"), call("b")],
    };
    let result = |id: &str| Message::Tool {
        tool_call_id: id.into(),
        content: "output".into(),
    };
    request(vec![assistant.clone(), result("b"), result("a")]).validate()?;
    assert!(request(vec![assistant.clone(), result("a")])
        .validate()
        .is_err());
    assert!(request(vec![assistant.clone(), result("a"), result("a")])
        .validate()
        .is_err());
    assert!(request(vec![result("a")]).validate().is_err());
    assert!(request(vec![
        assistant.clone(),
        Message::User {
            content: "continue".into()
        }
    ])
    .validate()
    .is_err());
    assert!(request(vec![
        assistant.clone(),
        result("a"),
        result("b"),
        assistant,
        result("a"),
        result("b")
    ])
    .validate()
    .is_err());
    Ok(())
}

#[test]
fn malformed_arguments_and_unknown_fields_fail_decoding() {
    for arguments in ["null", "[]", "\"{}\""] {
        let json = format!(r#"{{"id":"a","name":"read","arguments":{arguments}}}"#);
        assert!(serde_json::from_str::<ToolCall>(&json).is_err());
    }
    assert!(serde_json::from_str::<Message>(
        r#"{"role":"tool","tool_call_id":"a","content":"x","approved":true}"#
    )
    .is_err());
}

#[tokio::test]
async fn cancellation_is_sticky_for_existing_and_late_waiters(
) -> Result<(), Box<dyn std::error::Error>> {
    let cancel = Cancellation::default();
    let other = cancel.clone();
    let waiter = tokio::spawn(async move { other.cancelled().await });
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(1), waiter).await??;
    tokio::time::timeout(std::time::Duration::from_secs(1), cancel.cancelled()).await?;
    assert!(cancel.is_cancelled());
    Ok(())
}
