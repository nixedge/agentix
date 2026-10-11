use agentix_api::{
    agent::{AgentEvent, EscalationRequest, EventEnvelope},
    models::{ModelOperationRequest, Revision},
    orchestration::{Heartbeat, RoleCapability, TaskState},
    wire::{decode_request, Sha256, Text, MAX_REQUEST_BYTES},
};
use serde_json::{json, Value};

const ID: &str = "00000000-0000-4000-8000-000000000001";

fn escalation() -> Value {
    json!({
        "schema_version": 1,
        "knowledge": "Read the local implementation and tests.",
        "question": "Which invariant explains this behavior?",
        "local_insufficiency": "The dependency sources do not document the invariant.",
        "target": {"provider": "open_ai", "model": "explicit-cloud-model"},
        "context": {"session_id": ID, "turn_id": ID, "invocation_id": "call-1"}
    })
}

#[test]
fn escalation_requires_bounded_rationale_and_rejects_permission_claims(
) -> Result<(), Box<dyn std::error::Error>> {
    let request: EscalationRequest = serde_json::from_value(escalation())?;
    assert_eq!(request.context.invocation_id.as_str(), "call-1");
    for field in ["knowledge", "question", "local_insufficiency"] {
        let mut value = escalation();
        value[field] = json!("   ");
        assert!(serde_json::from_value::<EscalationRequest>(value).is_err());
    }
    let mut value = escalation();
    value["approved"] = json!(true);
    assert!(serde_json::from_value::<EscalationRequest>(value).is_err());
    let mut value = escalation();
    value["question"] = json!("x".repeat(8193));
    assert!(serde_json::from_value::<EscalationRequest>(value).is_err());
    Ok(())
}

#[test]
fn versions_and_nested_unknown_fields_fail_closed() {
    for version in [json!(0), json!(2), json!("1"), Value::Null] {
        let mut value = escalation();
        value["schema_version"] = version;
        assert!(serde_json::from_value::<EscalationRequest>(value).is_err());
    }
    let mut value = escalation();
    value["target"]["base_url"] = json!("https://arbitrary.example");
    assert!(serde_json::from_value::<EscalationRequest>(value).is_err());
}

#[test]
fn installs_require_immutable_verified_artifacts() -> Result<(), Box<dyn std::error::Error>> {
    let valid = json!({"kind":"install_and_warm", "schema_version":1,
        "idempotency_key":"install-1", "repo_id":"owner/model",
        "revision_commit":"a".repeat(40), "filename":"model.gguf",
        "expected_sha256":"b".repeat(64), "expected_bytes":1024});
    serde_json::from_value::<ModelOperationRequest>(valid.clone())?;
    for (field, invalid) in [
        ("revision_commit", json!("main")),
        ("expected_sha256", json!("unknown")),
        ("expected_bytes", json!(0)),
        ("download_url", json!("https://arbitrary.example")),
    ] {
        let mut value = valid.clone();
        value[field] = invalid;
        assert!(serde_json::from_value::<ModelOperationRequest>(value).is_err());
    }
    Ok(())
}

#[test]
fn worker_payload_cannot_supply_a_role_or_capability() -> Result<(), Box<dyn std::error::Error>> {
    let valid = json!({"schema_version":1,"assignment":{"assignment_id":ID,"generation":1}});
    serde_json::from_value::<Heartbeat>(valid.clone())?;
    for (field, invalid) in [
        ("role", json!("supervisor")),
        ("capability", json!("forged")),
    ] {
        let mut value = valid.clone();
        value[field] = invalid;
        assert!(serde_json::from_value::<Heartbeat>(value).is_err());
    }
    let mut value = valid;
    value["assignment"]["generation"] = json!(0);
    assert!(serde_json::from_value::<Heartbeat>(value).is_err());
    Ok(())
}

#[test]
fn capabilities_do_not_leak_through_debug() -> Result<(), Box<dyn std::error::Error>> {
    let raw = "c".repeat(64);
    let capability = RoleCapability::new(raw.clone())?;
    assert!(!format!("{capability:?}").contains(&raw));
    assert_eq!(capability.expose_for_header(), raw);
    Ok(())
}

#[test]
fn event_envelopes_keep_the_jsonl_shape_and_reject_unknown_data(
) -> Result<(), Box<dyn std::error::Error>> {
    let valid = json!({"schema_version":1,"session_id":ID,"turn_id":ID,"sequence":7,
        "type":"text_delta","payload":{"text":"hello"}});
    let event: EventEnvelope = serde_json::from_value(valid.clone())?;
    assert!(matches!(event.event, AgentEvent::TextDelta { .. }));
    assert_eq!(serde_json::to_value(event)?, valid);
    let mut extra = valid.clone();
    extra["role"] = json!("supervisor");
    assert!(serde_json::from_value::<EventEnvelope>(extra).is_err());
    let mut extra = valid;
    extra["payload"]["unexpected"] = json!(true);
    assert!(serde_json::from_value::<EventEnvelope>(extra).is_err());
    Ok(())
}

#[test]
fn bounded_decoder_rejects_large_payloads_without_echoing_them() {
    let large = vec![b'x'; MAX_REQUEST_BYTES + 1];
    assert!(decode_request::<EscalationRequest>(&large).is_err());
    let invalid = br#"{"secret":"do-not-echo-me"}"#;
    let result = decode_request::<EscalationRequest>(invalid);
    assert!(result.is_err());
    assert!(!format!("{result:?}").contains("do-not-echo-me"));
}

#[test]
fn scalar_invariants_and_terminal_states_are_explicit() {
    assert!(Text::<8>::new("123456789").is_err());
    assert!(Text::<8>::new("\0").is_err());
    assert!(Sha256::new("A".repeat(64)).is_err());
    assert!(Revision::new("abcdef").is_err());
    assert!(!TaskState::Reviewing.is_terminal());
    assert!(!TaskState::Revoking.is_terminal());
    assert!(TaskState::Completed.is_terminal());
}

#[test]
fn legacy_local_chat_shape_remains_compatible() -> Result<(), Box<dyn std::error::Error>> {
    let request: agentix_api::ChatCompletionRequest = serde_json::from_value(json!({
        "model":"local-model", "messages":[{"role":"user","content":"hello"}],"stream":true
    }))?;
    assert_eq!(request.model, "local-model");
    assert_eq!(request.stream, Some(true));
    Ok(())
}
