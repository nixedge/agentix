use agentix_infer::{
    exchange::{Message, ToolCall, ToolCompletionRequest, ToolDefinition},
    InferError,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;

mod coder;

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";
pub const MAX_OUTPUT: usize = 1024 * 1024;

pub struct ToolTemplate {
    source: String,
    format: CallFormat,
}

#[derive(Clone, Copy)]
enum CallFormat {
    Json,
    Coder,
}

impl ToolTemplate {
    pub fn new(source: &str) -> Result<Self, InferError> {
        if source.len() > 64 * 1024
            || ![OPEN, CLOSE, "tools", "tool_calls", "<|im_start|>"]
                .iter()
                .all(|marker| source.contains(marker))
        {
            return Err(InferError::ToolCompletionUnsupported);
        }
        let mut template = Self {
            source: source.to_string(),
            format: CallFormat::Json,
        };
        // Verify both advertised tools and historical calls/results survive the
        // actual template. Token spelling alone is not a capability declaration.
        let probe = ToolCompletionRequest {
            messages: vec![
                Message::User {
                    content: "probe_user_739".into(),
                },
                Message::Assistant {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "probe_id".into(),
                        name: "probe_function_739".into(),
                        arguments: serde_json::from_value(json!({"probe_arg_739": 1}))
                            .map_err(|_| invalid("invalid probe"))?,
                    }],
                },
                Message::Tool {
                    tool_call_id: "probe_id".into(),
                    content: "probe_result_739".into(),
                },
            ],
            tools: vec![ToolDefinition {
                name: "probe_definition_739".into(),
                description: "probe_description_739".into(),
                parameters: json!({"type":"object", "properties":{"probe_arg_739":{"type":"integer"}}, "required":["probe_arg_739"]}),
            }],
            max_tokens: 1,
            temperature: None,
        };
        let rendered = template.render(&probe)?;
        if ![
            "probe_user_739",
            "probe_function_739",
            "probe_arg_739",
            "probe_result_739",
            "probe_definition_739",
            OPEN,
            CLOSE,
        ]
        .iter()
        .all(|marker| rendered.contains(marker))
        {
            return Err(InferError::ToolCompletionUnsupported);
        }
        let history = rendered
            .split("<|im_start|>assistant")
            .nth(1)
            .and_then(|s| s.split("<|im_end|>").next())
            .ok_or(InferError::ToolCompletionUnsupported)?;
        if history.contains("<function=probe_function_739>") {
            template.format = CallFormat::Coder;
        }
        let historical_tools = vec![ToolDefinition {
            name: "probe_function_739".into(),
            description: String::new(),
            parameters: json!({"type":"object", "properties":{"probe_arg_739":{"type":"integer"}}}),
        }];
        let parsed = template.parse_output(history, &historical_tools)?;
        if parsed.calls.len() != 1 || parsed.calls[0].arguments != probe_call_arguments() {
            return Err(InferError::ToolCompletionUnsupported);
        }
        Ok(template)
    }

    pub fn parse_output(
        &self,
        output: &str,
        tools: &[ToolDefinition],
    ) -> Result<ParsedOutput, InferError> {
        parse_with_format(output, tools, self.format)
    }

    pub fn repair_prompt(&self) -> &'static str {
        match self.format {
            CallFormat::Json => "Your previous tool response was invalid. Return only complete <tool_call> blocks with an available function name and an arguments JSON object, or answer without tools.",
            CallFormat::Coder => "Your previous tool response was invalid. Use the advertised tool format: <tool_call>\n<function=available_function_name>\n<parameter=parameter_name>\nvalue\n</parameter>\n</function>\n</tool_call>. Include all required parameters, with raw text for strings and JSON for arrays/objects, or answer without tools.",
        }
    }

    pub fn render(&self, request: &ToolCompletionRequest) -> Result<String, InferError> {
        request.validate()?;
        let messages: Vec<Value> = request.messages.iter().map(|message| match message {
            Message::System { content } => json!({"role":"system", "content":content}),
            Message::User { content } => json!({"role":"user", "content":content}),
            Message::Tool { content, tool_call_id } => json!({"role":"tool", "content":content, "tool_call_id":tool_call_id}),
            Message::Assistant { content, tool_calls } => {
                let calls: Vec<Value> = tool_calls.iter().map(|call| {
                    // Coder parameters are literal text. Some native templates
                    // use Jinja's display conversion, which is not JSON for
                    // nested nulls/bools. Supply their wire text explicitly.
                    let arguments: serde_json::Map<String, Value> = call.arguments.iter().map(|(name, value)| {
                        let value = match self.format {
                            CallFormat::Coder if !value.is_string() => Value::String(value.to_string()),
                            _ => value.clone(),
                        };
                        (name.clone(), value)
                    }).collect();
                    json!({"id":call.id, "type":"function", "function":{"name":call.name, "arguments":arguments}})
                }).collect();
                json!({"role":"assistant", "content":content, "tool_calls":calls})
            }
        }).collect();
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| json!({"type":"function", "function":tool}))
            .collect();
        let mut env = minijinja::Environment::new();
        // Match Hugging Face's chat-template environment. Literal newlines
        // around Jinja blocks otherwise change the assistant's token prefix.
        env.set_trim_blocks(true);
        env.set_lstrip_blocks(true);
        env.set_fuel(Some(200_000));
        env.set_recursion_limit(64);
        env.add_function(
            "raise_exception",
            |_message: String| -> Result<String, minijinja::Error> {
                Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "template rejected messages",
                ))
            },
        );
        let template = env
            .template_from_str(&self.source)
            .map_err(|_| invalid("unsupported template syntax"))?;
        let mut output = BoundedWriter(Vec::new());
        template.render_captured_to(json!({"messages":messages, "tools":tools, "add_generation_prompt":true, "enable_thinking":false, "bos_token":"", "eos_token":"<|im_end|>"}), &mut output)
            .map_err(|_| invalid("template rendering failed or exceeded limits"))?;
        String::from_utf8(output.0).map_err(|_| invalid("template output is not UTF-8"))
    }
}

fn probe_call_arguments() -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([("probe_arg_739".into(), json!(1))])
}

struct BoundedWriter(Vec<u8>);
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_OUTPUT {
            return Err(std::io::Error::other("rendered prompt exceeds limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedCall {
    name: String,
    arguments: serde_json::Map<String, Value>,
}

pub struct ParsedOutput {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

pub fn parse_output(output: &str, tools: &[ToolDefinition]) -> Result<ParsedOutput, InferError> {
    parse_with_format(output, tools, CallFormat::Json)
}

fn parse_with_format(
    output: &str,
    tools: &[ToolDefinition],
    format: CallFormat,
) -> Result<ParsedOutput, InferError> {
    if output.len() > MAX_OUTPUT {
        return Err(invalid("model output exceeds limit"));
    }
    let mut rest = output;
    let mut text = String::new();
    let mut calls = Vec::new();
    while let Some(start) = rest.find(OPEN) {
        let prefix = &rest[..start];
        if prefix.contains("<tool_call") || prefix.contains("</tool_call") {
            return Err(invalid("malformed tool marker"));
        }
        text.push_str(prefix);
        let body = &rest[start + OPEN.len()..];
        let end = body
            .find(CLOSE)
            .ok_or_else(|| invalid("truncated tool call"))?;
        let call: GeneratedCall = match format {
            CallFormat::Json => serde_json::from_str(body[..end].trim())
                .map_err(|_| invalid("malformed tool call JSON"))?,
            CallFormat::Coder => coder::parse_call(body[..end].trim(), tools)?,
        };
        if !tools.iter().any(|tool| tool.name == call.name) {
            return Err(invalid("model requested an unavailable tool"));
        }
        if calls.len() >= 128 {
            return Err(invalid("too many generated tool calls"));
        }
        calls.push(ToolCall {
            id: format!("call_{}", uuid::Uuid::new_v4().simple()),
            name: call.name,
            arguments: call.arguments,
        });
        rest = &body[end + CLOSE.len()..];
    }
    if rest.contains("<tool_call") || rest.contains("</tool_call") {
        return Err(invalid("truncated or unmatched tool marker"));
    }
    text.push_str(rest);
    Ok(ParsedOutput { text, calls })
}

fn invalid(reason: &'static str) -> InferError {
    InferError::InvalidToolExchange(reason)
}
