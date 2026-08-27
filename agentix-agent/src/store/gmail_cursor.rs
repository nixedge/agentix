use crate::db::AgentDb;
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailCursor {
    pub history_id: Option<String>,
    pub last_scanned_at: Option<DateTime<Utc>>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct GmailCursorStore(pub AgentDb);

impl GmailCursorStore {
    #[allow(dead_code)]
    const KEY: &'static [u8] = b"cursor";

    #[allow(dead_code)]
    pub async fn load(&self) -> anyhow::Result<GmailCursor> {
        let tree = self.0.gmail_cursor.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<GmailCursor> {
            match tree.get(Self::KEY).context("sled get")? {
                None => Ok(GmailCursor {
                    history_id: None,
                    last_scanned_at: None,
                }),
                Some(bytes) => serde_json::from_slice(&bytes).context("deserialize gmail cursor"),
            }
        })
        .await
        .context("spawn_blocking")?
    }

    #[allow(dead_code)]
    pub async fn save(&self, history_id: &str) -> anyhow::Result<()> {
        let tree = self.0.gmail_cursor.clone();
        let cursor = GmailCursor {
            history_id: Some(history_id.to_owned()),
            last_scanned_at: Some(Utc::now()),
        };
        let bytes = serde_json::to_vec(&cursor).context("serialize gmail cursor")?;
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            tree.insert(Self::KEY, bytes).context("sled insert")?;
            Ok(())
        })
        .await
        .context("spawn_blocking")?
    }
}
