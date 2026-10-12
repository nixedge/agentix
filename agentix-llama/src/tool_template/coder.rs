use super::{invalid, GeneratedCall};
use agentix_infer::{exchange::ToolDefinition, InferError};
use serde_json::{Map, Value};

// This is Qwen's delimiter protocol, not XML: code and strings are literal,
// including ampersands and angle brackets. Never entity-decode argument text.
pub(super) fn parse_call(
    body: &str,
    tools: &[ToolDefinition],
) -> Result<GeneratedCall, InferError> {
    let (name, rest) = tag(body, "<function=")?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| invalid("model requested an unavailable tool"))?;
    let mut rest = rest.trim();
    let mut arguments = Map::new();
    while !rest.starts_with("</function>") {
        let (parameter, value) = tag(rest, "<parameter=")?;
        let end = value
            .find("</parameter>")
            .ok_or_else(|| invalid("truncated parameter"))?;
        let schema = tool
            .parameters
            .get("properties")
            .and_then(|p| p.get(parameter))
            .or_else(|| {
                tool.parameters
                    .get("additionalProperties")
                    .filter(|s| s.is_object())
            })
            .ok_or_else(|| invalid("unknown parameter"))?;
        // Remove only the protocol's framing newline, not meaningful whitespace
        // in a file body, shell command or patch. Replay uses the same framing.
        let value = &value[..end];
        let value = value.strip_prefix('\n').unwrap_or(value);
        let value = value.strip_suffix('\n').unwrap_or(value);
        let value = typed_value(value, schema)?;
        if arguments.insert(parameter.to_owned(), value).is_some() {
            return Err(invalid("duplicate parameter"));
        }
        rest = rest_after_parameter(rest)?.trim();
    }
    if rest != "</function>" {
        return Err(invalid("unexpected content after function"));
    }
    if let Some(required) = tool.parameters.get("required").and_then(Value::as_array) {
        for field in required {
            if !field
                .as_str()
                .is_some_and(|name| arguments.contains_key(name))
            {
                return Err(invalid("missing required parameter"));
            }
        }
    }
    Ok(GeneratedCall {
        name: name.to_owned(),
        arguments,
    })
}

fn rest_after_parameter(rest: &str) -> Result<&str, InferError> {
    rest.split_once("</parameter>")
        .map(|(_, rest)| rest)
        .ok_or_else(|| invalid("truncated parameter"))
}

fn tag<'a>(input: &'a str, prefix: &str) -> Result<(&'a str, &'a str), InferError> {
    let (name, rest) = input
        .strip_prefix(prefix)
        .and_then(|s| s.split_once('>'))
        .ok_or_else(|| invalid("malformed function or parameter tag"))?;
    if name.is_empty()
        || name.len() > 128
        || name.chars().any(|c| c.is_control() || "<>=\"'".contains(c))
    {
        return Err(invalid("invalid function or parameter name"));
    }
    Ok((name, rest))
}

fn typed_value(raw: &str, schema: &Value) -> Result<Value, InferError> {
    let kind = schema.get("type");
    let accepts = |name: &str| {
        kind.is_some_and(|kind| {
            kind == name
                || kind
                    .as_array()
                    .is_some_and(|types| types.iter().any(|t| t == name))
        })
    };
    if accepts("null") && matches!(raw.trim(), "null" | "None") {
        return Ok(Value::Null);
    }
    if accepts("string") {
        return Ok(Value::String(raw.to_owned()));
    }
    let normalized = match raw.trim() {
        "True" => "true",
        "False" => "false",
        "None" => "null",
        other => other,
    };
    let value: Value = serde_json::from_str(normalized)
        .map_err(|_| invalid("parameter is not valid JSON for its type"))?;
    let valid = kind.is_none()
        || match &value {
            Value::Null => accepts("null"),
            Value::Bool(_) => accepts("boolean"),
            Value::Number(n) => {
                accepts("number") || (accepts("integer") && (n.is_i64() || n.is_u64()))
            }
            Value::Array(_) => accepts("array"),
            Value::Object(_) => accepts("object"),
            Value::String(_) => false,
        };
    if !valid {
        return Err(invalid("parameter type mismatch"));
    }
    Ok(value)
}
