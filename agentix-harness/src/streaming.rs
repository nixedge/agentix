use crate::{
    client::{parse_response, ChatResponse},
    event::AgentEvent,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
pub struct SseCompletion {
    buffer: Vec<u8>,
    total: usize,
    text: String,
    calls: BTreeMap<usize, Value>,
    finish: Option<String>,
    done: bool,
}
impl SseCompletion {
    pub fn push(
        &mut self,
        bytes: &[u8],
        events: Option<&UnboundedSender<AgentEvent>>,
    ) -> Result<()> {
        self.total = self.total.saturating_add(bytes.len());
        if self.total > 4 * 1024 * 1024 {
            bail!("completion stream exceeds 4 MiB");
        }
        self.buffer.extend_from_slice(bytes);
        loop {
            let delimiter = self
                .buffer
                .windows(2)
                .position(|b| b == b"\n\n")
                .map(|i| (i, 2))
                .into_iter()
                .chain(
                    self.buffer
                        .windows(4)
                        .position(|b| b == b"\r\n\r\n")
                        .map(|i| (i, 4)),
                )
                .min_by_key(|pair| pair.0);
            let Some((end, width)) = delimiter else { break };
            let frame = String::from_utf8(self.buffer.drain(..end + width).collect())?;
            let data = frame
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .map(|s| s.strip_prefix(' ').unwrap_or(s))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            if self.done {
                bail!("data after stream completion");
            }
            if data == "[DONE]" {
                if self.finish.is_none() {
                    bail!("stream ended without finish reason");
                }
                self.done = true;
                continue;
            }
            let value: Value = serde_json::from_str(&data)?;
            if let Some(error) = value.get("error") {
                let detail = error
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| error.to_string());
                let detail: String = detail.chars().take(16384).collect();
                return Err(anyhow::anyhow!(detail)).context("model stream reported an error");
            }
            if self.finish.is_some() {
                bail!("data after finish reason");
            }
            let choices = value["choices"]
                .as_array()
                .context("stream missing choices")?;
            if choices.len() != 1 || choices[0]["index"] != 0 {
                bail!("unsupported choice count/index");
            }
            let choice = &choices[0];
            let delta = &choice["delta"];
            if let Some(text) = delta.get("content").filter(|v| !v.is_null()) {
                let text = text.as_str().context("invalid text delta")?;
                self.text.push_str(text);
                if let Some(tx) = events {
                    let _ = tx.send(AgentEvent::TextDelta { text: text.into() });
                }
            }
            if let Some(calls) = delta.get("tool_calls") {
                for fragment in calls.as_array().context("invalid call delta")? {
                    let index = fragment["index"].as_u64().context("missing call index")? as usize;
                    if index >= 128 {
                        bail!("too many tool calls");
                    }
                    let call = self.calls.entry(index).or_insert_with(
                        || json!({"id":"","type":"function","function":{"name":"","arguments":""}}),
                    );
                    if let Some(kind) = fragment.get("type") {
                        if kind != "function" {
                            bail!("unsupported tool type");
                        }
                    }
                    if let Some(id) = fragment.get("id") {
                        let id = id.as_str().context("invalid call ID")?;
                        if call["id"] != "" && call["id"] != id {
                            bail!("call ID changed during stream");
                        }
                        call["id"] = json!(id);
                    }
                    for field in ["name", "arguments"] {
                        if let Some(part) = fragment.get("function").and_then(|f| f.get(field)) {
                            let part = part.as_str().context("invalid function fragment")?;
                            let previous = call["function"][field]
                                .as_str()
                                .context("invalid accumulator")?;
                            call["function"][field] = json!(format!("{previous}{part}"));
                        }
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").filter(|v| !v.is_null()) {
                self.finish = Some(reason.as_str().context("invalid finish reason")?.into());
            }
        }
        Ok(())
    }
    pub fn finish(self) -> Result<ChatResponse> {
        if !self.done || self.buffer.iter().any(|b| !b.is_ascii_whitespace()) {
            bail!("truncated completion stream");
        }
        if self
            .calls
            .keys()
            .enumerate()
            .any(|(expected, index)| expected != *index)
        {
            bail!("noncontiguous call indexes");
        }
        parse_response(
            &json!({"choices":[{"finish_reason":self.finish,"message":{"content":self.text,"tool_calls":self.calls.into_values().collect::<Vec<_>>()}}]}),
        )
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;
    #[test]
    fn preserves_bounded_server_error_as_cause() {
        let mut stream = SseCompletion::default();
        let error = stream.push(b"data: {\"error\":{\"message\":\"context window exceeded\",\"code\":\"context_length\"}}\n\n", None).err();
        assert!(error.is_some());
        if let Some(error) = error {
            assert_eq!(error.to_string(), "model stream reported an error");
            assert!(format!("{error:#}").contains("context window exceeded"));
            assert!(format!("{error:#}").contains("context_length"));
        }
        let payload = format!("data: {}\n\n", json!({"error":"界".repeat(20000)}));
        let error = SseCompletion::default()
            .push(payload.as_bytes(), None)
            .err();
        assert!(error.is_some_and(|error| format!("{error:#}").chars().count() < 16500));
    }
}
