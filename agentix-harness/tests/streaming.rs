use agentix_harness::streaming::SseCompletion;
use anyhow::Result;
use serde_json::{json, Value};
fn frame(delta: Value, finish: Value) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}
fn stream(arguments: &str, ids: &[&str]) -> String {
    let calls:Vec<_>=ids.iter().enumerate().map(|(index,id)|json!({"index":index,"id":id,"type":"function","function":{"name":"read_file","arguments":arguments}})).collect();
    format!(
        "{}{}data: [DONE]\r\n\r\n",
        frame(json!({"tool_calls":calls}), Value::Null),
        frame(json!({}), json!("tool_calls"))
    )
}
#[test]
fn every_byte_boundary_preserves_unicode_and_complete_calls() -> Result<()> {
    let wire = format!(
        "{}{}",
        frame(json!({"content":"héllo 🦀"}), Value::Null),
        stream("{\"path\":\"a.rs\"}", &["one"])
    );
    for split in 0..wire.len() {
        let mut decoder = SseCompletion::default();
        decoder.push(&wire.as_bytes()[..split], None)?;
        decoder.push(&wire.as_bytes()[split..], None)?;
        let response = decoder.finish()?;
        assert_eq!(response.content.as_deref(), Some("héllo 🦀"));
        assert_eq!(response.tool_calls[0].arguments["path"], "a.rs");
    }
    Ok(())
}
#[test]
fn rejects_duplicates_malformed_and_truncated_streams() -> Result<()> {
    for wire in [
        stream("{}", &["same", "same"]),
        stream("{", &["a"]),
        stream("null", &["a"]),
        stream("{}", &["a"]).replace("data: [DONE]\r\n\r\n", ""),
    ] {
        let mut decoder = SseCompletion::default();
        decoder.push(wire.as_bytes(), None)?;
        assert!(decoder.finish().is_err());
    }
    let mut decoder = SseCompletion::default();
    assert!(decoder.push(b"data: [DONE]\n\n", None).is_err());
    Ok(())
}
#[test]
fn fragmented_arguments_require_a_terminal_frame() -> Result<()> {
    let mut decoder = SseCompletion::default();
    decoder.push(frame(json!({"tool_calls":[{"index":0,"id":"a","function":{"name":"read_","arguments":"{\"pa"}}]}),Value::Null).as_bytes(),None)?;
    decoder.push(frame(json!({"tool_calls":[{"index":0,"function":{"name":"file","arguments":"th\":\"x\"}"}}]}),json!("tool_calls")).as_bytes(),None)?;
    decoder.push(b"data: [DONE]\n\n", None)?;
    assert_eq!(decoder.finish()?.tool_calls[0].name, "read_file");
    Ok(())
}

#[test]
fn finish_errors_explain_truncation_and_inconsistent_batches() -> Result<()> {
    for (delta, reason, expected) in [
        (
            json!({"content":"partial answer"}),
            "length",
            "output token limit",
        ),
        (
            json!({"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"write_file","arguments":"{\"content\":\"partial"}}]}),
            "length",
            "output token limit",
        ),
        (json!({}), "tool_calls", "received 0 tool calls"),
        (
            json!({"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"read_file","arguments":"{}"}}]}),
            "stop",
            "received 1 tool calls",
        ),
        (json!({}), "content_filter", "content_filter"),
    ] {
        let wire = format!(
            "{}{}data: [DONE]\n\n",
            frame(delta, Value::Null),
            frame(json!({}), json!(reason))
        );
        let mut decoder = SseCompletion::default();
        decoder.push(wire.as_bytes(), None)?;
        let error = match decoder.finish() {
            Err(error) => error,
            Ok(_) => anyhow::bail!("incomplete responses must not dispatch tools"),
        };
        let detail = format!("{error:#}");
        assert!(detail.contains(expected), "{detail}");
        assert!(detail.contains(reason), "{detail}");
    }
    Ok(())
}
