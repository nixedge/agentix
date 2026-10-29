use agentix_store::{Record, Store, COLLECTIONS};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

#[derive(Deserialize)]
struct ExportRow {
    table: String,
    row: Value,
    embedding: Option<String>,
}
fn decode(line: &str) -> Result<(String, Record)> {
    let exported: ExportRow = serde_json::from_str(line)?;
    ensure!(
        COLLECTIONS.contains(&exported.table.as_str())
            || matches!(exported.table.as_str(), "repo_index" | "sync_state"),
        "unknown export table {}",
        exported.table
    );
    ensure!(exported.row.is_object(), "export row must be an object");
    let embedding = exported
        .embedding
        .map(|s| serde_json::from_str::<Vec<f32>>(&s))
        .transpose()?;
    Ok((
        exported.table,
        Record {
            row: exported.row,
            embedding,
        },
    ))
}
fn metadata_key(table: &str, row: &Value) -> Result<String> {
    Ok(if table == "repo_index" {
        row["repo_path"]
            .as_str()
            .context("missing repo_path")?
            .to_owned()
    } else {
        format!(
            "{}:{}",
            row["source_name"].as_str().context("missing source_name")?,
            row["scope_key"].as_str().context("missing scope_key")?
        )
    })
}
pub async fn import_postgres(
    input: &Path,
    destination: &Path,
    model: &str,
    dimensions: usize,
) -> Result<Value> {
    ensure!(
        !destination.exists() || std::fs::read_dir(destination)?.next().is_none(),
        "import destination must be a new or empty directory"
    );
    let store = Store::open(destination, model, dimensions).await?;
    let mut reader = BufReader::new(File::open(input)?);
    let mut expected: BTreeMap<String, usize> = BTreeMap::new();
    let mut line = String::new();
    let mut digest = Sha256::new();
    let mut number = 0;
    // Validate the complete export before the first persisted record.
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        number += 1;
        digest.update(line.as_bytes());
        if line.trim().is_empty() {
            continue;
        }
        let (table, record) = decode(&line).with_context(|| format!("export line {number}"))?;
        if COLLECTIONS.contains(&table.as_str()) {
            store.validate_record(&table, &record)?;
        } else {
            metadata_key(&table, &record.row)?;
        }
        *expected.entry(table).or_default() += 1;
    }
    ensure!(!expected.is_empty(), "empty export");
    let checksum = format!("{:x}", digest.finalize());
    let manifest_path = destination.join("import-manifest.json");
    let mut manifest = json!({"status":"incomplete","source_sha256":checksum,"model_assertion":model,"dimensions":dimensions,"expected_counts":expected});
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    reader.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut active = String::new();
    let mut batch = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        digest.update(line.as_bytes());
        if line.trim().is_empty() {
            continue;
        }
        let (table, record) = decode(&line)?;
        if !batch.is_empty() && (table != active || batch.len() >= 128) {
            store.put(&active, std::mem::take(&mut batch)).await?;
        }
        if COLLECTIONS.contains(&table.as_str()) {
            active = table;
            batch.push(record);
        } else {
            let key = metadata_key(&table, &record.row)?;
            store.set_metadata(&table, &key, record.row).await?;
        }
    }
    if !batch.is_empty() {
        store.put(&active, batch).await?;
    }
    ensure!(
        format!("{:x}", digest.finalize()) == checksum,
        "export changed while importing"
    );
    for (table, count) in &expected {
        let actual = if COLLECTIONS.contains(&table.as_str()) {
            store.count(table, "true").await?
        } else {
            store.metadata_rows(table).await?.len()
        };
        ensure!(
            actual == *count,
            "import count mismatch for {table}: expected {count}, got {actual}"
        );
    }
    manifest["status"] = json!("complete");
    std::fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn imports_all_collections_metadata_and_nullable_vectors() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let file = tmp.path().join("export.jsonl");
        let target = tmp.path().join("index");
        let rows = [
            json!({"table":"code_chunks","row":{"repo_path":"repo","file_path":"x.rs","chunk_index":0,"content":"codemarker","project":["team"]},"embedding":"[1,2,3,4]"}),
            json!({"table":"documents","row":{"repo_path":"repo","source_path":"README.md","chunk_index":0,"content":"docmarker"},"embedding":null}),
            json!({"table":"github_issues","row":{"repo":"o/r","number":1,"content":"issuemarker"},"embedding":"[1,2,3,4]"}),
            json!({"table":"github_prs","row":{"repo":"o/r","number":2,"content":"prmarker"},"embedding":"[1,2,3,4]"}),
            json!({"table":"github_issue_comments","row":{"repo":"o/r","comment_id":10,"content":"commentmarker"},"embedding":null}),
            json!({"table":"github_pr_comments","row":{"repo":"o/r","comment_type":"review_comment","comment_id":11,"content":"reviewmarker"},"embedding":null}),
            json!({"table":"repo_index","row":{"repo_path":"repo","source_kind":"local","project":["team"]},"embedding":null}),
            json!({"table":"sync_state","row":{"source_name":"github","scope_key":"o/r:issues","watermark":"2026-10-28T00:00:00Z","error_count":0},"embedding":null}),
        ];
        std::fs::write(
            &file,
            rows.iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )?;
        let report = import_postgres(&file, &target, "fixture", 4).await?;
        assert_eq!(report["status"], "complete");
        let store = Store::open(&target, "fixture", 4).await?;
        for table in COLLECTIONS {
            assert_eq!(store.count(table, "true").await?, 1);
        }
        assert_eq!(store.metadata_rows("sync_state").await?.len(), 1);
        assert!(import_postgres(&file, &target, "fixture", 4).await.is_err());
        let bad = tmp.path().join("bad.jsonl");
        std::fs::write(&bad, "{\"table\":\"unexpected\",\"row\":{}}")?;
        assert!(
            import_postgres(&bad, &tmp.path().join("bad-index"), "fixture", 4)
                .await
                .is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[tokio::test]
    async fn null_project_metadata_survives_streaming_batch_boundaries() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let input = tmp.path().join("export.jsonl");
        let destination = tmp.path().join("index");
        let lines=(0..130).map(|i|json!({"table":"code_chunks","row":{"repo_path":"repo","file_path":"lib.rs","chunk_index":i,"content":format!("marker{i}"),"project":null},"embedding":null}).to_string()).collect::<Vec<_>>().join("\n");
        std::fs::write(&input, lines)?;
        import_postgres(&input, &destination, "fixture", 4).await?;
        let store = Store::open(&destination, "fixture", 4).await?;
        let rows = store.rows("code_chunks", "true").await?;
        assert_eq!(rows.len(), 130);
        assert!(rows.iter().all(|r| r["project"].is_null()));
        assert_eq!(
            store
                .keyword("code_chunks", "marker129", "true", 10)
                .await?
                .len(),
            1
        );
        Ok(())
    }
}
