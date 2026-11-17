use crate::{decode, encode, keys, storage, SledRepository};
use agentix_assistant_core::{
    Error, ProjectionJob, Record, RecordBody, RecordingMetadata, RecordingReceipt,
    RecordingTranscript, Result, StoredRecording, SCHEMA_VERSION,
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct RecordingArchive {
    repository: SledRepository,
    root: PathBuf,
}

pub fn recording_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn key(id: Uuid) -> Vec<u8> {
    format!("audio/recording/{id}").into_bytes()
}
fn transcript_key(id: Uuid) -> Vec<u8> {
    format!("audio/transcript/{id}").into_bytes()
}
fn source_key(metadata: &RecordingMetadata) -> Vec<u8> {
    format!(
        "audio/source/{}/{}",
        metadata.source_device, metadata.source_session
    )
    .into_bytes()
}
fn load(db: &sled::Db, id: Uuid) -> Result<StoredRecording> {
    let bytes = db
        .get(key(id))
        .map_err(storage)?
        .ok_or(Error::NotFound(id))?;
    let record: StoredRecording = decode(&bytes)?;
    record.metadata.validate()?;
    if record.metadata.id != id {
        return Err(Error::Storage("recording identity mismatch".into()));
    }
    Ok(record)
}
fn server_id(db: &sled::Db) -> Result<Uuid> {
    let key = b"meta/recordings-server-id";
    if let Some(bytes) = db.get(key).map_err(storage)? {
        return decode(&bytes);
    }
    let id = Uuid::new_v4();
    db.insert(key, encode(&id)?).map_err(storage)?;
    db.flush().map_err(storage)?;
    Ok(id)
}
fn blob(root: &Path, metadata: &RecordingMetadata) -> PathBuf {
    root.join("blobs")
        .join(&metadata.sha256[..2])
        .join(format!("{}.ogg", metadata.sha256))
}

impl RecordingArchive {
    pub fn new(repository: SledRepository, root: PathBuf) -> Self {
        Self { repository, root }
    }

    pub async fn put(
        &self,
        metadata: RecordingMetadata,
        audio: Vec<u8>,
    ) -> Result<RecordingReceipt> {
        metadata.validate()?;
        let root = self.root.clone();
        self.repository
            .blocking(move |db| {
                if audio.len() as u64 != metadata.audio_bytes
                    || recording_hash(&audio) != metadata.sha256
                {
                    return Err(Error::Invalid("recording checksum/length mismatch".into()));
                }
                validate_ogg(&audio, metadata.duration_ms)?;
                let prior = db
                    .get(key(metadata.id))
                    .map_err(storage)?
                    .map(|bytes| decode::<StoredRecording>(&bytes))
                    .transpose()?;
                if prior.as_ref().is_some_and(|r| r.metadata != metadata) {
                    return Err(Error::RequestConflict);
                }
                if let Some(existing) = db.get(source_key(&metadata)).map_err(storage)? {
                    let id: Uuid = decode(&existing)?;
                    if id != metadata.id {
                        return Err(Error::RequestConflict);
                    }
                }
                let destination = blob(&root, &metadata);
                let parent = destination
                    .parent()
                    .ok_or_else(|| Error::Storage("invalid blob location".into()))?;
                let mut directories = fs::DirBuilder::new();
                directories.recursive(true);
                #[cfg(unix)]
                directories.mode(0o700);
                directories.create(parent).map_err(storage)?;
                if destination.exists() {
                    let existing = fs::read(&destination).map_err(storage)?;
                    if recording_hash(&existing) != metadata.sha256 {
                        return Err(Error::Storage("stored audio checksum mismatch".into()));
                    }
                } else {
                    let temporary = parent.join(format!(".{}.part", Uuid::new_v4()));
                    let published = (|| {
                        let mut options = OpenOptions::new();
                        options.create_new(true).write(true);
                        #[cfg(unix)]
                        options.mode(0o600);
                        let mut file = options.open(&temporary).map_err(storage)?;
                        file.write_all(&audio).map_err(storage)?;
                        file.sync_all().map_err(storage)?;
                        fs::rename(&temporary, &destination).map_err(storage)?;
                        Ok(())
                    })();
                    if published.is_err() {
                        let _ = fs::remove_file(&temporary);
                    }
                    published?;
                }
                // Persist every directory entry before Sled can acknowledge its blob reference.
                let blobs = root.join("blobs");
                for directory in [
                    parent,
                    blobs.as_path(),
                    root.as_path(),
                    root.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new(".")),
                ] {
                    File::open(directory)
                        .and_then(|file| file.sync_all())
                        .map_err(storage)?;
                }
                let now = Utc::now();
                let recording = prior.unwrap_or_else(|| StoredRecording {
                    metadata: metadata.clone(),
                    stored_at: now,
                    transcription_attempts: 0,
                    retry_at: now,
                    transcription_error: None,
                    transcript_record_ids: vec![],
                    transcribed_at: None,
                });
                let identity = server_id(db)?;
                let mut batch = sled::Batch::default();
                batch.insert(key(metadata.id), encode(&recording)?);
                batch.insert(source_key(&metadata), encode(&metadata.id)?);
                db.apply_batch(batch).map_err(storage)?;
                db.flush().map_err(storage)?;
                let search_pending = recording
                    .transcript_record_ids
                    .iter()
                    .any(|id| db.contains_key(keys::projection(*id)).unwrap_or(true));
                Ok(RecordingReceipt {
                    server_id: identity,
                    recording,
                    search_pending,
                })
            })
            .await
    }

    pub async fn get(&self, id: Uuid) -> Result<RecordingReceipt> {
        let root = self.root.clone();
        self.repository
            .blocking(move |db| {
                db.flush().map_err(storage)?;
                let recording = load(db, id)?;
                let size = fs::metadata(blob(&root, &recording.metadata))
                    .map_err(storage)?
                    .len();
                if size != recording.metadata.audio_bytes {
                    return Err(Error::Storage("stored audio length mismatch".into()));
                }
                let search_pending = recording
                    .transcript_record_ids
                    .iter()
                    .any(|id| db.contains_key(keys::projection(*id)).unwrap_or(true));
                Ok(RecordingReceipt {
                    server_id: server_id(db)?,
                    recording,
                    search_pending,
                })
            })
            .await
    }

    pub async fn audio(&self, id: Uuid) -> Result<Vec<u8>> {
        let root = self.root.clone();
        self.repository
            .blocking(move |db| {
                let recording = load(db, id)?;
                let audio = fs::read(blob(&root, &recording.metadata)).map_err(storage)?;
                if recording_hash(&audio) != recording.metadata.sha256 {
                    return Err(Error::Storage("stored audio checksum mismatch".into()));
                }
                Ok(audio)
            })
            .await
    }

    pub async fn audio_path(&self, id: Uuid) -> Result<PathBuf> {
        let root = self.root.clone();
        self.repository
            .blocking(move |db| Ok(blob(&root, &load(db, id)?.metadata)))
            .await
    }

    pub async fn transcript(&self, id: Uuid) -> Result<RecordingTranscript> {
        self.repository
            .blocking(move |db| {
                db.flush().map_err(storage)?;
                load(db, id)?;
                db.get(transcript_key(id))
                    .map_err(storage)?
                    .map(|v| decode(&v))
                    .transpose()?
                    .ok_or(Error::Busy)
            })
            .await
    }

    pub async fn pending(&self, limit: usize) -> Result<Vec<StoredRecording>> {
        if !(1..=100).contains(&limit) {
            return Err(Error::Invalid("invalid recording queue limit".into()));
        }
        self.repository
            .blocking(move |db| {
                let mut pending = vec![];
                for row in db.scan_prefix(b"audio/recording/") {
                    let (_, bytes) = row.map_err(storage)?;
                    let recording: StoredRecording = decode(&bytes)?;
                    recording.metadata.validate()?;
                    if recording.transcribed_at.is_none() && recording.retry_at <= Utc::now() {
                        pending.push(recording);
                        if pending.len() == limit {
                            break;
                        }
                    }
                }
                Ok(pending)
            })
            .await
    }

    pub async fn begin_attempt(&self, id: Uuid) -> Result<()> {
        self.repository
            .blocking(move |db| {
                let mut recording = load(db, id)?;
                if recording.transcribed_at.is_some() {
                    return Ok(());
                }
                recording.transcription_attempts =
                    recording.transcription_attempts.saturating_add(1);
                recording.retry_at = Utc::now() + Duration::minutes(5);
                recording.transcription_error = Some("transcription_pending_or_retrying".into());
                db.insert(key(id), encode(&recording)?).map_err(storage)?;
                db.flush().map_err(storage)?;
                Ok(())
            })
            .await
    }

    pub async fn defer_attempt(&self, id: Uuid) -> Result<()> {
        self.repository
            .blocking(move |db| {
                let mut recording = load(db, id)?;
                if recording.transcribed_at.is_none() {
                    recording.retry_at = Utc::now() + Duration::minutes(5);
                    recording.transcription_error = Some("local_transcription_unavailable".into());
                    db.insert(key(id), encode(&recording)?).map_err(storage)?;
                }
                db.flush().map_err(storage)?;
                Ok(())
            })
            .await
    }

    pub async fn finish_transcript(
        &self,
        id: Uuid,
        text: String,
        model: String,
    ) -> Result<RecordingTranscript> {
        if text.len() > 2 * 1024 * 1024 || model.is_empty() || model.len() > 128 {
            return Err(Error::Invalid("transcript exceeds limits".into()));
        }
        self.repository
            .blocking(move |db| {
                let mut recording = load(db, id)?;
                if let Some(bytes) = db.get(transcript_key(id)).map_err(storage)? {
                    db.flush().map_err(storage)?;
                    return decode(&bytes);
                }
                let now = Utc::now();
                let transcript = RecordingTranscript {
                    recording_id: id,
                    text,
                    model,
                    completed_at: now,
                };
                let mut batch = sled::Batch::default();
                let mut remainder = transcript.text.trim();
                let mut index = 0u32;
                while !remainder.is_empty() {
                    let mut end = remainder.len().min(16_384);
                    while !remainder.is_char_boundary(end) {
                        end -= 1;
                    }
                    let mut digest = Sha256::new();
                    digest.update(id.as_bytes());
                    digest.update(index.to_be_bytes());
                    let hash = digest.finalize();
                    let mut bytes = [0u8; 16];
                    bytes.copy_from_slice(&hash[..16]);
                    bytes[6] = (bytes[6] & 15) | 0x50;
                    bytes[8] = (bytes[8] & 63) | 0x80;
                    let note_id = Uuid::from_bytes(bytes);
                    if db.contains_key(keys::record(note_id)).map_err(storage)? {
                        return Err(Error::RequestConflict);
                    }
                    let record = Record {
                        schema_version: SCHEMA_VERSION,
                        id: note_id,
                        revision: 1,
                        created_at: now,
                        updated_at: now,
                        deleted: false,
                        body: RecordBody::Note {
                            text: format!(
                                "Plaud recording {} [{}] part {}\n{}",
                                recording.metadata.recorded_at,
                                id,
                                index + 1,
                                &remainder[..end]
                            ),
                        },
                    };
                    record.validate()?;
                    batch.insert(keys::record(note_id), encode(&record)?);
                    batch.insert(
                        keys::projection(note_id),
                        encode(&ProjectionJob {
                            record_id: note_id,
                            revision: 1,
                        })?,
                    );
                    recording.transcript_record_ids.push(note_id);
                    remainder = &remainder[end..];
                    index += 1;
                }
                recording.transcribed_at = Some(now);
                recording.transcription_error = None;
                batch.insert(transcript_key(id), encode(&transcript)?);
                batch.insert(key(id), encode(&recording)?);
                db.apply_batch(batch).map_err(storage)?;
                db.flush().map_err(storage)?;
                Ok(transcript)
            })
            .await
    }
}

fn invalid_audio() -> Error {
    Error::Invalid("invalid or unsupported Ogg Opus recording".into())
}

fn validate_ogg(bytes: &[u8], duration_ms: u64) -> Result<()> {
    let mut at = 0;
    let mut sequence = 0u32;
    let mut serial = None;
    let mut frames = 0u64;
    let mut ended = false;
    while at < bytes.len() {
        if ended
            || bytes.len() - at < 28
            || &bytes[at..at + 4] != b"OggS"
            || bytes[at + 4] != 0
            || bytes[at + 26] != 1
        {
            return Err(invalid_audio());
        }
        let size = 28 + usize::from(bytes[at + 27]);
        if bytes.len() - at < size {
            return Err(invalid_audio());
        }
        let page = &bytes[at..at + size];
        let u32_at = |offset| -> Result<u32> {
            Ok(u32::from_le_bytes(
                page[offset..offset + 4]
                    .try_into()
                    .map_err(|_| invalid_audio())?,
            ))
        };
        if u32_at(18)? != sequence || serial.is_some_and(|s| u32_at(14).ok() != Some(s)) {
            return Err(invalid_audio());
        }
        serial = Some(u32_at(14)?);
        let mut crc = 0u32;
        for (index, byte) in page.iter().enumerate() {
            crc ^= u32::from(if (22..26).contains(&index) { 0 } else { *byte }) << 24;
            for _ in 0..8 {
                crc = (crc << 1)
                    ^ if crc & 0x8000_0000 != 0 {
                        0x04c1_1db7
                    } else {
                        0
                    };
            }
        }
        if crc != u32_at(22)? {
            return Err(invalid_audio());
        }
        let packet = &page[28..];
        let flags = page[5];
        if sequence == 0 {
            if flags != 2
                || packet.len() != 19
                || &packet[..8] != b"OpusHead"
                || packet[8] != 1
                || packet[9] != 1
                || packet[10..12] != [0, 0]
                || packet[18] != 0
            {
                return Err(invalid_audio());
            }
        } else if sequence == 1 {
            if flags != 0 || !packet.starts_with(b"OpusTags") {
                return Err(invalid_audio());
            }
        } else {
            if packet.is_empty() || packet.len() > 80 || (flags != 0 && flags != 4) {
                return Err(invalid_audio());
            }
            frames += 1;
            let granule = u64::from_le_bytes(page[6..14].try_into().map_err(|_| invalid_audio())?);
            if granule != frames * 960 {
                return Err(invalid_audio());
            }
            ended = flags == 4;
        }
        at += size;
        sequence += 1;
    }
    if !ended || frames == 0 || frames * 20 != duration_ms {
        return Err(invalid_audio());
    }
    Ok(())
}
