use agentix_assistant_core::{Error, Repository};
use agentix_assistant_sled::{RecordingArchive, SledRepository};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

mod support;
use support::{fixture, metadata};

#[tokio::test]
async fn replay_restart_source_conflicts_and_atomic_transcript_work() -> TestResult {
    let temp = tempfile::tempdir()?;
    let repository = SledRepository::open(temp.path().join("db")).await?;
    let archive = RecordingArchive::new(repository.clone(), temp.path().join("audio"));
    let audio = fixture();
    let metadata = metadata(&audio);
    let first = archive.put(metadata.clone(), audio.clone()).await?;
    assert_eq!(archive.put(metadata.clone(), audio.clone()).await?, first);
    assert_eq!(archive.audio(metadata.id).await?, audio);
    assert_eq!(archive.pending(10).await?.len(), 1);
    let mut changed = metadata.clone();
    changed.id = Uuid::new_v4();
    assert!(matches!(
        archive.put(changed, audio.clone()).await,
        Err(Error::RequestConflict)
    ));
    let mut bad = audio.clone();
    bad[30] ^= 1;
    assert!(matches!(
        archive.put(metadata.clone(), bad).await,
        Err(Error::Invalid(_))
    ));
    archive.begin_attempt(metadata.id).await?;
    assert!(archive.pending(10).await?.is_empty());
    let text = "Remember the cobalt accordion. ".repeat(1000);
    let transcript = archive
        .finish_transcript(metadata.id, text.clone(), "fixture-local".into())
        .await?;
    assert_eq!(transcript.text, text);
    assert_eq!(
        archive
            .finish_transcript(
                metadata.id,
                "different retry".into(),
                "fixture-local".into()
            )
            .await?,
        transcript
    );
    let receipt = archive.get(metadata.id).await?;
    assert!(receipt.recording.transcribed_at.is_some());
    assert!(receipt.search_pending);
    assert!(receipt.recording.transcript_record_ids.len() > 1);
    assert_eq!(
        repository.records().await?.len(),
        receipt.recording.transcript_record_ids.len()
    );
    assert_eq!(
        repository.pending_projections(100).await?.len(),
        receipt.recording.transcript_record_ids.len()
    );
    drop(archive);
    repository.close().await?;
    let reopened = SledRepository::open(temp.path().join("db")).await?;
    let archive = RecordingArchive::new(reopened.clone(), temp.path().join("audio"));
    assert_eq!(archive.get(metadata.id).await?, receipt);
    assert_eq!(archive.transcript(metadata.id).await?, transcript);
    assert_eq!(archive.put(metadata.clone(), audio).await?, receipt);
    drop(archive);
    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn corrupt_container_or_missing_original_cannot_report_success() -> TestResult {
    let temp = tempfile::tempdir()?;
    let repository = SledRepository::open(temp.path().join("db")).await?;
    let archive = RecordingArchive::new(repository.clone(), temp.path().join("audio"));
    let mut invalid = fixture();
    invalid[35] ^= 1;
    assert!(matches!(
        archive.put(metadata(&invalid), invalid).await,
        Err(Error::Invalid(_))
    ));
    let audio = fixture();
    let metadata = metadata(&audio);
    archive.put(metadata.clone(), audio).await?;
    let path = archive.audio_path(metadata.id).await?;
    tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await??;
    assert!(matches!(
        archive.get(metadata.id).await,
        Err(Error::Storage(_))
    ));
    drop(archive);
    repository.close().await?;
    Ok(())
}
