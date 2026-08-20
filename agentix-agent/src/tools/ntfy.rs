use anyhow::Context;

#[derive(Clone)]
pub struct NtfyClient {
    url: String,
    topic: String,
    client: reqwest::Client,
}

impl NtfyClient {
    pub fn new(url: String, topic: String) -> Self {
        Self {
            url,
            topic,
            client: reqwest::Client::new(),
        }
    }

    pub async fn send(&self, title: &str, body: &str, priority: &str) -> anyhow::Result<()> {
        let endpoint = format!("{}/{}", self.url.trim_end_matches('/'), self.topic);
        let resp = self
            .client
            .post(&endpoint)
            .header("Title", title)
            .header("Priority", priority)
            .header("Tags", "agentix")
            .body(body.to_owned())
            .send()
            .await
            .context("ntfy POST failed")?;

        anyhow::ensure!(
            resp.status().is_success(),
            "ntfy returned non-2xx status: {}",
            resp.status()
        );
        Ok(())
    }
}
