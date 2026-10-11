#[path = "../src/tools/access.rs"]
mod access;
use access::{digest, FileAccess};
use anyhow::Result;
#[test]
fn edits_preserve_preimages_and_reject_escape_paths() -> Result<()> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let path = root.path().to_path_buf();
    let fs = FileAccess::new(path.clone(), vec![path.clone()], vec![path.clone()])?;
    fs.write(std::path::Path::new("a"), "old", None)?;
    assert!(fs.write(std::path::Path::new("a"), "bad", None).is_err());
    fs.write(std::path::Path::new("a"), "new", Some(&digest("old")))?;
    assert!(fs
        .write(std::path::Path::new("a"), "stale", Some(&digest("old")))
        .is_err());
    assert_eq!(fs.read(std::path::Path::new("a"))?, "new");
    std::os::unix::fs::symlink(outside.path(), path.join("link"))?;
    assert!(fs
        .write(std::path::Path::new("link/escaped"), "bad", None)
        .is_err());
    assert!(fs.read(std::path::Path::new("../secret")).is_err());
    assert!(fs.read(std::path::Path::new(".git/config")).is_err());
    assert!(fs.list(std::path::Path::new("."))?.contains("a"));
    fs.check(std::path::Path::new("a"), Some(&digest("new")))?;
    fs.delete(std::path::Path::new("a"), &digest("new"))?;
    assert!(!path.join("a").exists());
    Ok(())
}
#[test]
fn read_only_grants_and_binary_limits_are_enforced() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().to_path_buf();
    let fs = FileAccess::new(path.clone(), vec![path.clone()], vec![])?;
    std::fs::write(path.join("binary"), b"a\0b")?;
    assert!(fs.read(std::path::Path::new("binary")).is_err());
    assert!(fs.write(std::path::Path::new("new"), "bad", None).is_err());
    Ok(())
}

#[test]
fn symlink_swap_cannot_redirect_a_file_edit_outside_the_grant() -> Result<()> {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let dir = root.path().join("directory");
    let held = root.path().join("held");
    std::fs::create_dir(&dir)?;
    std::fs::write(dir.join("value"), "safe")?;
    std::fs::write(outside.path().join("value"), "secret")?;
    let fs = FileAccess::new(
        root.path().into(),
        vec![root.path().into()],
        vec![root.path().into()],
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let target = outside.path().to_path_buf();
    let racer = std::thread::spawn(move || -> Result<()> {
        while !flag.load(Ordering::Relaxed) {
            std::fs::rename(&dir, &held)?;
            std::os::unix::fs::symlink(&target, &dir)?;
            std::thread::yield_now();
            std::fs::remove_file(&dir)?;
            std::fs::rename(&held, &dir)?;
        }
        Ok(())
    });
    let mut leaked = false;
    for _ in 0..1000 {
        if fs
            .read(std::path::Path::new("directory/value"))
            .is_ok_and(|s| s != "safe")
        {
            leaked = true;
            break;
        }
        let _ = fs.write(
            std::path::Path::new("directory/value"),
            "safe",
            Some(&digest("safe")),
        );
    }
    stop.store(true, Ordering::Relaxed);
    racer
        .join()
        .map_err(|_| anyhow::anyhow!("race fixture panicked"))??;
    assert!(!leaked);
    assert_eq!(
        std::fs::read_to_string(outside.path().join("value"))?,
        "secret"
    );
    Ok(())
}

#[tokio::test]
async fn patch_preflight_blocks_partial_edits_and_text_search_reports_lines() -> Result<()> {
    use agentix_harness::Tool;
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("a"), "old\n")?;
    std::fs::write(root.path().join("b"), "external\n")?;
    let access = std::sync::Arc::new(agentix_ax::tools::access::FileAccess::new(
        root.path().into(),
        vec![root.path().into()],
        vec![root.path().into()],
    )?);
    let patch = agentix_ax::tools::patch::ApplyPatch(access.clone());
    assert!(patch.call(serde_json::json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-old\n+new\n*** Update File: b\n@@\n-old\n+new\n*** End Patch","expected_sha256":{"a":digest("old\n"),"b":digest("old\n")}})).await.is_err());
    assert_eq!(std::fs::read_to_string(root.path().join("a"))?, "old\n");
    let result: serde_json::Value = serde_json::from_str(
        &agentix_ax::tools::search::SearchText(access)
            .call(serde_json::json!({"path":"b","pattern":"external"}))
            .await?,
    )?;
    assert_eq!(result["matches"][0]["line"], 1);
    assert_eq!(result["matches"][0]["text"], "external");
    Ok(())
}
