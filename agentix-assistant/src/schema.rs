use serde_json::{json, Value};

fn object(action: &str, properties: Value, fields: &[&str]) -> Value {
    let mut properties = properties.as_object().cloned().unwrap_or_default();
    properties.insert("action".into(), json!({"const": action}));
    let mut required = vec!["action"];
    required.extend_from_slice(fields);
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

pub fn decision_schema() -> Value {
    // llama.cpp's grammar parser caps explicit repetition at 2000. Keep byte
    // bounds in Rust validation; generation is separately capped by max_tokens.
    let text = json!({"type":"string","minLength":1});
    let id = json!({"type":"string","format":"uuid"});
    let revision = json!({"type":"integer","minimum":1});
    let nullable_id = json!({"anyOf":[id,{"type":"null"}]});
    let due = json!({"anyOf":[{"type":"string","format":"date-time"},{"type":"null"}]});
    json!({"oneOf":[
        object("add_list_item", json!({"list_name":text,"list_id":nullable_id,"text":text}), &["list_name","list_id","text"]),
        object("show_list", json!({"list_name":text,"list_id":nullable_id}), &["list_name","list_id"]),
        object("save_note", json!({"text":text}), &["text"]),
        object("save_task", json!({"title":text,"due_at":due}), &["title","due_at"]),
        object("edit_list_item", json!({"id":id,"expected_revision":revision,"text":text}), &["id","expected_revision","text"]),
        object("set_task_completed", json!({"id":id,"expected_revision":revision,"completed":{"type":"boolean"}}), &["id","expected_revision","completed"]),
        object("delete_record", json!({"id":id,"expected_revision":revision}), &["id","expected_revision"]),
        object("unsupported", json!({"capability":{"type":"string","enum":["email","calendar","reminder","other"]}}), &["capability"]),
        object("clarify", json!({"question":text}), &["question"])
    ]})
}
