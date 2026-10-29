use agentix_store::{Store, COLLECTIONS};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_DIRTY_AGE: u64 = 300;
const LEASE_SECONDS: u64 = 900;
static IDS: AtomicU64 = AtomicU64::new(0);

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn operation_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        IDS.fetch_add(1, Ordering::Relaxed)
    )
}

/// Every caller records its changes, including while another client optimizes.
pub async fn maintain(store: &Store, name: &str, dirty: bool, force: bool) -> Result<Value> {
    ensure!(COLLECTIONS.contains(&name), "unknown collection {name}");
    let rows = store.count(name, "true").await?;
    let owner = operation_id();
    let time = now();
    store
        .update_metadata("maintenance", name, |previous| {
            let mut state =
                previous.unwrap_or_else(|| json!({"generation":0,"indexed_generation":0}));
            state["collection"] = json!(name);
            if dirty {
                state["generation"] = json!(state["generation"].as_u64().unwrap_or(0) + 1);
            }
            state["observed_rows"] = json!(rows);
            let indexed = state["indexed_rows"].as_u64();
            let threshold = (indexed.unwrap_or(0) / 10).clamp(64, 512);
            let changed = indexed.is_none_or(|old| old.abs_diff(rows as u64) >= threshold);
            let pending = state["generation"] != state["indexed_generation"];
            let old =
                time.saturating_sub(state["optimized_at"].as_u64().unwrap_or(0)) >= MAX_DIRTY_AGE;
            if (force || changed || (pending && old))
                && state["lease_until"].as_u64().unwrap_or(0) <= time
            {
                state["owner"] = json!(owner);
                state["lease_until"] = json!(time + LEASE_SECONDS);
                state["claimed_generation"] = state["generation"].clone();
            }
            Ok(state)
        })
        .await?;
    let state = store
        .metadata("maintenance", name)
        .await?
        .unwrap_or_default();
    if state["owner"].as_str() != Some(owner.as_str()) {
        let status = if state["lease_until"].as_u64().unwrap_or(0) > time {
            "busy"
        } else if state["generation"] != state["indexed_generation"] {
            "deferred"
        } else {
            "current"
        };
        return Ok(json!({"collection":name,"status":status,"rows":rows,"state":state}));
    }
    let generation = state["claimed_generation"].clone();
    let result = store.maintain(name).await;
    let error = result.as_ref().err().map(|e| format!("{e:#}"));
    store
        .update_metadata("maintenance", name, |previous| {
            let mut state = previous.unwrap_or_default();
            if state["owner"].as_str() == Some(owner.as_str()) {
                state["lease_until"] = json!(0);
                state["last_error"] = json!(error);
                if error.is_none() {
                    state["indexed_rows"] = json!(rows);
                    state["indexed_generation"] = generation.clone();
                    state["optimized_at"] = json!(now());
                }
            }
            Ok(state)
        })
        .await?;
    result?;
    Ok(json!({"collection":name,"status":"optimized","rows":rows}))
}

pub async fn after_ingest(store: &Store, names: &[&str]) -> Result<Vec<Value>> {
    let mut reports = Vec::new();
    for name in names {
        reports.push(maintain(store, name, true, false).await?);
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dirty_work_is_batched_and_a_shared_lease_defers_competing_clients() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), "fixture", 4).await?;
        assert_eq!(
            maintain(&store, "code_chunks", true, false).await?["status"],
            "optimized"
        );
        assert_eq!(
            maintain(&store, "code_chunks", true, false).await?["status"],
            "deferred"
        );
        let before = store
            .metadata("maintenance", "code_chunks")
            .await?
            .unwrap_or_default();
        store
            .update_metadata("maintenance", "code_chunks", |previous| {
                let mut state = previous.unwrap_or_default();
                state["owner"] = json!("other-client");
                state["lease_until"] = json!(now() + 60);
                Ok(state)
            })
            .await?;
        assert_eq!(
            maintain(&store, "code_chunks", true, true).await?["status"],
            "busy"
        );
        let busy = store
            .metadata("maintenance", "code_chunks")
            .await?
            .unwrap_or_default();
        assert!(busy["generation"].as_u64() > before["generation"].as_u64());
        assert_eq!(busy["indexed_generation"], before["indexed_generation"]);
        assert_eq!(busy["claimed_generation"], before["claimed_generation"]);
        assert_eq!(busy["collection"], "code_chunks");
        store
            .update_metadata("maintenance", "code_chunks", |previous| {
                let mut state = previous.unwrap_or_default();
                state["lease_until"] = json!(0);
                state["optimized_at"] = json!(0);
                Ok(state)
            })
            .await?;
        assert_eq!(
            maintain(&store, "code_chunks", false, false).await?["status"],
            "optimized"
        );
        let done = store
            .metadata("maintenance", "code_chunks")
            .await?
            .unwrap_or_default();
        assert_eq!(done["generation"], done["indexed_generation"]);
        assert!(maintain(&store, "unknown", true, false).await.is_err());
        Ok(())
    }
}
