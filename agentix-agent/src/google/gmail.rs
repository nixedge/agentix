use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::auth::GoogleAuth;

const GMAIL_API: &str = "https://www.googleapis.com/gmail/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailMessage {
    pub id: String,
    pub thread_id: String,
    pub snippet: Option<String>,
    pub subject: Option<String>,
    pub from: Option<String>,
}

#[derive(Clone)]
pub struct GmailClient {
    auth: Arc<GoogleAuth>,
    client: reqwest::Client,
}

impl GmailClient {
    pub fn new(auth: Arc<GoogleAuth>) -> Self {
        Self {
            auth,
            client: reqwest::Client::new(),
        }
    }

    pub async fn list_unread(&self, max_results: u32) -> anyhow::Result<Vec<GmailMessage>> {
        let token = self.auth.token().await?;
        let url = format!("{GMAIL_API}/users/me/messages");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .query(&[
                ("q", "is:unread label:inbox".to_owned()),
                ("maxResults", max_results.to_string()),
            ])
            .send()
            .await
            .context("gmail list request")?;

        anyhow::ensure!(
            resp.status().is_success(),
            "gmail API returned {}",
            resp.status()
        );

        let body: serde_json::Value = resp.json().await.context("parsing gmail list response")?;
        let message_refs = match body["messages"].as_array() {
            Some(arr) => arr.clone(),
            None => return Ok(vec![]),
        };

        let mut messages = Vec::new();
        for item in message_refs.iter().take(max_results as usize) {
            let id = match item["id"].as_str() {
                Some(id) => id.to_owned(),
                None => continue,
            };
            if let Ok(msg) = self.get_message_meta(&id).await {
                messages.push(msg);
            }
        }
        Ok(messages)
    }

    pub async fn get_message_meta(&self, message_id: &str) -> anyhow::Result<GmailMessage> {
        let token = self.auth.token().await?;
        let url = format!("{GMAIL_API}/users/me/messages/{message_id}");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .query(&[("format", "metadata"), ("metadataHeaders", "Subject,From")])
            .send()
            .await
            .context("gmail get message request")?;

        anyhow::ensure!(
            resp.status().is_success(),
            "gmail message API returned {}",
            resp.status()
        );

        let body: serde_json::Value = resp
            .json()
            .await
            .context("parsing gmail message response")?;

        let thread_id = body["threadId"].as_str().unwrap_or("").to_owned();
        let snippet = body["snippet"].as_str().map(str::to_owned);

        let headers = body["payload"]["headers"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let subject = headers
            .iter()
            .find(|h| h["name"].as_str() == Some("Subject"))
            .and_then(|h| h["value"].as_str())
            .map(str::to_owned);

        let from = headers
            .iter()
            .find(|h| h["name"].as_str() == Some("From"))
            .and_then(|h| h["value"].as_str())
            .map(str::to_owned);

        Ok(GmailMessage {
            id: message_id.to_owned(),
            thread_id,
            snippet,
            subject,
            from,
        })
    }

    pub async fn get_history_id(&self) -> anyhow::Result<String> {
        let token = self.auth.token().await?;
        let url = format!("{GMAIL_API}/users/me/profile");
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .context("gmail profile request")?;
        anyhow::ensure!(
            resp.status().is_success(),
            "gmail profile API returned {}",
            resp.status()
        );
        let body: serde_json::Value = resp.json().await.context("parsing gmail profile")?;
        body["historyId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("no historyId in gmail profile"))
    }
}
