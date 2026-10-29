use agentix_store::Store;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use super::embed::embed_batch;

const GITHUB_API: &str = "https://api.github.com";
const PAGE_SIZE: usize = 100;
const EMBED_BATCH: usize = 8;
const MIN_BODY_LEN: usize = 20;

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn ingest_github(
    pool: &Store,
    repo: &str,
    force: bool,
    streams: &[String],
) -> Result<()> {
    if !repo.contains('/') {
        anyhow::bail!("repo must be in OWNER/REPO format, got: {}", repo);
    }

    let token = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    if token.is_empty() {
        eprintln!("Warning: GITHUB_TOKEN not set. Rate limit: 60 req/hour.");
    }

    let http = reqwest::Client::builder()
        .user_agent("agentix-indexer (https://github.com/nixedge/agentix)")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut failures = Vec::new();
    anyhow::ensure!(
        streams.iter().all(|s| matches!(
            s.as_str(),
            "all" | "issues" | "prs" | "issue_comments" | "pr_comments"
        )),
        "unknown GitHub stream"
    );
    let do_all = streams.is_empty() || streams.iter().any(|s| s == "all");

    if do_all || streams.iter().any(|s| s == "issues") {
        eprintln!("── Issues ──");
        match ingest_issues(pool, &http, &token, repo, force).await {
            Ok((n, s)) => eprintln!("  ✓ {} upserted, {} skipped", n, s),
            Err(e) => {
                record_error(pool, "github", &format!("{repo}:issues"), &e.to_string()).await?;
                failures.push(e.to_string());
                eprintln!("  ✗ issues failed: {e}");
            }
        }
    }

    if do_all || streams.iter().any(|s| s == "prs") {
        eprintln!("── Pull Requests ──");
        match ingest_prs(pool, &http, &token, repo, force).await {
            Ok((n, s)) => eprintln!("  ✓ {} upserted, {} skipped", n, s),
            Err(e) => {
                record_error(pool, "github", &format!("{repo}:prs"), &e.to_string()).await?;
                failures.push(e.to_string());
                eprintln!("  ✗ PRs failed: {e}");
            }
        }
    }

    if do_all || streams.iter().any(|s| s == "issue_comments") {
        eprintln!("── Issue Comments ──");
        match ingest_issue_comments(pool, &http, &token, repo, force).await {
            Ok(n) => eprintln!("  ✓ {} upserted", n),
            Err(e) => {
                record_error(
                    pool,
                    "github",
                    &format!("{repo}:issue_comments"),
                    &e.to_string(),
                )
                .await?;
                failures.push(e.to_string());
                eprintln!("  ✗ issue comments failed: {e}");
            }
        }
    }

    if do_all || streams.iter().any(|s| s == "pr_comments") {
        eprintln!("── PR Review Comments ──");
        match ingest_pr_comments(pool, &http, &token, repo, force).await {
            Ok(n) => eprintln!("  ✓ {} upserted", n),
            Err(e) => {
                record_error(
                    pool,
                    "github",
                    &format!("{repo}:pr_comments"),
                    &e.to_string(),
                )
                .await?;
                failures.push(e.to_string());
                eprintln!("  ✗ PR review comments failed: {e}");
            }
        }
    }

    anyhow::ensure!(
        failures.is_empty(),
        "GitHub sync failed: {}",
        failures.join("; ")
    );
    Ok(())
}

// ── GitHub API client helpers ─────────────────────────────────────────────────

fn gh_headers(token: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION};
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "X-GitHub-Api-Version",
        HeaderValue::from_static("2022-11-28"),
    );
    if !token.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(AUTHORIZATION, v);
        }
    }
    headers
}

/// Paginate a GitHub endpoint, yielding JSON arrays item by item.
async fn paginate(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    extra_params: &[(&str, &str)],
) -> Result<Vec<serde_json::Value>> {
    let mut all = vec![];
    let mut next_url = Some(format!(
        "{url}?per_page={PAGE_SIZE}{}",
        extra_params
            .iter()
            .map(|(k, v)| format!("&{k}={v}"))
            .collect::<String>()
    ));
    let headers = gh_headers(token);

    while let Some(url) = next_url.take() {
        let resp = http
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
            .context("GitHub API request failed")?;

        resp.error_for_status_ref()
            .with_context(|| format!("GitHub API error for {url}"))?;

        // Extract next page URL from Link header
        let link = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        next_url = extract_next_link(&link);

        let data: serde_json::Value = resp.json().await?;
        if let Some(arr) = data.as_array() {
            all.extend(arr.iter().cloned());
        }
    }
    Ok(all)
}

fn extract_next_link(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        let part = part.trim();
        if part.contains(r#"rel="next""#) {
            if let Some(url) = part.split(';').next() {
                let url = url.trim().trim_start_matches('<').trim_end_matches('>');
                return Some(url.to_string());
            }
        }
    }
    None
}

// ── Sync state helpers ────────────────────────────────────────────────────────

async fn get_watermark(pool: &Store, source: &str, scope: &str) -> Result<Option<String>> {
    Ok(pool
        .metadata("sync_state", &format!("{source}:{scope}"))
        .await?
        .and_then(|r| r["watermark"].as_str().map(str::to_owned)))
}
async fn set_watermark(
    pool: &Store,
    source: &str,
    scope: &str,
    watermark: Option<&str>,
) -> Result<()> {
    let key = format!("{source}:{scope}");
    let now = Utc::now().to_rfc3339();
    pool.update_metadata("sync_state",&key,|previous|{
        let watermark=latest_ts(previous.as_ref().and_then(|r|r["watermark"].as_str()),watermark);
        Ok(serde_json::json!({"id":key,"source_name":source,"scope_key":scope,"watermark":watermark,
            "last_synced_at":now,"updated_at":now,"error_count":0,"last_error":null}))
    }).await
}
async fn record_error(pool: &Store, source: &str, scope: &str, error: &str) -> Result<()> {
    let key = format!("{source}:{scope}");
    pool.update_metadata("sync_state", &key, |previous| {
        let mut row = previous.unwrap_or_else(
            || serde_json::json!({"id":key,"source_name":source,"scope_key":scope}),
        );
        row["error_count"] = serde_json::json!(row["error_count"].as_u64().unwrap_or(0) + 1);
        row["last_error"] = serde_json::json!(error.chars().take(1000).collect::<String>());
        row["updated_at"] = serde_json::json!(Utc::now().to_rfc3339());
        Ok(row)
    })
    .await
}

// ── Content helpers ───────────────────────────────────────────────────────────

fn make_content(title: &str, body: Option<&str>) -> String {
    let title = title.trim();
    match body.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => format!("{title}\n\n{b}"),
        None => title.to_string(),
    }
}

fn content_hash(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    hex::encode(h.finalize())
}

fn parse_ts(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|d| d.with_timezone(&Utc))
}

fn latest_ts<'a>(a: Option<&'a str>, b: Option<&'a str>) -> Option<&'a str> {
    match (a, b) {
        (None, b) => b,
        (a, None) => a,
        (Some(a), Some(b)) => {
            let ta = DateTime::parse_from_rfc3339(a).ok();
            let tb = DateTime::parse_from_rfc3339(b).ok();
            match (ta, tb) {
                (Some(ta), Some(tb)) => Some(if tb > ta { b } else { a }),
                _ => Some(a),
            }
        }
    }
}

fn labels_str(labels: &serde_json::Value) -> String {
    labels
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

// ── Issues ────────────────────────────────────────────────────────────────────

async fn ingest_issues(
    pool: &Store,
    http: &reqwest::Client,
    token: &str,
    repo: &str,
    force: bool,
) -> Result<(usize, usize)> {
    let scope = format!("{repo}:issues");
    let watermark = if force {
        None
    } else {
        get_watermark(pool, "github", &scope).await?
    };

    let mut params = vec![("state", "all"), ("sort", "updated"), ("direction", "asc")];
    let since_owned: String;
    if let Some(ref wm) = watermark {
        eprintln!("  issues: incremental since {wm}");
        since_owned = wm.clone();
        params.push(("since", &since_owned));
    } else {
        eprintln!("  issues: full fetch");
    }

    let items = paginate(
        http,
        token,
        &format!("{GITHUB_API}/repos/{repo}/issues"),
        &params,
    )
    .await?;

    let mut new_watermark: Option<String> = watermark.clone();
    let mut upserted = 0usize;
    let mut skipped = 0usize;
    let mut pending: Vec<serde_json::Value> = vec![];

    for issue in items {
        // GitHub issues API returns PRs too — filter them out
        if issue["pull_request"].is_object() {
            continue;
        }
        new_watermark =
            latest_ts(new_watermark.as_deref(), issue["updated_at"].as_str()).map(str::to_string);

        pending.push(issue);
        if pending.len() >= EMBED_BATCH {
            flush_issues(&mut pending, pool, repo, &mut upserted, &mut skipped).await?;
        }
    }
    flush_issues(&mut pending, pool, repo, &mut upserted, &mut skipped).await?;

    set_watermark(pool, "github", &scope, new_watermark.as_deref()).await?;
    Ok((upserted, skipped))
}

async fn flush_issues(
    pending: &mut Vec<serde_json::Value>,
    pool: &Store,
    repo: &str,
    upserted: &mut usize,
    _skipped: &mut usize,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let contents: Vec<String> = pending
        .iter()
        .map(|issue| {
            make_content(
                issue["title"].as_str().unwrap_or(""),
                issue["body"].as_str(),
            )
        })
        .collect();
    let texts: Vec<&str> = contents.iter().map(String::as_str).collect();
    let embeddings = embed_batch(&texts).await?;

    for (issue, emb) in pending.iter().zip(embeddings.iter()) {
        let content = make_content(
            issue["title"].as_str().unwrap_or(""),
            issue["body"].as_str(),
        );
        let hash = content_hash(&content);
        pool.put("github_issues",vec![agentix_store::Record{row:serde_json::json!({
            "repo":repo,"number":issue["number"],"state":issue["state"],"title":issue["title"],"body":issue["body"],
            "author":issue["user"]["login"],"labels":labels_str(&issue["labels"]),"content":content,
            "created_at":issue["created_at"],"updated_at":issue["updated_at"],"closed_at":issue["closed_at"],
            "content_hash":hash,"indexed_at":Utc::now().to_rfc3339()}),embedding:Some(emb.clone())}]).await?;

        *upserted += 1;
    }
    pending.clear();
    Ok(())
}

// ── Pull Requests ─────────────────────────────────────────────────────────────

async fn ingest_prs(
    pool: &Store,
    http: &reqwest::Client,
    token: &str,
    repo: &str,
    force: bool,
) -> Result<(usize, usize)> {
    let scope = format!("{repo}:prs");
    let watermark = if force {
        None
    } else {
        get_watermark(pool, "github", &scope).await?
    };

    let params = [("state", "all"), ("sort", "updated"), ("direction", "asc")];
    if let Some(ref wm) = watermark {
        eprintln!("  PRs: incremental since {wm}");
    } else {
        eprintln!("  PRs: full fetch");
    }

    let watermark_dt = watermark.as_deref().and_then(|s| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    });

    let items = paginate(
        http,
        token,
        &format!("{GITHUB_API}/repos/{repo}/pulls"),
        &params,
    )
    .await?;

    let mut new_watermark: Option<String> = watermark.clone();
    let mut upserted = 0usize;
    let mut skipped = 0usize;
    let mut pending: Vec<serde_json::Value> = vec![];

    for pr in items {
        // GitHub PRs API has no `since` param — filter client-side
        if let Some(wm_dt) = watermark_dt {
            if let Some(updated_dt) = parse_ts(pr["updated_at"].as_str()) {
                if updated_dt <= wm_dt {
                    skipped += 1;
                    continue;
                }
            }
        }
        new_watermark =
            latest_ts(new_watermark.as_deref(), pr["updated_at"].as_str()).map(str::to_string);

        pending.push(pr);
        if pending.len() >= EMBED_BATCH {
            flush_prs(&mut pending, pool, repo, &mut upserted).await?;
        }
    }
    flush_prs(&mut pending, pool, repo, &mut upserted).await?;

    set_watermark(pool, "github", &scope, new_watermark.as_deref()).await?;
    Ok((upserted, skipped))
}

async fn flush_prs(
    pending: &mut Vec<serde_json::Value>,
    pool: &Store,
    repo: &str,
    upserted: &mut usize,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let contents: Vec<String> = pending
        .iter()
        .map(|pr| make_content(pr["title"].as_str().unwrap_or(""), pr["body"].as_str()))
        .collect();
    let texts: Vec<&str> = contents.iter().map(String::as_str).collect();
    let embeddings = embed_batch(&texts).await?;

    for (pr, emb) in pending.iter().zip(embeddings.iter()) {
        let content = make_content(pr["title"].as_str().unwrap_or(""), pr["body"].as_str());
        let hash = content_hash(&content);
        let state = if pr["merged_at"].is_string() {
            "merged"
        } else {
            pr["state"].as_str().unwrap_or("open")
        };
        pool.put(
            "github_prs",
            vec![agentix_store::Record {
                row: serde_json::json!({
            "repo":repo,"number":pr["number"],"state":state,"title":pr["title"],"body":pr["body"],
            "author":pr["user"]["login"],"labels":labels_str(&pr["labels"]),"content":content,
            "base_branch":pr["base"]["ref"],"head_branch":pr["head"]["ref"],
            "created_at":pr["created_at"],"updated_at":pr["updated_at"],"merged_at":pr["merged_at"],
            "content_hash":hash,"indexed_at":Utc::now().to_rfc3339()}),
                embedding: Some(emb.clone()),
            }],
        )
        .await?;

        *upserted += 1;
    }
    pending.clear();
    Ok(())
}

// ── Issue comments ────────────────────────────────────────────────────────────

async fn ingest_issue_comments(
    pool: &Store,
    http: &reqwest::Client,
    token: &str,
    repo: &str,
    force: bool,
) -> Result<usize> {
    let scope = format!("{repo}:issue_comments");
    let watermark = if force {
        None
    } else {
        get_watermark(pool, "github", &scope).await?
    };

    let mut params = vec![("sort", "updated"), ("direction", "asc")];
    let since_owned: String;
    if let Some(ref wm) = watermark {
        eprintln!("  issue comments: incremental since {wm}");
        since_owned = wm.clone();
        params.push(("since", &since_owned));
    } else {
        eprintln!("  issue comments: full fetch");
    }

    let items = paginate(
        http,
        token,
        &format!("{GITHUB_API}/repos/{repo}/issues/comments"),
        &params,
    )
    .await?;

    let mut new_watermark: Option<String> = watermark.clone();
    let mut upserted = 0usize;
    let mut pending: Vec<serde_json::Value> = vec![];

    for comment in items {
        let body = comment["body"].as_str().unwrap_or("");
        if body.trim().len() < MIN_BODY_LEN {
            continue;
        }
        // Extract issue number from issue_url: .../issues/123
        let issue_number: Option<i32> = comment["issue_url"]
            .as_str()
            .and_then(|u| u.rsplit('/').next())
            .and_then(|s| s.parse().ok());
        if issue_number.is_none() {
            continue;
        }
        new_watermark =
            latest_ts(new_watermark.as_deref(), comment["updated_at"].as_str()).map(str::to_string);

        pending.push(comment);
        if pending.len() >= EMBED_BATCH {
            flush_issue_comments(&mut pending, pool, repo, &mut upserted).await?;
        }
    }
    flush_issue_comments(&mut pending, pool, repo, &mut upserted).await?;

    set_watermark(pool, "github", &scope, new_watermark.as_deref()).await?;
    Ok(upserted)
}

async fn flush_issue_comments(
    pending: &mut Vec<serde_json::Value>,
    pool: &Store,
    repo: &str,
    upserted: &mut usize,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let texts: Vec<&str> = pending
        .iter()
        .map(|c| c["body"].as_str().unwrap_or(""))
        .collect();
    let embeddings = embed_batch(&texts).await?;

    for (comment, emb) in pending.iter().zip(embeddings.iter()) {
        let body = comment["body"].as_str().unwrap_or("");
        let issue_number: i32 = comment["issue_url"]
            .as_str()
            .and_then(|u| u.rsplit('/').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let hash = content_hash(body);

        pool.put("github_issue_comments",vec![agentix_store::Record{row:serde_json::json!({
            "repo":repo,"issue_number":issue_number,"comment_id":comment["id"],"author":comment["user"]["login"],"content":body,
            "created_at":comment["created_at"],"updated_at":comment["updated_at"],"content_hash":hash,
            "indexed_at":Utc::now().to_rfc3339()}),embedding:Some(emb.clone())}]).await?;

        *upserted += 1;
    }
    pending.clear();
    Ok(())
}

// ── PR review comments ────────────────────────────────────────────────────────

async fn ingest_pr_comments(
    pool: &Store,
    http: &reqwest::Client,
    token: &str,
    repo: &str,
    force: bool,
) -> Result<usize> {
    let scope = format!("{repo}:pr_comments");
    let watermark = if force {
        None
    } else {
        get_watermark(pool, "github", &scope).await?
    };

    let mut params = vec![("sort", "updated"), ("direction", "asc")];
    let since_owned: String;
    if let Some(ref wm) = watermark {
        eprintln!("  PR review comments: incremental since {wm}");
        since_owned = wm.clone();
        params.push(("since", &since_owned));
    } else {
        eprintln!("  PR review comments: full fetch");
    }

    let items = paginate(
        http,
        token,
        &format!("{GITHUB_API}/repos/{repo}/pulls/comments"),
        &params,
    )
    .await?;

    let mut new_watermark: Option<String> = watermark.clone();
    let mut upserted = 0usize;
    let mut pending: Vec<serde_json::Value> = vec![];

    for comment in items {
        let body = comment["body"].as_str().unwrap_or("");
        if body.trim().len() < MIN_BODY_LEN {
            continue;
        }
        let pr_number: Option<i32> = comment["pull_request_url"]
            .as_str()
            .and_then(|u| u.rsplit('/').next())
            .and_then(|s| s.parse().ok());
        if pr_number.is_none() {
            continue;
        }
        new_watermark =
            latest_ts(new_watermark.as_deref(), comment["updated_at"].as_str()).map(str::to_string);

        pending.push(comment);
        if pending.len() >= EMBED_BATCH {
            flush_pr_comments(&mut pending, pool, repo, &mut upserted).await?;
        }
    }
    flush_pr_comments(&mut pending, pool, repo, &mut upserted).await?;

    set_watermark(pool, "github", &scope, new_watermark.as_deref()).await?;
    Ok(upserted)
}

async fn flush_pr_comments(
    pending: &mut Vec<serde_json::Value>,
    pool: &Store,
    repo: &str,
    upserted: &mut usize,
) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }

    // Build full searchable content: prepend file path if available
    let contents: Vec<String> = pending
        .iter()
        .map(|c| {
            let body = c["body"].as_str().unwrap_or("");
            match c["path"].as_str() {
                Some(p) => format!("{p}\n{body}"),
                None => body.to_string(),
            }
        })
        .collect();
    let texts: Vec<&str> = contents.iter().map(String::as_str).collect();
    let embeddings = embed_batch(&texts).await?;

    for (comment, emb) in pending.iter().zip(embeddings.iter()) {
        let body = comment["body"].as_str().unwrap_or("");
        let pr_number: i32 = comment["pull_request_url"]
            .as_str()
            .and_then(|u| u.rsplit('/').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let file_path = comment["path"].as_str();
        let content = match file_path {
            Some(p) => format!("{p}\n{body}"),
            None => body.to_string(),
        };
        let diff_hunk = comment["diff_hunk"]
            .as_str()
            .map(|s| s.chars().take(2000).collect::<String>());
        let hash = content_hash(&content);

        pool.put("github_pr_comments",vec![agentix_store::Record{row:serde_json::json!({
            "repo":repo,"pr_number":pr_number,"comment_id":comment["id"],"comment_type":"review_comment","author":comment["user"]["login"],
            "content":content,"file_path":file_path,"diff_hunk":diff_hunk,"created_at":comment["created_at"],"updated_at":comment["updated_at"],
            "content_hash":hash,"indexed_at":Utc::now().to_rfc3339()}),embedding:Some(emb.clone())}]).await?;

        *upserted += 1;
    }
    pending.clear();
    Ok(())
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    #[tokio::test]
    async fn sync_errors_preserve_watermarks_and_success_never_regresses_them() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store = Store::open(tmp.path(), "fixture", 4).await?;
        let scope = "owner/repo:issues";
        set_watermark(&store, "github", scope, Some("2026-10-28T00:00:00Z")).await?;
        record_error(&store, "github", scope, &"é".repeat(1100)).await?;
        assert_eq!(
            get_watermark(&store, "github", scope).await?.as_deref(),
            Some("2026-10-28T00:00:00Z")
        );
        let row = store
            .metadata("sync_state", &format!("github:{scope}"))
            .await?
            .context("missing sync state")?;
        assert_eq!(row["error_count"], 1);
        assert_eq!(
            row["last_error"]
                .as_str()
                .context("missing error")?
                .chars()
                .count(),
            1000
        );
        set_watermark(&store, "github", scope, Some("2026-10-27T00:00:00Z")).await?;
        assert_eq!(
            get_watermark(&store, "github", scope).await?.as_deref(),
            Some("2026-10-28T00:00:00Z")
        );
        let row = store
            .metadata("sync_state", &format!("github:{scope}"))
            .await?
            .context("missing sync state")?;
        assert_eq!(row["error_count"], 0);
        assert!(row["last_error"].is_null());
        record_error(&store, "github", scope, "temporary error").await?;
        set_watermark(&store, "github", scope, None).await?;
        assert_eq!(
            get_watermark(&store, "github", scope).await?.as_deref(),
            Some("2026-10-28T00:00:00Z")
        );
        let empty_scope = "owner/empty:issues";
        record_error(&store, "github", empty_scope, "temporary error").await?;
        set_watermark(&store, "github", empty_scope, None).await?;
        let empty = store
            .metadata("sync_state", &format!("github:{empty_scope}"))
            .await?
            .context("missing empty stream state")?;
        assert!(empty["watermark"].is_null());
        assert_eq!(empty["error_count"], 0);
        assert!(empty["last_error"].is_null());
        assert!(empty["last_synced_at"].is_string());
        Ok(())
    }
}
