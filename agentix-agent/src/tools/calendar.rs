use agentix_harness::Tool;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::google::calendar::CalendarClient;

pub struct CalendarListEventsTool(pub Arc<CalendarClient>);
pub struct CalendarCreateEventTool(pub Arc<CalendarClient>);

#[async_trait]
impl Tool for CalendarListEventsTool {
    fn name(&self) -> &str {
        "calendar_list_events"
    }

    fn description(&self) -> &str {
        "List upcoming Google Calendar events within a time range."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "hours_ahead": {
                    "type": "integer",
                    "description": "How many hours ahead to look (default 24)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of events to return (default 10)"
                }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let hours = args["hours_ahead"].as_i64().unwrap_or(24);
        let max = args["max_results"].as_u64().unwrap_or(10) as u32;
        let now = Utc::now();
        let end = now + chrono::Duration::hours(hours);
        let events = self.0.list_events(now, end, max).await?;
        Ok(serde_json::to_string(&events)?)
    }
}

#[async_trait]
impl Tool for CalendarCreateEventTool {
    fn name(&self) -> &str {
        "calendar_create_event"
    }

    fn description(&self) -> &str {
        "Create a new Google Calendar event."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "required": ["summary", "start", "end"],
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Event title"
                },
                "start": {
                    "type": "string",
                    "description": "Start time as ISO 8601"
                },
                "end": {
                    "type": "string",
                    "description": "End time as ISO 8601"
                },
                "description": {
                    "type": "string",
                    "description": "Optional description"
                }
            }
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let summary = args["summary"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("summary is required"))?;
        let start_str = args["start"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("start is required"))?;
        let end_str = args["end"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("end is required"))?;
        let description = args["description"].as_str();

        let start = chrono::DateTime::parse_from_rfc3339(start_str)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map_err(|e| anyhow::anyhow!("invalid start: {e}"))?;
        let end = chrono::DateTime::parse_from_rfc3339(end_str)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map_err(|e| anyhow::anyhow!("invalid end: {e}"))?;

        let event = self
            .0
            .create_event(summary, start, end, description)
            .await?;
        Ok(serde_json::to_string(&event)?)
    }
}
