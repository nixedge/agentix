use agentix_store::{
    transport::{Operation, Request, PROTOCOL},
    Record, Store,
};
use anyhow::Result;
use serde_json::json;
#[path = "support/http.rs"]
mod http;
use http::{StorageServer, TOKEN};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_remote_clients_share_reads_writes_and_atomic_metadata() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let local = Store::open(dir.path(), "fixture", 4).await?;
    let server = StorageServer::start(local.clone(), Some(TOKEN.into()))?;
    let a = Store::connect(&server.url, Some(TOKEN.into()), "fixture", 4).await?;
    let b = Store::connect(&server.url, Some(TOKEN.into()), "fixture", 4).await?;
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..12 {
        let client = if i % 2 == 0 { a.clone() } else { b.clone() };
        tasks.spawn(async move {
            client.replace_file("code_chunks", "repo", &format!("{i}.rs"), vec![Record {
                row: json!({"repo_path":"repo","file_path":format!("{i}.rs"),"chunk_index":0,"content":"sharedneedle implementation","project":[format!("team{i}")]}), embedding: Some(vec![1., 0.2, 0.3, 0.4]),
            }]).await?;
            client.update_metadata("repo_index", "shared", |old| {
                let mut value = old.unwrap_or_else(|| json!({"count":0}));
                value["count"] = json!(value["count"].as_u64().unwrap_or(0) + 1);
                Ok(value)
            }).await?;
            assert!(!client.keyword("code_chunks", "sharedneedle", "true", 10).await?.is_empty());
            anyhow::Ok(())
        });
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    assert_eq!(b.count("code_chunks", "true").await?, 12);
    assert_eq!(
        local
            .metadata("repo_index", "shared")
            .await?
            .map(|v| v["count"].clone()),
        Some(json!(12))
    );
    assert_eq!(
        a.hybrid(
            "code_chunks",
            "sharedneedle",
            &[1., 0.2, 0.3, 0.4],
            "array_has(projects, 'team1')",
            10
        )
        .await?
        .len(),
        1
    );
    a.retag_file("code_chunks", "repo", "1.rs", Some("extra"), Some(4))
        .await?;
    b.tag_repository("repo", Some("common")).await?;
    assert_eq!(
        a.count("code_chunks", "array_has(projects, 'common')")
            .await?,
        12
    );
    assert_eq!(
        a.rows("code_chunks", "file_path = '1.rs'").await?[0]["file_mtime"],
        4
    );
    b.replace_file("code_chunks", "repo", "1.rs", vec![])
        .await?;
    assert_eq!(a.count("code_chunks", "true").await?, 11);
    assert_eq!(b.delete("code_chunks", "true").await?, 11);
    assert_eq!(a.count("code_chunks", "true").await?, 0);
    Ok(())
}

#[tokio::test]
async fn auth_protocol_payload_and_invalid_records_are_rejected_without_mutation() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let local = Store::open(dir.path(), "fixture", 4).await?;
    let server = StorageServer::start(local.clone(), Some(TOKEN.into()))?;
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(format!("{}/v1/info", server.url))
            .send()
            .await?
            .status(),
        401
    );
    assert_eq!(
        http.post(format!("{}/v1/store", server.url))
            .body("invalid JSON")
            .send()
            .await?
            .status(),
        401
    );
    assert!(
        Store::connect(&server.url, Some("wrong".into()), "fixture", 4)
            .await
            .is_err()
    );
    assert!(
        Store::connect(&server.url, Some(TOKEN.into()), "different", 4)
            .await
            .is_err()
    );
    assert!(
        Store::connect(&server.url, Some(TOKEN.into()), "fixture", 8)
            .await
            .is_err()
    );
    let response = http
        .post(format!("{}/v1/store", server.url))
        .bearer_auth(TOKEN)
        .json(&Request {
            protocol: PROTOCOL + 1,
            model: "fixture".into(),
            dimensions: 4,
            operation: Operation::Count {
                name: "code_chunks".into(),
                filter: "true".into(),
            },
        })
        .send()
        .await?;
    assert_eq!(response.status(), 409);
    let client = Store::connect(&server.url, Some(TOKEN.into()), "fixture", 4).await?;
    assert!(client.put("code_chunks", vec![Record { row:json!({"repo_path":"repo","file_path":"a.rs","chunk_index":0,"content":"invalid"}), embedding:Some(vec![1., 2.]) }]).await.is_err());
    assert!(client
        .keyword("code_chunks", "x", "true", 101)
        .await
        .is_err());
    assert_eq!(local.count("code_chunks", "true").await?, 0);
    let oversized = http
        .post(format!("{}/v1/store", server.url))
        .bearer_auth(TOKEN)
        .header("content-type", "application/json")
        .body(vec![b' '; 32 * 1024 * 1024 + 1])
        .send()
        .await?;
    assert_eq!(oversized.status(), 413);
    Ok(())
}

#[tokio::test]
async fn lan_clients_read_and_write_without_credentials() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let local = Store::open(dir.path(), "fixture", 4).await?;
    let server = StorageServer::start(local.clone(), None)?;
    let a = Store::connect(&server.url, None, "fixture", 4).await?;
    let b = Store::connect(&server.url, None, "fixture", 4).await?;
    a.put("code_chunks", vec![Record { row: json!({"repo_path":"lan","file_path":"lib.rs","chunk_index":0,"content":"lansharedneedle"}), embedding:Some(vec![1.,0.2,0.3,0.4]) }]).await?;
    assert_eq!(
        b.keyword("code_chunks", "lansharedneedle", "true", 10)
            .await?
            .len(),
        1
    );
    b.update_metadata("repo_index", "lan", |_| Ok(json!({"project":["lan"]})))
        .await?;
    assert_eq!(a.metadata_rows("repo_index").await?.len(), 1);
    assert_eq!(b.delete("code_chunks", "true").await?, 1);
    assert_eq!(local.count("code_chunks", "true").await?, 0);
    Ok(())
}
