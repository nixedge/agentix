use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_RECORDING_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingMetadata {
    pub id: Uuid,
    pub source_device: String,
    pub source_session: u32,
    pub source_bytes: u64,
    pub recorded_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub audio_bytes: u64,
    pub sha256: String,
    pub media_type: String,
}

impl RecordingMetadata {
    pub fn validate(&self) -> Result<()> {
        let hex = |value: &str, length| {
            value.len() == length
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if self.id.is_nil()
            || !hex(&self.source_device, 32)
            || !hex(&self.sha256, 64)
            || self.source_session == 0
            || self.recorded_at.timestamp_millis() != i64::from(self.source_session) * 1000
            || self.source_bytes == 0
            || !self.source_bytes.is_multiple_of(80)
            || self.duration_ms != self.source_bytes / 80 * 20
            || self.duration_ms > 4 * 60 * 60 * 1000
            || !(1..=MAX_RECORDING_BYTES as u64).contains(&self.audio_bytes)
            || self.media_type != "audio/ogg"
        {
            return Err(Error::Invalid(
                "invalid recording metadata or size/profile limit".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredRecording {
    pub metadata: RecordingMetadata,
    pub stored_at: DateTime<Utc>,
    pub transcription_attempts: u32,
    pub retry_at: DateTime<Utc>,
    pub transcription_error: Option<String>,
    pub transcript_record_ids: Vec<Uuid>,
    pub transcribed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingReceipt {
    pub server_id: Uuid,
    pub recording: StoredRecording,
    pub search_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingTranscript {
    pub recording_id: Uuid,
    pub text: String,
    pub model: String,
    pub completed_at: DateTime<Utc>,
}
