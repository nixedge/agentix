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

pub struct CalendarClient {
    auth: Option<std::sync::Arc<GoogleAuth>>,
    static_token: Option<String>,
    calendar_id: String,
    client: reqwest::Client,
    base_url: String,
}

impl CalendarClient {
    pub fn new(auth: std::sync::Arc<GoogleAuth>, calendar_id: String) -> Self {
        Self {
            auth: Some(auth),
            static_token: None,
            calendar_id,
            client: reqwest::Client::new(),
            base_url: CALENDAR_API.to_owned(),
        }
    }

    #[cfg(test)]
    pub fn new_for_test(base_url: String, token: String, calendar_id: String) -> Self {
        Self {
            auth: None,
            static_token: Some(token),
            calendar_id,
            client: reqwest::Client::new(),
            base_url,
        }
    }

    async fn get_token(&self) -> anyhow::Result<String> {
        if let Some(ref t) = self.static_token {
            return Ok(t.clone());
        }
        self.auth
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("GoogleAuth is not configured"))?
            .token()
            .await
    }

    pub async fn list_events(
        &self,
        time_min: DateTime<Utc>,
        time_max: DateTime<Utc>,
        max_results: u32,
    ) -> anyhow::Result<Vec<CalendarEvent>> {
        let token = self.get_token().await?;
        let cal_id = percent_encode_path(&self.calendar_id);
        let url = format!("{}/calendars/{cal_id}/events", self.base_url);
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
        let token = self.get_token().await?;
        let cal_id = percent_encode_path(&self.calendar_id);
        let url = format!("{}/calendars/{cal_id}/events", self.base_url);

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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_event_json(id: &str, summary: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "summary": summary,
            "start": { "dateTime": "2026-08-20T10:00:00Z" },
            "end": { "dateTime": "2026-08-20T11:00:00Z" }
        })
    }

    #[tokio::test]
    async fn list_events_parses_response() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path_regex(r"/calendars/.+/events"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "kind": "calendar#events",
                "items": [
                    make_event_json("evt1", "Team standup"),
                    make_event_json("evt2", "Lunch"),
                ]
            })))
            .mount(&server)
            .await;

        let client = CalendarClient::new_for_test(
            server.uri(),
            "test_token".into(),
            "primary".into(),
        );

        let time_min = Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).unwrap();
        let time_max = Utc.with_ymd_and_hms(2026, 8, 21, 0, 0, 0).unwrap();
        let events = client
            .list_events(time_min, time_max, 10)
            .await
            .expect("list_events");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "evt1");
        assert_eq!(events[0].summary.as_deref(), Some("Team standup"));
        assert_eq!(events[1].id, "evt2");
    }

    #[tokio::test]
    async fn create_event_sends_correct_body() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path_regex(r"/calendars/.+/events"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(make_event_json("new_evt", "Doctor")),
            )
            .mount(&server)
            .await;

        let client = CalendarClient::new_for_test(
            server.uri(),
            "test_token".into(),
            "primary".into(),
        );

        let start = Utc.with_ymd_and_hms(2026, 8, 25, 14, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 8, 25, 15, 0, 0).unwrap();
        let event = client
            .create_event("Doctor", start, end, Some("Annual checkup"))
            .await
            .expect("create_event");

        assert_eq!(event.id, "new_evt");
        assert_eq!(event.summary.as_deref(), Some("Doctor"));

        let reqs = server.received_requests().await.unwrap_or_default();
        assert_eq!(reqs.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap_or_default();
        assert_eq!(body["summary"].as_str(), Some("Doctor"));
        assert_eq!(body["description"].as_str(), Some("Annual checkup"));
    }
}
