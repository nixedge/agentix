use agentix_store::{project_filter, Store};
use anyhow::{ensure, Context, Result};
use std::{path::Path, process::Command, sync::atomic::Ordering};
#[path = "../../agentix-store/tests/support/http.rs"]
mod http;
#[path = "../../agentix-store/tests/support/mod.rs"]
mod support;
use support::Embeddings;
fn run(index: &Path, server: &Embeddings, remote: Option<&str>, args: &[&str]) -> Result<()> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ingest"));
    command
        .args(args)
        .env("AGENTIX_LANCE_PATH", index)
        .env("EMBED_MODEL", "fixture")
        .env("EMBED_DIMENSIONS", "4")
        .env("OLLAMA_HOST", &server.url)
        .env_remove("PG_DSN")
        .env_remove("AGENTIX_STORE_TOKEN_FILE");
    if let Some(url) = remote {
        command
            .env_remove("AGENTIX_LANCE_PATH")
            .env("AGENTIX_STORE_URL", url)
            .env("AGENTIX_STORE_TOKEN", http::TOKEN);
    } else {
        command
            .env_remove("AGENTIX_STORE_URL")
            .env_remove("AGENTIX_STORE_TOKEN");
    }
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "ingest failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
#[tokio::test]
async fn code_docs_retag_shorter_deleted_files_and_prune_survive_process_reopen() -> Result<()> {
    exercise(false).await
}
#[tokio::test]
async fn remote_code_docs_ingestion_retag_and_prune_use_shared_store() -> Result<()> {
    exercise(true).await
}

#[tokio::test]
async fn android_sources_and_configuration_are_searchable_locally() -> Result<()> {
    exercise_android(false).await
}

#[tokio::test]
async fn android_sources_and_configuration_are_searchable_remotely() -> Result<()> {
    exercise_android(true).await
}

async fn exercise_android(remote: bool) -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("android");
    std::fs::create_dir_all(repo.join(".git"))?;
    std::fs::write(repo.join(".gitignore"), "ignored/\nlocal.properties\n")?;
    let fixtures = [
        ("app/src/main/kotlin/InboxScreen.kt", "kotlin", "composeinboxmarker", "@Composable\nfun InboxScreen() {\n    Text(\"composeinboxmarker\")\n}\n"),
        ("app/src/main/kotlin/Reminder.kt", "kotlin", "remindermethodmarker", "class Reminder {\n    fun dismiss() {\n        println(\"remindermethodmarker\")\n    }\n}\n"),
        ("app/src/main/java/Legacy.java", "java", "legacyjavamarker", "class Legacy {\n    void open() {\n        System.out.println(\"legacyjavamarker\");\n    }\n}\n"),
        ("build.gradle.kts", "kotlin", "kotlingradlemarker", "plugins {\n    id(\"kotlingradlemarker\")\n}\n"),
        ("settings.gradle", "groovy", "groovygradlemarker", "rootProject.name = 'groovygradlemarker'\n"),
        ("gradle.properties", "properties", "androidpropertymarker", "androidpropertymarker=true\n"),
        ("app/src/main/AndroidManifest.xml", "xml", "manifestactivitymarker", "<manifest>\n    <application android:label=\"manifestactivitymarker\" />\n</manifest>\n"),
        ("app/src/main/res/values/strings.xml", "xml", "androidresourcemarker", "<resources>\n    <string name=\"app_name\">androidresourcemarker</string>\n</resources>\n"),
    ];
    for (file, _, _, content) in fixtures {
        let path = repo.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    for file in [
        "app/build/generated/Generated.kt",
        ".gradle/cache/Cache.kt",
        ".kotlin/sessions/Session.kt",
        ".idea/workspace.xml",
        "ignored/Private.kt",
        "local.properties",
    ] {
        let path = repo.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, "generatednoisemarker")?;
    }
    let index = tmp.path().join("index");
    let local = Store::open(&index, "fixture", 4).await?;
    let storage = if remote {
        Some(http::StorageServer::start(local, Some(http::TOKEN.into()))?)
    } else {
        None
    };
    let remote = storage.as_ref().map(|s| s.url.as_str());
    let server = Embeddings::start()?;
    let path = repo.to_str().context("UTF8 Android fixture path")?;
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "android"],
    )?;
    let reader = Store::open(&index, "fixture", 4).await?;
    let repo_filter = agentix_store::repo_filter(path);
    let project = project_filter(&["android".into()]);
    for (file, language, marker, source) in fixtures {
        let filter = format!("{repo_filter} AND ({project}) AND language = '{language}'");
        let hits = reader.keyword("code_chunks", marker, &filter, 10).await?;
        ensure!(!hits.is_empty(), "missing Android search result for {file}");
        for (row, _) in hits {
            assert_eq!(row["file_path"], file);
            let start = row["start_line"].as_u64().context("chunk start")? as usize;
            let end = row["end_line"].as_u64().context("chunk end")? as usize;
            ensure!(start > 0 && end >= start && end <= source.lines().count());
            let expected = source
                .lines()
                .skip(start - 1)
                .take(end - start + 1)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                row["content"].as_str().context("chunk content")?.trim_end(),
                expected
            );
        }
    }
    let functions = reader.keyword("code_chunks", "composeinboxmarker", &format!("{repo_filter} AND ({project}) AND language = 'kotlin' AND symbol_kind = 'function'"), 10).await?;
    assert_eq!(functions.len(), 1);
    ensure!(functions[0].0["content"]
        .as_str()
        .context("Compose function")?
        .contains("@Composable"));
    let methods = reader
        .keyword(
            "code_chunks",
            "remindermethodmarker",
            &format!("{repo_filter} AND language = 'kotlin' AND symbol_kind = 'method'"),
            10,
        )
        .await?;
    assert_eq!(methods.len(), 1);
    assert!(reader
        .keyword(
            "code_chunks",
            "composeinboxmarker",
            &project_filter(&["other".into()]),
            10
        )
        .await?
        .is_empty());
    let rows = reader.rows("code_chunks", &repo_filter).await?;
    ensure!(
        rows.iter().all(|row| fixtures
            .iter()
            .any(|(file, _, _, _)| row["file_path"] == *file)),
        "unexpected Android generated/cache file indexed"
    );
    let calls = server.requests.load(Ordering::Relaxed);
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "android"],
    )?;
    assert_eq!(
        server.requests.load(Ordering::Relaxed),
        calls,
        "unchanged Android files re-embedded"
    );
    std::fs::remove_file(repo.join("app/src/main/kotlin/InboxScreen.kt"))?;
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "android"],
    )?;
    assert!(reader
        .keyword("code_chunks", "composeinboxmarker", &repo_filter, 10)
        .await?
        .is_empty());
    Ok(())
}

#[tokio::test]
async fn git_ingestion_resumes_partial_rows_and_requires_requested_docs_completion() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("git-source");
    std::fs::create_dir(&repo)?;
    std::fs::write(
        repo.join("lib.rs"),
        "pub fn recoveredfirst() -> u8 { 1 }\npub fn recoveredsecond() -> u8 { 2 }\n",
    )?;
    std::fs::write(repo.join("README.md"), "# Recovery\n\nThis recoverymanualmarker documents both functions in this disposable Git fixture.\n")?;
    for args in [
        vec!["init", "--initial-branch=master"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "fixture",
        ],
    ] {
        let output = Command::new("git").current_dir(&repo).args(args).output()?;
        ensure!(
            output.status.success(),
            "fixture git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let revision = Command::new("git")
        .current_dir(&repo)
        .args(["rev-parse", "HEAD"])
        .output()?;
    ensure!(revision.status.success(), "fixture rev-parse failed");
    let revision = String::from_utf8(revision.stdout)?;
    let url = repo.to_str().context("fixture path")?;
    let key = format!("git::{url}@{}", &revision.trim()[..12]);
    let index = tmp.path().join("index");
    let store = Store::open(&index, "fixture", 4).await?;
    // Reproduce an interrupted revision with data but no successful completion.
    store.put("code_chunks", vec![agentix_store::Record {
        row: serde_json::json!({"repo_path":key,"file_path":"lib.rs","chunk_index":0,"content":"partialoldmarker"}),
        embedding: Some(vec![1.0, 0.2, 0.3, 0.4]),
    }]).await?;
    let server = Embeddings::start()?;
    run(
        &index,
        &server,
        None,
        &["git", url, "--branch", "master", "--no-docs"],
    )?;
    assert_eq!(store.count("code_chunks", "true").await?, 2);
    assert_eq!(store.count("documents", "true").await?, 0);
    let state = store
        .metadata("git_ingest", &key)
        .await?
        .context("completion state")?;
    assert_eq!(state["complete"], true);
    assert_eq!(state["docs_indexed"], false);
    run(&index, &server, None, &["git", url, "--branch", "master"])?;
    assert_eq!(store.count("documents", "true").await?, 1);
    let state = store
        .metadata("git_ingest", &key)
        .await?
        .context("completion state")?;
    assert_eq!(state["docs_indexed"], true);
    let requests = server.requests.load(Ordering::Relaxed);
    run(
        &index,
        &server,
        None,
        &["git", url, "--branch", "master", "--project", "another"],
    )?;
    assert_eq!(
        requests,
        server.requests.load(Ordering::Relaxed),
        "completed revision re-embedded"
    );
    assert_eq!(
        store
            .count("code_chunks", &project_filter(&["another".into()]))
            .await?,
        2
    );
    // A forced run must invalidate prior completion before even the model probe.
    let failed = Command::new(env!("CARGO_BIN_EXE_ingest"))
        .args(["git", url, "--branch", "master", "--force"])
        .env("AGENTIX_LANCE_PATH", &index)
        .env_remove("AGENTIX_STORE_URL")
        .env_remove("AGENTIX_STORE_TOKEN")
        .env_remove("AGENTIX_STORE_TOKEN_FILE")
        .env_remove("PG_DSN")
        .env("EMBED_MODEL", "fixture")
        .env("EMBED_DIMENSIONS", "4")
        .env("OLLAMA_HOST", "http://127.0.0.1:1")
        .output()?;
    assert!(!failed.status.success());
    let incomplete = store
        .metadata("git_ingest", &key)
        .await?
        .context("incomplete state")?;
    assert_eq!(incomplete["complete"], false);
    store
        .replace_file("documents", &key, "README.md", vec![])
        .await?;
    run(&index, &server, None, &["git", url, "--branch", "master"])?;
    assert_eq!(store.count("documents", "true").await?, 1);
    run(&index, &server, None, &["prune", "repo", &key])?;
    assert!(store.metadata("git_ingest", &key).await?.is_none());
    assert!(store.metadata("repo_index", &key).await?.is_none());
    Ok(())
}
async fn exercise(remote: bool) -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo)?;
    std::fs::write(
        repo.join("lib.rs"),
        "pub fn firstoriginal() -> u8 { 1 }\npub fn secondobsolete() -> u8 { 2 }\n",
    )?;
    std::fs::write(repo.join("README.md"),"# Fixture\n\nThis manualmarker explains how the two fixture functions are used by callers in this small project.\n")?;
    let index = tmp.path().join("index");
    let local = Store::open(&index, "fixture", 4).await?;
    let storage = if remote {
        Some(http::StorageServer::start(local, Some(http::TOKEN.into()))?)
    } else {
        None
    };
    let remote = storage.as_ref().map(|s| s.url.as_str());
    let server = Embeddings::start()?;
    let path = repo.to_str().context("UTF8 fixture path")?;
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "alpha"],
    )?;
    let reader = Store::open(&index, "fixture", 4).await?;
    assert_eq!(reader.count("code_chunks", "true").await?, 2);
    assert_eq!(reader.count("documents", "true").await?, 1);
    let calls = server.requests.load(Ordering::Relaxed);
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "beta"],
    )?;
    assert_eq!(
        server.requests.load(Ordering::Relaxed),
        calls,
        "unchanged files should not be embedded again"
    );
    for tag in ["alpha", "beta"] {
        assert_eq!(
            reader
                .count("code_chunks", &project_filter(&[tag.into()]))
                .await?,
            2
        );
        assert_eq!(
            reader
                .count("documents", &project_filter(&[tag.into()]))
                .await?,
            1
        );
    }
    std::fs::write(
        repo.join("lib.rs"),
        "pub fn replacementonly() -> u8 { 3 }\n",
    )?;
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "beta"],
    )?;
    assert_eq!(reader.count("code_chunks", "true").await?, 1);
    assert!(reader
        .rows("code_chunks", "true")
        .await?
        .iter()
        .all(|row| row["content"].as_str().is_some_and(
            |text| text.contains("replacementonly") && !text.contains("secondobsolete")
        )));
    assert!(reader
        .keyword("code_chunks", "secondobsolete", "true", 10)
        .await?
        .is_empty());
    std::fs::remove_file(repo.join("lib.rs"))?;
    run(&index, &server, remote, &["code", path])?;
    assert_eq!(reader.count("code_chunks", "true").await?, 0);
    run(
        &index,
        &server,
        remote,
        &["prune", "--dry-run", "repo", path],
    )?;
    assert_eq!(reader.count("documents", "true").await?, 1);
    run(&index, &server, remote, &["prune", "repo", path])?;
    assert_eq!(reader.count("documents", "true").await?, 0);
    Ok(())
}

#[tokio::test]
async fn formal_sources_are_searchable_locally() -> Result<()> {
    exercise_formal_sources(false).await
}

#[tokio::test]
async fn formal_sources_are_searchable_remotely() -> Result<()> {
    exercise_formal_sources(true).await
}

async fn exercise_formal_sources(remote: bool) -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().join("formal");
    std::fs::create_dir_all(repo.join(".git"))?;
    std::fs::write(repo.join(".gitignore"), "ignored/\n")?;
    let fixtures = [
        ("Proof.agda", "agda", "agdaproofmarker", "module Proof where\nagdaproofmarker : {A : Set} → A → A\nagdaproofmarker x = x\n"),
        ("Literate.lagda", "agda", "literatemarker", "A literate explanation.\n\\begin{code}\nmodule Literate where\nliteratemarker : {A : Set} → A → A\nliteratemarker x = x\n\\end{code}\n"),
        ("Main.LEAN", "lean", "leanproofmarker", "theorem leanproofmarker : ∀ (p : Prop), p → p := by\n  intro p h\n  exact h\n"),
        ("main.go", "go", "gofunctionmarker", "package main\nfunc gofunctionmarker() int { return 1 }\n"),
        ("Main.ELM", "elm", "elmfunctionmarker", "module Main exposing (elmfunctionmarker)\n\nelmfunctionmarker : Int\nelmfunctionmarker = 1\n"),
    ];
    for (file, _, _, source) in fixtures {
        std::fs::write(repo.join(file), source)?;
    }
    for file in [
        ".lake/packages/Dependency.lean",
        "nested/.lake/build/Generated.lean",
        "ignored/Secret.agda",
        "ignored/Secret.lean",
        "ignored/Secret.go",
        "ignored/Secret.elm",
        "elm-stuff/generated/Cache.elm",
    ] {
        let path = repo.join(file);
        std::fs::create_dir_all(path.parent().context("fixture parent")?)?;
        std::fs::write(path, "cacheignoredmarker")?;
    }
    let index = tmp.path().join("index");
    let local = Store::open(&index, "fixture", 4).await?;
    let storage = if remote {
        Some(http::StorageServer::start(local, Some(http::TOKEN.into()))?)
    } else {
        None
    };
    let remote = storage.as_ref().map(|s| s.url.as_str());
    let server = Embeddings::start()?;
    let path = repo.to_str().context("UTF8 formal fixture path")?;
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "formal"],
    )?;
    let reader = Store::open(&index, "fixture", 4).await?;
    assert_eq!(reader.count("code_chunks", "true").await?, 5);
    let project = project_filter(&["formal".into()]);
    for (file, language, marker, source) in fixtures {
        let filter = format!("({project}) AND language = '{language}'");
        let hits = reader.keyword("code_chunks", marker, &filter, 10).await?;
        assert_eq!(hits.len(), 1, "missing formal source {file}");
        let row = &hits[0].0;
        assert_eq!(row["file_path"], file);
        let start = row["start_line"].as_u64().context("formal start")? as usize;
        let end = row["end_line"].as_u64().context("formal end")? as usize;
        ensure!(start > 0 && end >= start && end <= source.lines().count());
        let expected = source
            .lines()
            .skip(start - 1)
            .take(end - start + 1)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(row["content"], expected);
        let kind = if language == "lean" {
            "theorem"
        } else {
            "function"
        };
        assert_eq!(row["symbol_kind"], kind);
        assert_eq!(
            reader
                .keyword(
                    "code_chunks",
                    marker,
                    &format!("({filter}) AND symbol_kind = '{kind}'"),
                    10
                )
                .await?
                .len(),
            1
        );
        let other_language = if language == "agda" { "lean" } else { "agda" };
        assert!(reader
            .keyword(
                "code_chunks",
                marker,
                &format!("language = '{other_language}'"),
                10
            )
            .await?
            .is_empty());
        assert!(reader
            .keyword(
                "code_chunks",
                marker,
                &project_filter(&["other".into()]),
                10
            )
            .await?
            .is_empty());
    }
    assert!(reader
        .keyword("code_chunks", "cacheignoredmarker", "true", 10)
        .await?
        .is_empty());
    let calls = server.requests.load(Ordering::Relaxed);
    run(
        &index,
        &server,
        remote,
        &["code", path, "--project", "formal"],
    )?;
    assert_eq!(server.requests.load(Ordering::Relaxed), calls);
    std::fs::write(repo.join("Proof.agda"), "-- replacementagdamarker\n")?;
    std::fs::write(repo.join("Main.LEAN"), "-- replacementleanmarker\n")?;
    std::fs::write(
        repo.join("main.go"),
        "package main\nfunc replacementgomarker() {}\n",
    )?;
    std::fs::write(
        repo.join("Main.ELM"),
        "module Main exposing (..)\nreplacementelmmarker = 2\n",
    )?;
    run(&index, &server, remote, &["code", path])?;
    for (marker, language, old) in [
        ("replacementagdamarker", "agda", "agdaproofmarker"),
        ("replacementleanmarker", "lean", "leanproofmarker"),
        ("replacementgomarker", "go", "gofunctionmarker"),
        ("replacementelmmarker", "elm", "elmfunctionmarker"),
    ] {
        assert_eq!(
            reader
                .keyword(
                    "code_chunks",
                    marker,
                    &format!("language = '{language}'"),
                    10
                )
                .await?
                .len(),
            1
        );
        assert!(reader
            .keyword("code_chunks", old, "true", 10)
            .await?
            .is_empty());
    }
    for (file, _, _, _) in fixtures {
        std::fs::remove_file(repo.join(file))?;
    }
    run(&index, &server, remote, &["code", path])?;
    assert_eq!(reader.count("code_chunks", "true").await?, 0);
    Ok(())
}

#[test]
fn formal_grammars_extract_declarations_and_original_line_ranges() {
    use agentix_indexer::ingest::symbols::extract_symbols;
    for (language, source, expected) in [
        (
            "go",
            include_str!("fixtures/indexing.go"),
            vec![
                ("Identity", "function"),
                ("Point", "struct"),
                ("Reader", "interface"),
                ("Alias", "type"),
                ("Point.Read", "method"),
                ("First", "const"),
                ("Second", "const"),
                ("Global", "variable"),
            ],
        ),
        (
            "elm",
            include_str!("fixtures/indexing.elm"),
            vec![
                ("identity", "function"),
                ("Choice", "type"),
                ("Box", "type"),
                ("unicode", "function"),
            ],
        ),
        (
            "agda",
            include_str!("fixtures/formal.agda"),
            vec![
                ("identity", "function"),
                ("nested", "function"),
                ("Bool", "type"),
                ("Box", "struct"),
                ("_∧_", "function"),
            ],
        ),
        (
            "lean",
            include_str!("fixtures/formal.lean"),
            vec![
                ("identity", "function"),
                ("identity_eq", "theorem"),
                ("unicode_identity", "theorem"),
                ("Count", "type"),
                ("Box", "struct"),
                ("Choice", "type"),
                ("nested", "function"),
            ],
        ),
    ] {
        let symbols = extract_symbols(source, language);
        for (name, kind) in expected {
            assert!(
                symbols.iter().any(|s| s.name == name && s.kind == kind),
                "missing {language} {kind} {name}; got {:?}",
                symbols
                    .iter()
                    .map(|s| (&s.name, &s.kind))
                    .collect::<Vec<_>>()
            );
        }
        assert!(symbols.iter().all(|s| !s.name.contains("Fake")));
        for symbol in &symbols {
            let expected = source
                .lines()
                .skip(symbol.start_line - 1)
                .take(symbol.end_line - symbol.start_line + 1)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(symbol.content, expected);
        }
        if language == "agda" || language == "elm" {
            let identity: Vec<_> = symbols.iter().filter(|s| s.name == "identity").collect();
            assert_eq!(identity.len(), 1);
            assert!(identity[0].content.contains("identity :"));
            assert!(identity[0].content.contains("="));
        }
        if language == "agda" {
            assert!(symbols
                .iter()
                .any(|s| s.name == "Bool" && s.content.contains("true : Bool")));
            assert!(symbols
                .iter()
                .any(|s| s.name == "Box" && s.content.contains("value : A")));
            let operator: Vec<_> = symbols.iter().filter(|s| s.name == "_∧_").collect();
            assert_eq!(operator.len(), 1);
            assert!(operator[0].content.contains("false ∧ b = false"));
            assert!(operator[0].content.contains("true ∧ b = b"));
        }
    }
}
