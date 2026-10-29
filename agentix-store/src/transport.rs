//! Trusted shared-index HTTP contract and client. This module never binds a listener.
use anyhow::{ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use crate::Record;

pub const PROTOCOL: u32 = 1;
#[derive(Debug, Serialize, Deserialize)]
pub struct Info {
    pub protocol: u32,
    pub schema: String,
    pub model: String,
    pub dimensions: usize,
}
#[derive(Serialize, Deserialize)]
pub struct Request {
    pub protocol: u32,
    pub model: String,
    pub dimensions: usize,
    pub operation: Operation,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Repositories {
        projects: Vec<String>,
    },
    Rows {
        name: String,
        filter: String,
    },
    Count {
        name: String,
        filter: String,
    },
    Put {
        name: String,
        records: Vec<Record>,
    },
    ReplaceFile {
        name: String,
        repo: String,
        file: String,
        records: Vec<Record>,
    },
    RetagFile {
        name: String,
        repo: String,
        file: String,
        project: Option<String>,
        mtime: Option<i64>,
    },
    TagRepository {
        repo: String,
        project: Option<String>,
    },
    Delete {
        name: String,
        filter: String,
    },
    Maintain {
        name: String,
    },
    MaintainIfNeeded {
        name: String,
    },
    Keyword {
        name: String,
        text: String,
        filter: String,
        limit: i32,
    },
    Hybrid {
        name: String,
        text: String,
        vector: Vec<f32>,
        filter: String,
        limit: i32,
    },
    Metadata {
        namespace: String,
        key: String,
    },
    MetadataRows {
        namespace: String,
    },
    SetMetadata {
        namespace: String,
        key: String,
        value: Value,
    },
    CompareMetadata {
        namespace: String,
        key: String,
        expected: Option<Value>,
        value: Value,
    },
}
#[derive(Serialize, Deserialize)]
pub struct Response {
    pub value: Value,
}
#[derive(Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

/// Read a runtime credential without ever including its contents in diagnostics.
pub fn credential() -> Result<Option<String>> {
    let inline = std::env::var_os("AGENTIX_STORE_TOKEN")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("storage token must be UTF-8"))
        })
        .transpose()?;
    let file = std::env::var_os("AGENTIX_STORE_TOKEN_FILE");
    let token = match (inline, file) {
        (Some(token), None) => token,
        (None, Some(path)) => {
            std::fs::read_to_string(path).context("cannot read storage credential file")?
        }
        (None, None) => return Ok(None),
        _ => anyhow::bail!("set only one of AGENTIX_STORE_TOKEN and AGENTIX_STORE_TOKEN_FILE"),
    };
    let token = token.trim().to_owned();
    ensure!(
        token.len() >= 32 && token.bytes().all(|b| b.is_ascii_graphic()),
        "storage credential must contain at least 32 printable ASCII characters without spaces"
    );
    Ok(Some(token))
}

#[derive(Clone)]
pub(crate) struct Remote {
    http: reqwest::Client,
    url: String,
    token: Option<String>,
    model: String,
    dimensions: usize,
}
impl Remote {
    pub async fn connect(
        url: &str,
        token: Option<String>,
        model: &str,
        dimensions: usize,
    ) -> Result<Self> {
        let parsed = reqwest::Url::parse(url).context("invalid AGENTIX_STORE_URL")?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.query().is_none()
                && parsed.fragment().is_none(),
            "storage URL must be HTTP(S) without credentials, query or fragment"
        );
        let remote = Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(300))
                .build()?,
            url: url.trim_end_matches('/').to_owned(),
            token,
            model: model.into(),
            dimensions,
        };
        let response = remote
            .authorize(remote.http.get(format!("{}/v1/info", remote.url)))
            .send()
            .await
            .context("storage server connection failed")?;
        let info: Info = Self::decode(response).await?;
        ensure!(
            info.protocol == PROTOCOL && info.schema == crate::VERSION,
            "storage protocol/schema mismatch"
        );
        ensure!(
            info.model == model && info.dimensions == dimensions,
            "storage embedding model/dimensions mismatch"
        );
        Ok(remote)
    }
    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
    async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        if !response.status().is_success() {
            let status = response.status();
            let error = response
                .json::<ApiError>()
                .await
                .ok()
                .map(|e| e.error)
                .unwrap_or_else(|| "request rejected".into());
            anyhow::bail!("storage API {status}: {error}");
        }
        response
            .json()
            .await
            .context("invalid storage API response")
    }
    pub async fn call<T: DeserializeOwned>(&self, operation: Operation) -> Result<T> {
        let request = Request {
            protocol: PROTOCOL,
            model: self.model.clone(),
            dimensions: self.dimensions,
            operation,
        };
        let response = self
            .authorize(self.http.post(format!("{}/v1/store", self.url)))
            .json(&request)
            .send()
            .await
            .context(
                "storage request failed; a mutation may have committed, inspect before retrying",
            )?;
        let response: Response = Self::decode(response).await?;
        serde_json::from_value(response.value).context("invalid storage operation result")
    }
}
