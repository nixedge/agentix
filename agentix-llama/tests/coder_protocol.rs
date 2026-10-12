use agentix_infer::exchange::{Message, ToolCompletionRequest, ToolDefinition};
use agentix_llama::tool_template::{parse_output, ToolTemplate, MAX_OUTPUT};
use serde_json::json;

fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "edit".into(),
        description: "Edit a file".into(),
        parameters: json!({"type":"object", "properties": {
            "content":{"type":"string"}, "count":{"type":"integer"},
            "enabled":{"type":"boolean"}, "options":{"type":"object"},
            "paths":{"type":"array"}, "nothing":{"type":"null"},
            "expected_sha256":{"type":["string","null"]}
        }, "required":["content"]}),
    }]
}

fn call(body: &str) -> String {
    format!("<tool_call>\n<function=edit>\n{body}</function>\n</tool_call>")
}

#[test]
fn native_coder_template_round_trips_typed_calls_and_file_whitespace(
) -> Result<(), Box<dyn std::error::Error>> {
    round_trip_template(include_str!("fixtures/qwen3-coder.jinja"))
}

#[test]
fn installed_unsloth_template_round_trips_typed_calls_and_file_whitespace(
) -> Result<(), Box<dyn std::error::Error>> {
    round_trip_template(include_str!("fixtures/qwen3-coder-unsloth.jinja"))
}

fn round_trip_template(source: &str) -> Result<(), Box<dyn std::error::Error>> {
    let template = ToolTemplate::new(source)?;
    let content = "\n  if a < b && c > d {\n    println!(\"null &amp; 🦀\");\n  }\n\n";
    let options =
        json!({"a":1, "new_file":null, "enabled":true, "nested":["quoted\"\nline", false]});
    let output = call(&format!("<parameter=content>\n{content}\n</parameter>\n<parameter=count>42</parameter>\n<parameter=enabled>true</parameter>\n<parameter=options>{options}</parameter>\n<parameter=paths>[\"a\",\"b\"]</parameter>\n<parameter=nothing>null</parameter>\n"));
    let parsed = template.parse_output(&output, &tools())?;
    let expected = json!({"content":content,"count":42,"enabled":true,"options":options,"paths":["a","b"],"nothing":null});
    assert_eq!(json!(parsed.calls[0].arguments), expected);
    let id = parsed.calls[0].id.clone();
    let mut request = ToolCompletionRequest {
        messages: vec![
            Message::User {
                content: "edit".into(),
            },
            Message::Assistant {
                content: String::new(),
                tool_calls: parsed.calls,
            },
            Message::Tool {
                tool_call_id: id,
                content: "saved".into(),
            },
        ],
        tools: tools(),
        max_tokens: 32,
        temperature: None,
    };
    let rendered = template.render(&request)?;
    assert!(rendered.contains("<name>content</name>"));
    assert!(rendered.contains("<tool_response>\nsaved\n</tool_response>"));
    assert!(rendered.trim_end().ends_with("<|im_start|>assistant"));
    let history = rendered
        .split("<|im_start|>assistant")
        .nth(1)
        .ok_or("missing history")?
        .split("<|im_end|>")
        .next()
        .ok_or("missing end")?;
    assert_eq!(
        json!(template.parse_output(history, &tools())?.calls[0].arguments),
        expected
    );
    request.messages.extend([
        Message::Assistant {
            content: "Saved the edit.".into(),
            tool_calls: vec![],
        },
        Message::User {
            content: "Now review the diff.".into(),
        },
    ]);
    let follow_up = template.render(&request)?;
    assert!(follow_up.starts_with("<|im_start|>system\n"));
    // This is a generation prefix, not prose: an extra newline changes the
    // tokens from which the model continues its next tool call.
    assert!(follow_up.ends_with(concat!(
        "<tool_response>\nsaved\n</tool_response>\n<|im_end|>\n",
        "<|im_start|>assistant\nSaved the edit.<|im_end|>\n",
        "<|im_start|>user\nNow review the diff.<|im_end|>\n",
        "<|im_start|>assistant\n",
    )));
    assert!(follow_up.contains("<name>content</name>\n<type>string</type>\n</parameter>"));
    let follow_up_call = call("<parameter=content>\n  next edit\n\n</parameter>\n");
    let parsed = template.parse_output(&follow_up_call, &tools())?;
    assert_eq!(parsed.calls[0].arguments["content"], "  next edit\n");
    request.tools.clear();
    assert!(template.render(&request)?.contains(content));
    assert!(template.repair_prompt().contains("<function="));
    assert!(!ToolTemplate::new(include_str!("fixtures/qwen2.5.jinja"))?
        .repair_prompt()
        .contains("<function="));
    Ok(())
}

#[test]
fn coder_strings_are_not_json_decoded_and_batches_are_atomic(
) -> Result<(), Box<dyn std::error::Error>> {
    let template = ToolTemplate::new(include_str!("fixtures/qwen3-coder.jinja"))?;
    for content in [
        "null",
        "true",
        "42",
        "\"quoted\"",
        "",
        "  ",
        "\\n",
        "<tag>&amp;</tag>",
    ] {
        let output = call(&format!("<parameter=content>\n{content}\n</parameter>\n"));
        assert_eq!(
            template.parse_output(&output, &tools())?.calls[0].arguments["content"],
            content
        );
        assert!(parse_output(&output, &tools()).is_err());
    }
    let valid = call("<parameter=content>x</parameter>");
    let create =
        call("<parameter=content>x</parameter><parameter=expected_sha256>\nnull\n</parameter>");
    assert!(
        template.parse_output(&create, &tools())?.calls[0].arguments["expected_sha256"].is_null()
    );
    let parsed = template.parse_output(&format!("Inspecting.{valid}\n{valid}"), &tools())?;
    assert_eq!(parsed.text, "Inspecting.\n");
    assert_eq!(parsed.calls.len(), 2);
    assert_ne!(parsed.calls[0].id, parsed.calls[1].id);
    assert!(template
        .parse_output(&format!("{valid}<tool_call>"), &tools())
        .is_err());
    assert!(template.parse_output(&valid.repeat(129), &tools()).is_err());
    assert!(template
        .parse_output(&"x".repeat(MAX_OUTPUT + 1), &tools())
        .is_err());
    assert!(template.parse_output(&valid, &[]).is_err());
    Ok(())
}

#[test]
fn malformed_coder_calls_never_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    let template = ToolTemplate::new(include_str!("fixtures/qwen3-coder.jinja"))?;
    for body in [
        "",
        "<parameter=content>x",
        "<parameter=content>x</parameter>garbage",
        "<parameter=content>x</parameter><parameter=content>y</parameter>",
        "<parameter=unknown>x</parameter>",
        "<parameter=content>x</parameter><parameter=count>1.2</parameter>",
        "<parameter=content>x</parameter><parameter=enabled>yes</parameter>",
        "<parameter=content>x</parameter><parameter=options>[]</parameter>",
        "<parameter=content>x</parameter><parameter=paths>{}</parameter>",
    ] {
        assert!(
            template.parse_output(&call(body), &tools()).is_err(),
            "accepted {body}"
        );
    }
    for output in [
        "<tool_call><function=missing></function></tool_call>",
        "<tool_call><function=edit><parameter=content>x</parameter></tool_call>",
        "<tool_call><function=edit><parameter=content>x</parameter></function>extra</tool_call>",
        "<tool_call>{\"name\":\"edit\",\"arguments\":{\"content\":\"x\"}}</tool_call>",
    ] {
        assert!(
            template.parse_output(output, &tools()).is_err(),
            "accepted {output}"
        );
    }
    Ok(())
}
