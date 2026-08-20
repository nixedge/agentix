use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::auth::GoogleAuth;

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

fn percent_encode_path(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || ".-_~@".contains(c) {
                vec![c]
            } else {
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                encoded
                    .bytes()
                    .flat_map(|b| format!("%{b:02X}").chars().collect::<Vec<char>>())
                    .collect::<Vec<char>>()
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub start: EventDateTime,
    pub end: EventDateTime,
    pub html_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDateTime {
    pub date_time: Option<DateTime<Utc>>,
    pub date: Option<String>,
    pub time_zone: Option<String>,
}

#[derive(Clone)]
pub struct CalendarClient {
    auth: std::sync::Arc<GoogleAuth>,
    calendar_id: String,
    client: reqwest::Client,
}

impl CalendarClient {
    pub fn new(auth: std::sync::Arc<GoogleAuth>, calendar_id: String) -> Self {
        Self {
            auth,
            calendar_id,
            client: reqwest::Client::new(),
        }
    }

    pub async fn list_events(
        &self,
        time_min: DateTime<Utc>,
        time_max: DateTime<Utc>,
        max_results: u32,
    ) -> anyhow::Result<Vec<CalendarEvent>> {
        let token = self.auth.token().await?;
        let cal_id = percent_encode_path(&self.calendar_id);
        let url = format!("{CALENDAR_API}/calendars/{cal_id}/events");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .query(&[
                ("timeMin", time_min.to_rfc3339()),
                ("timeMax", time_max.to_rfc3339()),
                ("maxResults", max_results.to_string()),
                ("singleEvents", "true".to_owned()),
                ("orderBy", "startTime".to_owned()),
            ])
            .send()
            .await
            .context("calendar list_events request")?;

        anyhow::ensure!(
            resp.status().is_success(),
            "calendar API returned {}",
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.context("parsing calendar response")?;
        let items = body["items"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing items in calendar response"))?;

        let events: Vec<CalendarEvent> = items
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();

        Ok(events)
    }

    pub async fn create_event(
        &self,
        summary: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        description: Option<&str>,
    ) -> anyhow::Result<CalendarEvent> {
        let token = self.auth.token().await?;
        let cal_id = percent_encode_path(&self.calendar_id);
        let url = format!("{CALENDAR_API}/calendars/{cal_id}/events");

        let body = serde_json::json!({
            "summary": summary,
            "description": description,
            "start": { "dateTime": start.to_rfc3339() },
            "end": { "dateTime": end.to_rfc3339() },
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .context("calendar create_event request")?;

        anyhow::ensure!(
            resp.status().is_success(),
            "calendar API returned {}",
            resp.status()
        );

        resp.json::<CalendarEvent>()
            .await
            .context("parsing created event")
    }
}
