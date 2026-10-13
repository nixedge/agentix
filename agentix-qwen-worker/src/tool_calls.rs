use serde_json::{Map, Value};

const OPEN_CALL: &str = "<tool_call>";
const CLOSE_CALL: &str = "</tool_call>";
const CLOSE_FUNCTION: &str = "</function>";
const CLOSE_PARAMETER: &str = "</parameter>";

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedToolOutput {
    pub content: Option<String>,
    pub calls: Vec<ParsedToolCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallError {
    InvalidDefinitions,
    MalformedOutput,
}

/// Converts OpenAI's JSON-encoded assistant arguments into objects for the
/// pinned Qwen template, which iterates argument key/value pairs.
pub fn normalize_tool_history(tool_calls: &Value) -> Result<Value, ToolCallError> {
    let calls = tool_calls
        .as_array()
        .ok_or(ToolCallError::MalformedOutput)?;
    let mut normalized = tool_calls.clone();
    let normalized_calls = normalized
        .as_array_mut()
        .ok_or(ToolCallError::MalformedOutput)?;
    for (original, normalized_call) in calls.iter().zip(normalized_calls) {
        let original_arguments = original
            .get("function")
            .and_then(|function| function.get("arguments"))
            .ok_or(ToolCallError::MalformedOutput)?;
        let arguments = match original_arguments {
            Value::String(encoded) => {
                serde_json::from_str(encoded).map_err(|_| ToolCallError::MalformedOutput)?
            }
            Value::Object(_) => original_arguments.clone(),
            _ => return Err(ToolCallError::MalformedOutput),
        };
        if !arguments.is_object() {
            return Err(ToolCallError::MalformedOutput);
        }
        let function = normalized_call
            .get_mut("function")
            .and_then(Value::as_object_mut)
            .ok_or(ToolCallError::MalformedOutput)?;
        function.insert("arguments".to_owned(), arguments);
    }
    Ok(normalized)
}

/// Validates the scalar JSON Schema subset used by Agentix function tools.
pub fn validate_tool_definitions(tools: &[Value]) -> Result<(), ToolCallError> {
    let mut names = Vec::new();
    for tool in tools {
        let object = tool.as_object().ok_or(ToolCallError::InvalidDefinitions)?;
        if object.keys().any(|key| key != "type" && key != "function")
            || object.get("type").and_then(Value::as_str) != Some("function")
        {
            return Err(ToolCallError::InvalidDefinitions);
        }
        let function = object
            .get("function")
            .and_then(Value::as_object)
            .ok_or(ToolCallError::InvalidDefinitions)?;
        if function.keys().any(|key| {
            !matches!(
                key.as_str(),
                "name" | "description" | "parameters" | "strict"
            )
        }) {
            return Err(ToolCallError::InvalidDefinitions);
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| valid_name(name))
            .ok_or(ToolCallError::InvalidDefinitions)?;
        if names.contains(&name) {
            return Err(ToolCallError::InvalidDefinitions);
        }
        names.push(name);

        let parameters = function
            .get("parameters")
            .and_then(Value::as_object)
            .ok_or(ToolCallError::InvalidDefinitions)?;
        if parameters.keys().any(|key| {
            !matches!(
                key.as_str(),
                "type" | "properties" | "required" | "additionalProperties" | "description"
            )
        }) || parameters.get("type").and_then(Value::as_str) != Some("object")
            || parameters
                .get("additionalProperties")
                .is_some_and(|value| value != false)
        {
            return Err(ToolCallError::InvalidDefinitions);
        }
        let properties = parameters
            .get("properties")
            .and_then(Value::as_object)
            .ok_or(ToolCallError::InvalidDefinitions)?;
        for property in properties.values() {
            let schema = property
                .as_object()
                .ok_or(ToolCallError::InvalidDefinitions)?;
            if schema
                .keys()
                .any(|key| key != "type" && key != "description")
                || !matches!(
                    schema.get("type").and_then(Value::as_str),
                    Some("string" | "integer" | "number" | "boolean")
                )
            {
                return Err(ToolCallError::InvalidDefinitions);
            }
        }
        if let Some(required) = parameters.get("required") {
            let required = required
                .as_array()
                .ok_or(ToolCallError::InvalidDefinitions)?;
            for item in required {
                let name = item.as_str().ok_or(ToolCallError::InvalidDefinitions)?;
                if !properties.contains_key(name) {
                    return Err(ToolCallError::InvalidDefinitions);
                }
            }
        }
    }
    Ok(())
}

/// Parses complete Qwen `<tool_call>` blocks and validates names and arguments.
pub fn parse_tool_output(
    output: &str,
    tools: &[Value],
) -> Result<Option<ParsedToolOutput>, ToolCallError> {
    let Some(first_start) = output.find(OPEN_CALL) else {
        return if output.contains(CLOSE_CALL) {
            Err(ToolCallError::MalformedOutput)
        } else {
            Ok(None)
        };
    };
    if tools.is_empty() {
        return Err(ToolCallError::MalformedOutput);
    }
    validate_tool_definitions(tools).map_err(|_| ToolCallError::MalformedOutput)?;

    let prefix = output[..first_start].trim_end();
    let content = (!prefix.is_empty()).then(|| prefix.to_owned());
    let mut remaining = &output[first_start..];
    let mut calls = Vec::new();
    loop {
        let Some(after_open) = remaining.strip_prefix(OPEN_CALL) else {
            return Err(ToolCallError::MalformedOutput);
        };
        let close = after_open
            .find(CLOSE_CALL)
            .ok_or(ToolCallError::MalformedOutput)?;
        let call_body = &after_open[..close];
        calls.push(parse_call_body(call_body, tools)?);
        remaining = after_open[close + CLOSE_CALL.len()..].trim_start();
        if remaining.is_empty() {
            break;
        }
        if !remaining.starts_with(OPEN_CALL) {
            return Err(ToolCallError::MalformedOutput);
        }
    }
    if calls.is_empty() {
        return Err(ToolCallError::MalformedOutput);
    }
    Ok(Some(ParsedToolOutput { content, calls }))
}

fn parse_call_body(body: &str, tools: &[Value]) -> Result<ParsedToolCall, ToolCallError> {
    let body = body.trim_start();
    let after_function = body
        .strip_prefix("<function=")
        .ok_or(ToolCallError::MalformedOutput)?;
    let name_end = after_function
        .find('>')
        .ok_or(ToolCallError::MalformedOutput)?;
    let name = &after_function[..name_end];
    if !valid_name(name) {
        return Err(ToolCallError::MalformedOutput);
    }
    let tool = find_tool(tools, name).ok_or(ToolCallError::MalformedOutput)?;
    let function = tool
        .get("function")
        .and_then(Value::as_object)
        .ok_or(ToolCallError::MalformedOutput)?;
    let parameters = function
        .get("parameters")
        .and_then(Value::as_object)
        .ok_or(ToolCallError::MalformedOutput)?;
    let properties = parameters
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(ToolCallError::MalformedOutput)?;

    let mut rest = after_function[name_end + 1..].trim_start_matches('\n');
    let mut arguments = Map::new();
    loop {
        rest = rest.trim_start_matches('\n');
        if let Some(after_close) = rest.strip_prefix(CLOSE_FUNCTION) {
            if !after_close.trim().is_empty() {
                return Err(ToolCallError::MalformedOutput);
            }
            break;
        }
        let after_parameter = rest
            .strip_prefix("<parameter=")
            .ok_or(ToolCallError::MalformedOutput)?;
        let parameter_end = after_parameter
            .find('>')
            .ok_or(ToolCallError::MalformedOutput)?;
        let parameter_name = &after_parameter[..parameter_end];
        let schema = properties
            .get(parameter_name)
            .ok_or(ToolCallError::MalformedOutput)?;
        let parameter_body = &after_parameter[parameter_end + 1..];
        let value_end = parameter_body
            .find(CLOSE_PARAMETER)
            .ok_or(ToolCallError::MalformedOutput)?;
        let mut raw_value = &parameter_body[..value_end];
        raw_value = raw_value.strip_prefix('\n').unwrap_or(raw_value);
        raw_value = raw_value.strip_suffix('\n').unwrap_or(raw_value);
        let value = parse_parameter_value(raw_value, schema)?;
        if arguments.insert(parameter_name.to_owned(), value).is_some() {
            return Err(ToolCallError::MalformedOutput);
        }
        rest = &parameter_body[value_end + CLOSE_PARAMETER.len()..];
    }
    validate_arguments(&arguments, parameters)?;
    Ok(ParsedToolCall {
        name: name.to_owned(),
        arguments: Value::Object(arguments),
    })
}

fn parse_parameter_value(raw: &str, schema: &Value) -> Result<Value, ToolCallError> {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Ok(Value::String(raw.to_owned())),
        Some("integer" | "number" | "boolean") => {
            serde_json::from_str(raw).map_err(|_| ToolCallError::MalformedOutput)
        }
        _ => Err(ToolCallError::MalformedOutput),
    }
}

fn validate_arguments(
    arguments: &Map<String, Value>,
    parameters: &Map<String, Value>,
) -> Result<(), ToolCallError> {
    let properties = parameters
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(ToolCallError::MalformedOutput)?;
    for (name, value) in arguments {
        let expected_type = properties
            .get(name)
            .and_then(|schema| schema.get("type"))
            .and_then(Value::as_str)
            .ok_or(ToolCallError::MalformedOutput)?;
        let valid = match expected_type {
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            _ => false,
        };
        if !valid {
            return Err(ToolCallError::MalformedOutput);
        }
    }
    if let Some(required) = parameters.get("required") {
        let required = required.as_array().ok_or(ToolCallError::MalformedOutput)?;
        if required.iter().any(|item| match item.as_str() {
            Some(name) => !arguments.contains_key(name),
            None => true,
        }) {
            return Err(ToolCallError::MalformedOutput);
        }
    }
    Ok(())
}

fn find_tool<'a>(tools: &'a [Value], name: &str) -> Option<&'a Value> {
    tools.iter().find(|tool| {
        tool.get("function")
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            == Some(name)
    })
}

fn valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::{parse_tool_output, validate_tool_definitions, ToolCallError};
    use serde_json::{json, Value};

    fn definitions() -> Vec<Value> {
        vec![json!({
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"}
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }
            }
        })]
    }

    #[test]
    fn parses_and_validates_a_qwen_tool_call_with_optional_prefix() {
        let output = "Checking the project first.\n\n<tool_call>\n<function=run_command>\n<parameter=command>\ncargo test -p agentix-qwen\n</parameter>\n</function>\n</tool_call>";
        let parsed = parse_tool_output(output, &definitions());
        assert_eq!(
            parsed,
            Ok(Some(super::ParsedToolOutput {
                content: Some("Checking the project first.".to_owned()),
                calls: vec![super::ParsedToolCall {
                    name: "run_command".to_owned(),
                    arguments: json!({"command": "cargo test -p agentix-qwen"}),
                }],
            }))
        );
    }

    #[test]
    fn plain_text_and_multiple_calls_are_supported() {
        let tools = definitions();
        assert_eq!(parse_tool_output("finished", &tools), Ok(None));
        let output = "<tool_call><function=run_command><parameter=command>pwd</parameter></function></tool_call>\n<tool_call><function=run_command><parameter=command>git status</parameter></function></tool_call>";
        let parsed = parse_tool_output(output, &tools);
        let calls = match parsed {
            Ok(Some(super::ParsedToolOutput {
                content: None,
                calls,
            })) => calls,
            _ => panic!("expected two parsed calls"),
        };
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, json!({"command": "pwd"}));
        assert_eq!(calls[1].arguments, json!({"command": "git status"}));
    }

    #[test]
    fn rejects_unknown_tools_missing_arguments_and_incomplete_markup() {
        let tools = definitions();
        for output in [
            "<tool_call><function=unknown></function></tool_call>",
            "<tool_call><function=run_command></function></tool_call>",
            "<tool_call><function=run_command><parameter=command>pwd</parameter>",
        ] {
            assert_eq!(
                parse_tool_output(output, &tools),
                Err(ToolCallError::MalformedOutput)
            );
        }
    }

    #[test]
    fn rejects_duplicate_and_unsupported_tool_schemas() {
        let mut tools = definitions();
        tools.push(tools[0].clone());
        assert_eq!(
            validate_tool_definitions(&tools),
            Err(ToolCallError::InvalidDefinitions)
        );

        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "nested",
                "parameters": {
                    "type": "object",
                    "properties": {"options": {"type": "array"}}
                }
            }
        })];
        assert_eq!(
            validate_tool_definitions(&tools),
            Err(ToolCallError::InvalidDefinitions)
        );
    }

    #[test]
    fn normalizes_openai_tool_history_for_the_qwen_template() {
        let calls = json!([{
            "id": "call_123",
            "type": "function",
            "function": {
                "name": "run_command",
                "arguments": "{\"command\":\"pwd\"}"
            }
        }]);
        assert_eq!(
            super::normalize_tool_history(&calls),
            Ok(json!([{
                "id": "call_123",
                "type": "function",
                "function": {
                    "name": "run_command",
                    "arguments": {"command": "pwd"}
                }
            }]))
        );
    }
}
