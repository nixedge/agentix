use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use agentix_harness::client::{ChatBackend, ChatResponse};
use agentix_harness::tool::ToolCall;
use agentix_harness::{AgentLoop, EscalationPolicy, Tool};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

struct Scripted {
    replies: Mutex<VecDeque<ChatResponse>>,
    requests: Mutex<Vec<(Vec<Value>, usize)>>,
}

#[async_trait]
impl ChatBackend for Scripted {
    async fn complete(
        &self,
        _: &str,
        messages: &[Value],
        tools: &[&dyn Tool],
    ) -> Result<ChatResponse> {
        self.requests
            .lock()
            .map_err(|_| anyhow!("poisoned requests"))?
            .push((messages.to_vec(), tools.len()));
        self.replies
            .lock()
            .map_err(|_| anyhow!("poisoned replies"))?
            .pop_front()
            .ok_or_else(|| anyhow!("unexpected model retry"))
    }
}

struct CountTool(Arc<AtomicUsize>);

#[async_trait]
impl Tool for CountTool {
    fn needs_approval(&self) -> bool {
        false
    }
    fn name(&self) -> &str {
        "count"
    }
    fn description(&self) -> &str {
        "Count an invocation"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object"})
    }
    async fn call(&self, _: Value) -> Result<String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("same evidence".into())
    }
}

fn calls(ids: &[&str]) -> ChatResponse {
    ChatResponse {
        content: None,
        finish_reason: "tool_calls".into(),
        tool_calls: ids
            .iter()
            .map(|id| ToolCall {
                id: id.to_string(),
                name: "count".into(),
                arguments: json!({}),
            })
            .collect(),
    }
}

fn answer() -> ChatResponse {
    ChatResponse {
        content: Some("done".into()),
        finish_reason: "stop".into(),
        tool_calls: vec![],
    }
}

fn backend(replies: Vec<ChatResponse>) -> Scripted {
    Scripted {
        replies: Mutex::new(replies.into()),
        requests: Mutex::new(vec![]),
    }
}

#[tokio::test]
async fn system_prompt_survives_multiple_turns_without_duplication() -> Result<()> {
    let backend = backend(vec![answer(), answer()]);
    let agent = AgentLoop::new("unused", "local")
        .with_system_prompt("Use the configured home assistant tools.");
    agent.run_with_backend("first", &backend).await?;
    agent.run_with_backend("second", &backend).await?;
    let requests = backend
        .requests
        .lock()
        .map_err(|_| anyhow!("poisoned requests"))?;
    assert_eq!(requests.len(), 2);
    for (messages, _) in requests.iter() {
        assert_eq!(
            messages[0],
            json!({"role":"system","content":"Use the configured home assistant tools."})
        );
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1
        );
    }
    assert_eq!(requests[1].0[1]["content"], "first");
    assert_eq!(requests[1].0[2]["content"], "done");
    assert_eq!(requests[1].0[3]["content"], "second");
    Ok(())
}

#[tokio::test]
async fn budget_returns_a_result_for_every_call_and_finalizes_once() -> Result<()> {
    let count = Arc::new(AtomicUsize::new(0));
    let backend = backend(vec![calls(&["a", "b", "c"]), answer()]);
    let output = AgentLoop::new("unused", "local")
        .with_policy(EscalationPolicy {
            max_tool_calls: 1,
            ..Default::default()
        })
        .with_tool(Box::new(CountTool(Arc::clone(&count))))
        .run_with_backend("task", &backend)
        .await?;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(output.tool_calls_made, 1);
    let requests = backend
        .requests
        .lock()
        .map_err(|_| anyhow!("poisoned requests"))?;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].1, 0);
    let ids: Vec<_> = requests[1]
        .0
        .iter()
        .filter(|v| v["role"] == "tool")
        .map(|v| v["tool_call_id"].as_str())
        .collect();
    assert_eq!(ids, vec![Some("a"), Some("b"), Some("c")]);
    Ok(())
}

#[tokio::test]
async fn duplicate_ids_reject_entire_batch_before_side_effects() {
    let count = Arc::new(AtomicUsize::new(0));
    let result = AgentLoop::new("unused", "local")
        .with_tool(Box::new(CountTool(Arc::clone(&count))))
        .run_with_backend("task", &backend(vec![calls(&["a", "a"])]))
        .await;
    assert!(result.is_err());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn finalization_cannot_trigger_more_tools_or_model_retries() -> Result<()> {
    let backend = backend(vec![calls(&["forbidden"])]);
    let result = AgentLoop::new("unused", "local")
        .with_policy(EscalationPolicy {
            max_tool_calls: 0,
            ..Default::default()
        })
        .run_with_backend("task", &backend)
        .await;
    assert!(result.is_err());
    assert_eq!(
        backend
            .requests
            .lock()
            .map_err(|_| anyhow!("poisoned requests"))?
            .len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn repeated_evidence_enters_one_bounded_finalization() -> Result<()> {
    let count = Arc::new(AtomicUsize::new(0));
    let backend = backend(vec![calls(&["a"]), calls(&["b"]), answer()]);
    let output = AgentLoop::new("unused", "local")
        .with_policy(EscalationPolicy {
            max_tool_calls: 10,
            stagnation_window: 2,
            stagnation_min_matches: 2,
        })
        .with_tool(Box::new(CountTool(Arc::clone(&count))))
        .run_with_backend("task", &backend)
        .await?;
    assert_eq!(output.interventions, 1);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(
        backend
            .requests
            .lock()
            .map_err(|_| anyhow!("poisoned requests"))?[2]
            .1,
        0
    );
    Ok(())
}
