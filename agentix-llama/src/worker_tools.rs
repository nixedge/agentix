use agentix_infer::{
    exchange::{
        Cancellation, Message, ToolCall, ToolCompletionEvent as Event, ToolCompletionRequest,
        ToolDefinition, ToolFinishReason,
    },
    InferError,
};
use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tokio_stream::StreamExt;

fn invalid() -> InferError {
    InferError::InvalidToolExchange("invalid OpenAI tool exchange")
}

pub(crate) fn decode(value: &Value) -> Result<ToolCompletionRequest, InferError> {
    let messages = value["messages"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|message| {
            let text = || {
                message["content"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(invalid)
            };
            match message["role"].as_str() {
                Some("system" | "developer") => Ok(Message::System { content: text()? }),
                Some("user") => Ok(Message::User { content: text()? }),
                Some("tool") => Ok(Message::Tool {
                    content: text()?,
                    tool_call_id: message["tool_call_id"].as_str().ok_or_else(invalid)?.into(),
                }),
                Some("assistant") => {
                    let mut calls = vec![];
                    if let Some(values) = message.get("tool_calls") {
                        for call in values.as_array().ok_or_else(invalid)? {
                            if call["type"] != "function" {
                                return Err(invalid());
                            }
                            calls.push(ToolCall {
                                id: call["id"].as_str().ok_or_else(invalid)?.into(),
                                name: call["function"]["name"]
                                    .as_str()
                                    .ok_or_else(invalid)?
                                    .into(),
                                arguments: serde_json::from_str(
                                    call["function"]["arguments"].as_str().ok_or_else(invalid)?,
                                )
                                .map_err(|_| invalid())?,
                            });
                        }
                    }
                    let content = if message["content"].is_null() {
                        String::new()
                    } else {
                        text()?
                    };
                    Ok(Message::Assistant {
                        content,
                        tool_calls: calls,
                    })
                }
                _ => Err(invalid()),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut tools = vec![];
    if let Some(values) = value.get("tools") {
        for tool in values.as_array().ok_or_else(invalid)? {
            if tool["type"] != "function" {
                return Err(invalid());
            }
            tools.push(ToolDefinition {
                name: tool["function"]["name"]
                    .as_str()
                    .ok_or_else(invalid)?
                    .into(),
                description: tool["function"]
                    .get("description")
                    .map(|v| v.as_str().ok_or_else(invalid))
                    .transpose()?
                    .unwrap_or("")
                    .into(),
                parameters: tool["function"]["parameters"].clone(),
            });
        }
    }
    let request = ToolCompletionRequest {
        messages,
        tools,
        max_tokens: match value.get("max_tokens") {
            Some(v) => u32::try_from(v.as_u64().ok_or_else(invalid)?).map_err(|_| invalid())?,
            None => 2048,
        },
        temperature: value
            .get("temperature")
            .filter(|v| !v.is_null())
            .map(|v| v.as_f64().map(|v| v as f32).ok_or_else(invalid))
            .transpose()?,
    };
    request.validate()?;
    Ok(request)
}

pub(crate) async fn complete(state: &super::AppState, model: &str, value: Value) -> Response {
    if value.get("response_format").is_some_and(|v| !v.is_null()) {
        return error(InferError::InvalidToolExchange(
            "response_format is unsupported with tools",
        ));
    }
    let request = match decode(&value) {
        Ok(request) => request,
        Err(e) => return error(e),
    };
    let stream = match state
        .engine
        .complete_tools(model, request, Cancellation::default())
        .await
    {
        Ok(stream) => stream,
        Err(e) => return error(e),
    };
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let model = value["model"].clone();
    if value["stream"] == true {
        let stream = stream.map(move |item| {
            let (delta, finish) = match item {
                Ok(Event::Text(text)) => (json!({"content":text}), None),
                Ok(Event::Calls(calls)) => (json!({"tool_calls": calls.iter().enumerate().map(|(index, call)| { let mut value=wire_call(call); value["index"]=json!(index); value }).collect::<Vec<_>>()}), None),
                Ok(Event::Finished(reason)) => (json!({}), Some(reason_string(reason))),
                Err(e) => return Ok::<_, std::convert::Infallible>(format!("data: {}\n\n", json!({"error":{"code":"generation_failed","message":e.to_string()}}))),
            };
            Ok(format!("data: {}\n\n", json!({"id":id,"object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})))
        }).chain(tokio_stream::iter([Ok("data: [DONE]\n\n".to_string())]));
        return (
            [
                (header::CONTENT_TYPE, "text/event-stream"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            Body::from_stream(stream),
        )
            .into_response();
    }
    let mut stream = stream;
    let mut content = String::new();
    let mut calls = vec![];
    let mut finish = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(Event::Text(text)) => content.push_str(&text),
            Ok(Event::Calls(batch)) => calls.extend(batch.iter().map(wire_call)),
            Ok(Event::Finished(reason)) => finish = Some(reason_string(reason)),
            Err(e) => return error(e),
        }
    }
    let Some(finish) = finish else {
        return error(InferError::Backend("incomplete worker response".into()));
    };
    Json(json!({"id":id,"object":"chat.completion","model":model,"choices":[{"index":0,"message":{"role":"assistant","content":content,"tool_calls":calls},"finish_reason":finish}]})).into_response()
}

fn wire_call(call: &ToolCall) -> Value {
    json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":Value::Object(call.arguments.clone()).to_string()}})
}
fn reason_string(reason: ToolFinishReason) -> &'static str {
    match reason {
        ToolFinishReason::Stop => "stop",
        ToolFinishReason::ToolCalls => "tool_calls",
        ToolFinishReason::Length => "length",
        ToolFinishReason::Cancelled => "cancelled",
    }
}
pub(crate) fn error(error: InferError) -> Response {
    let (status, code) = match error {
        InferError::ModelNotFound(_) => (StatusCode::NOT_FOUND, "model_not_found"),
        InferError::ModelBusy(_) | InferError::CapacityExhausted => {
            (StatusCode::CONFLICT, "model_busy")
        }
        InferError::ToolCompletionUnsupported => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_tool_template",
        ),
        InferError::InvalidToolExchange(_) | InferError::ContextExceeded { .. } => {
            (StatusCode::BAD_REQUEST, "invalid_request")
        }
        InferError::Cancelled => (StatusCode::CONFLICT, "cancelled"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "inference_failed"),
    };
    (
        status,
        Json(
            json!({"error":{"code":code,"message":code,"retryable":status==StatusCode::CONFLICT}}),
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_tool_history_survives_openai_conversion() -> Result<(), InferError> {
        let request = decode(&json!({"messages":[
            {"role":"user","content":"inspect"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]},
            {"role":"tool","tool_call_id":"a","content":"data"}
        ],"tools":[]}))?;
        assert_eq!(request.messages.len(), 3);
        let Message::Assistant { tool_calls, .. } = &request.messages[1] else {
            return Err(invalid());
        };
        assert_eq!(tool_calls[0].arguments["path"], "a");
        Ok(())
    }

    #[test]
    fn invalid_histories_cannot_reach_generation() {
        for messages in [
            json!([{"role":"tool","tool_call_id":"orphan","content":"x"}]),
            json!([{"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"read","arguments":"broken"}}]}]),
            json!([{"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"read","arguments":"{}"}}]}]),
        ] {
            assert!(decode(&json!({"messages":messages})).is_err());
        }
    }
}
