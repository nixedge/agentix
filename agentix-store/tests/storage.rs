use agentix_store::{literal, project_filter, Record, Store};
use anyhow::{Context, Result};
use serde_json::json;
fn code(file: &str, index: i32, content: &str, project: &str) -> Record {
    Record {
        row: json!({"repo_path":"repo'one","file_path":file,"chunk_index":index,"content":content,"language":"rust","symbol_kind":"function","project":[project],"content_hash":"hash","file_mtime":1,"indexed_at":"2026-10-28T00:00:00Z"}),
        embedding: Some(vec![1.0, 0.2, 0.3, 0.4, 0.1, 0.5, 0.6, 0.7]),
    }
}
#[tokio::test]
async fn replacement_projects_reopen_and_failed_writes_preserve_data() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = Store::open(tmp.path(), "fixture", 8).await?;
    let reader = Store::open(tmp.path(), "fixture", 8).await?;
    store
        .replace_file(
            "code_chunks",
            "repo'one",
            "file.rs",
            vec![
                code("file.rs", 0, "uniquemarker first", "alpha"),
                code("file.rs", 1, "uniquemarker second", "alpha"),
            ],
        )
        .await?;
    assert_eq!(reader.count("code_chunks", "true").await?, 2);
    assert!(reader
        .keyword("code_chunks", "uniquemarker", "true", 10)
        .await?
        .iter()
        .all(|(_, score)| *score > 0.0));
    assert_eq!(
        reader
            .keyword(
                "code_chunks",
                "uniquemarker",
                &project_filter(&["alpha".into()]),
                10
            )
            .await?
            .len(),
        2
    );
    assert!(reader
        .keyword(
            "code_chunks",
            "uniquemarker",
            &project_filter(&["denied' OR true --".into()]),
            10
        )
        .await?
        .is_empty());
    store
        .replace_file(
            "code_chunks",
            "repo'one",
            "file.rs",
            vec![code("file.rs", 0, "updatedmarker", "beta")],
        )
        .await?;
    assert_eq!(reader.count("code_chunks", "true").await?, 1);
    for tag in ["alpha", "beta"] {
        assert_eq!(
            reader
                .keyword(
                    "code_chunks",
                    "updatedmarker",
                    &project_filter(&[tag.into()]),
                    10
                )
                .await?
                .len(),
            1
        );
    }
    assert!(reader
        .keyword("code_chunks", "uniquemarker", "true", 10)
        .await?
        .is_empty());
    let mut invalid = code("file.rs", 0, "badmarker", "beta");
    invalid.embedding = Some(vec![1.0; 7]);
    assert!(store
        .replace_file("code_chunks", "repo'one", "file.rs", vec![invalid])
        .await
        .is_err());
    assert_eq!(
        reader
            .keyword("code_chunks", "updatedmarker", "true", 10)
            .await?
            .len(),
        1
    );
    let reopened = Store::open(tmp.path(), "fixture", 8).await?;
    assert_eq!(reopened.count("code_chunks", "true").await?, 1);
    assert!(Store::open(tmp.path(), "wrong-model", 8).await.is_err());
    assert!(Store::open(tmp.path(), "fixture", 4).await.is_err());
    store
        .replace_file("code_chunks", "repo'one", "file.rs", vec![])
        .await?;
    assert_eq!(reader.count("code_chunks", "true").await?, 0);
    Ok(())
}
#[tokio::test]
async fn documents_github_comments_metadata_and_null_vectors() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = Store::open(tmp.path(), "fixture", 8).await?;
    let vector = vec![1.0; 8];
    for (collection, row) in [
        (
            "documents",
            json!({"repo_path":"docs","source_path":"README.md","chunk_index":0,"content":"manualmarker","doc_kind":"readme","title":"Read me","project":["docs-team"]}),
        ),
        (
            "github_issues",
            json!({"repo":"owner/repo","number":1,"state":"open","title":"Bug","content":"issuemarker"}),
        ),
        (
            "github_prs",
            json!({"repo":"owner/repo","number":2,"state":"merged","title":"Fix","content":"prmarker"}),
        ),
        (
            "github_issue_comments",
            json!({"repo":"owner/repo","comment_id":11,"issue_number":1,"content":"commentmarker"}),
        ),
        (
            "github_pr_comments",
            json!({"repo":"owner/repo","comment_id":12,"pr_number":2,"comment_type":"review_comment","content":"reviewmarker"}),
        ),
    ] {
        store
            .put(
                collection,
                vec![Record {
                    row,
                    embedding: Some(vector.clone()),
                }],
            )
            .await?;
        assert_eq!(store.count(collection, "true").await?, 1);
    }
    assert_eq!(
        store
            .hybrid(
                "documents",
                "manualmarker",
                &vector,
                "doc_kind = 'readme' AND array_has(projects, 'docs-team')",
                10
            )
            .await?
            .len(),
        1
    );
    assert!(store
        .hybrid(
            "documents",
            "manualmarker",
            &vector,
            "doc_kind = 'skill'",
            10
        )
        .await?
        .is_empty());
    assert_eq!(
        store
            .hybrid(
                "github_prs",
                "prmarker",
                &vector,
                "state = 'merged' AND repo_path LIKE 'owner/%'",
                10
            )
            .await?
            .len(),
        1
    );
    let mut issue = store.rows("github_issues", "true").await?.remove(0);
    issue["state"] = json!("closed");
    store
        .put(
            "github_issues",
            vec![Record {
                row: issue,
                embedding: Some(vector.clone()),
            }],
        )
        .await?;
    assert_eq!(store.count("github_issues", "state = 'closed'").await?, 1);
    let mut without_vector = code("legacy.rs", 0, "legacytextmarker", "legacy");
    without_vector.embedding = None;
    store.put("code_chunks", vec![without_vector]).await?;
    assert_eq!(
        store
            .keyword("code_chunks", "legacytextmarker", "true", 10)
            .await?
            .len(),
        1
    );
    assert!(store
        .hybrid("code_chunks", "no lexical match", &vector, "true", 10)
        .await?
        .is_empty());
    store
        .set_metadata(
            "sync_state",
            "github:owner/repo:issues",
            json!({"watermark":"2026-10-28T00:00:00Z","error_count":0}),
        )
        .await?;
    store
        .set_metadata(
            "repo_index",
            "docs",
            json!({"repo_path":"docs","source_kind":"local"}),
        )
        .await?;
    let reopened = Store::open(tmp.path(), "fixture", 8).await?;
    assert_eq!(reopened.metadata_rows("repo_index").await?.len(), 1);
    assert_eq!(
        reopened
            .metadata("sync_state", "github:owner/repo:issues")
            .await?
            .context("missing saved state")?["error_count"],
        0
    );
    assert!(store
        .keyword("documents", "manualmarker", "true", -1)
        .await
        .is_err());
    assert!(store
        .hybrid("documents", "manualmarker", &[f32::NAN; 8], "true", 10)
        .await
        .is_err());
    Ok(())
}
#[tokio::test]
async fn graph_maintenance_and_unindexed_updates_are_visible() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = Store::open(tmp.path(), "fixture", 8).await?;
    let mut records = (0..160)
        .map(|i| {
            let mut record = code("many.rs", i, &format!("graphmarker item{i}"), "team");
            record.embedding = Some(
                (0..8)
                    .map(|j| ((i * 31 + j * 17 + 1) % 101) as f32 + 1.0)
                    .collect(),
            );
            record
        })
        .collect::<Vec<_>>();
    let mut legacy = code("null.rs", 0, "legacy without vector", "team");
    legacy.embedding = None;
    records.push(legacy);
    store.put("code_chunks", records).await?;
    store.maintain("code_chunks").await?;
    let new = code("new.rs", 0, "freshgraphmarker", "other");
    let vector = new.embedding.clone().context("missing fixture vector")?;
    store.put("code_chunks", vec![new]).await?;
    let reader = Store::open(tmp.path(), "fixture", 8).await?;
    let rows = reader
        .hybrid(
            "code_chunks",
            "freshgraphmarker",
            &vector,
            &project_filter(&["other".into()]),
            10,
        )
        .await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0["file_path"], "new.rs");
    store
        .delete("code_chunks", &format!("file_path = {}", literal("new.rs")))
        .await?;
    assert!(reader
        .hybrid(
            "code_chunks",
            "freshgraphmarker",
            &vector,
            &project_filter(&["other".into()]),
            10
        )
        .await?
        .is_empty());
    Ok(())
}

#[test]
fn writer_process_fixture() -> Result<()> {
    let Some(path) = std::env::var_os("AGENTIX_TEST_LOCK_PATH") else {
        return Ok(());
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    fs2::FileExt::try_lock_exclusive(&file)?;
    use std::io::Write;
    println!("AGENTIX_LOCK_READY");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(())
}
#[tokio::test]
async fn competing_process_writer_is_rejected_while_reads_continue() -> Result<()> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    let tmp = tempfile::tempdir()?;
    let store = Store::open(tmp.path(), "fixture", 8).await?;
    store
        .put(
            "code_chunks",
            vec![code("one.rs", 0, "originalmarker", "team")],
        )
        .await?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", "writer_process_fixture", "--nocapture"])
        .env("AGENTIX_TEST_LOCK_PATH", tmp.path().join(".writer.lock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let input = child.stdin.take().context("missing child input")?;
    let mut output = std::io::BufReader::new(child.stdout.take().context("missing child output")?);
    loop {
        let mut line = String::new();
        anyhow::ensure!(
            output.read_line(&mut line)? > 0,
            "writer fixture exited early"
        );
        if line.contains("AGENTIX_LOCK_READY") {
            break;
        }
    }
    assert_eq!(store.count("code_chunks", "true").await?, 1);
    assert!(store
        .put("code_chunks", vec![code("two.rs", 0, "newmarker", "team")])
        .await
        .is_err());
    drop(input);
    assert!(child.wait()?.success());
    store
        .put("code_chunks", vec![code("two.rs", 0, "newmarker", "team")])
        .await?;
    assert_eq!(store.count("code_chunks", "true").await?, 2);
    Ok(())
}
