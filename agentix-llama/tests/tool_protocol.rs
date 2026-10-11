use agentix_infer::exchange::{Message, ToolCompletionRequest, ToolDefinition};
use agentix_llama::tool_template::{parse_output, ToolTemplate};
use serde_json::json;

fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "read_file".into(),
        description: "Read a file".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
    }]
}

#[test]
fn native_template_preserves_tools_history_and_literal_null(
) -> Result<(), Box<dyn std::error::Error>> {
    let template = ToolTemplate::new(include_str!("fixtures/qwen2.5.jinja"))?;
    let calls = parse_output(
        r#"<tool_call>{"name":"read_file","arguments":{"path":"src/lib.rs"}}</tool_call>"#,
        &tools(),
    )?
    .calls;
    let id = calls[0].id.clone();
    let mut request = ToolCompletionRequest {
        messages: vec![
            Message::User {
                content: "null".into(),
            },
            Message::Assistant {
                content: String::new(),
                tool_calls: calls,
            },
            Message::Tool {
                tool_call_id: id,
                content: "file content here".into(),
            },
        ],
        tools: tools(),
        max_tokens: 32,
        temperature: None,
    };
    let rendered = template.render(&request)?;
    assert!(rendered.contains("user\nnull<|im_end|>"));
    assert!(rendered.contains("src/lib.rs"));
    assert!(rendered.contains("file content here"));
    assert!(
        rendered.ends_with("<|im_start|>assistant\n"),
        "rendered suffix: {:?}",
        &rendered[rendered.len().saturating_sub(80)..]
    );
    request.tools.clear();
    let finalization = template.render(&request)?;
    assert!(finalization.contains("src/lib.rs"));
    assert!(!finalization.contains("# Tools"));
    Ok(())
}

#[test]
fn malformed_truncated_unknown_and_oversized_calls_fail_closed() {
    for output in [
        "<tool_call>{}",
        "</tool_call>",
        "<tool_call",
        "<tool_call>not json</tool_call>",
        r#"<tool_call>{"name":"read_file"}</tool_call>"#,
        r#"<tool_call>{"name":"read_file","arguments":"{}"}</tool_call>"#,
        r#"<tool_call>{"name":"read_file","arguments":[],"approved":true}</tool_call>"#,
        r#"<tool_call>{"name":"missing","arguments":{}}</tool_call>"#,
        r#"<tool_call>{"name":"read_file","arguments":{}}</tool_call><tool_call>"#,
    ] {
        assert!(parse_output(output, &tools()).is_err(), "accepted {output}");
    }
    assert!(parse_output(&"x".repeat(1024 * 1024 + 1), &tools()).is_err());
}

#[test]
fn calls_have_distinct_ids_and_text_is_not_lost() -> Result<(), Box<dyn std::error::Error>> {
    let output = r#"Inspecting.<tool_call>{"name":"read_file","arguments":{"path":"a"}}</tool_call><tool_call>{"name":"read_file","arguments":{"path":"b"}}</tool_call>More text."#;
    let parsed = parse_output(output, &tools())?;
    assert_eq!(parsed.text, "Inspecting.More text.");
    assert_eq!(parsed.calls.len(), 2);
    assert_ne!(parsed.calls[0].id, parsed.calls[1].id);
    assert_eq!(parse_output("plain text", &[])?.text, "plain text");
    assert!(parse_output(output, &[]).is_err());
    Ok(())
}

#[test]
fn unsupported_or_broken_templates_are_not_coding_ready() {
    for template in [
        "",
        "{{ messages }}",
        "<|im_start|><tool_call></tool_call>tools tool_calls",
        "{% broken %}<|im_start|><tool_call></tool_call>tools tool_calls",
    ] {
        assert!(ToolTemplate::new(template).is_err());
    }
}
