use agentix_assistant_core::RecordingMetadata;
use agentix_assistant_sled::recording_hash;
use chrono::{TimeZone, Utc};
use uuid::Uuid;
pub fn fixture() -> Vec<u8> {
    fixture_frames(2)
}
pub fn fixture_frames(frames: usize) -> Vec<u8> {
    let head = [
        b"OpusHead".as_slice(),
        &[1, 1, 0, 0],
        &16000u32.to_le_bytes(),
        &[0, 0, 0],
    ]
    .concat();
    let tags = [
        b"OpusTags".as_slice(),
        &0u32.to_le_bytes(),
        &0u32.to_le_bytes(),
    ]
    .concat();
    let mut bytes = vec![];
    for index in 0..frames + 2 {
        let packet = match index {
            0 => head.as_slice(),
            1 => tags.as_slice(),
            _ => &[0xf8, 0xff, 0xfe],
        };
        let mut page = vec![0u8; 28];
        page[..4].copy_from_slice(b"OggS");
        page[5] = if index == 0 {
            2
        } else if index == frames + 1 {
            4
        } else {
            0
        };
        let granule = if index >= 2 {
            (index as u64 - 1) * 960
        } else {
            0
        };
        page[6..14].copy_from_slice(&granule.to_le_bytes());
        page[14..18].copy_from_slice(&123u32.to_le_bytes());
        page[18..22].copy_from_slice(&(index as u32).to_le_bytes());
        page[26] = 1;
        page[27] = packet.len() as u8;
        page.extend_from_slice(packet);
        let mut crc = 0u32;
        for b in &page {
            crc ^= u32::from(*b) << 24;
            for _ in 0..8 {
                crc = (crc << 1)
                    ^ if crc & 0x8000_0000 != 0 {
                        0x04c1_1db7
                    } else {
                        0
                    };
            }
        }
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        bytes.extend(page);
    }
    bytes
}
pub fn metadata(bytes: &[u8]) -> RecordingMetadata {
    RecordingMetadata {
        id: Uuid::new_v4(),
        source_device: "a".repeat(32),
        source_session: 1700000000,
        source_bytes: 160,
        recorded_at: Utc
            .timestamp_opt(1700000000, 0)
            .single()
            .unwrap_or_default(),
        duration_ms: 40,
        audio_bytes: bytes.len() as u64,
        sha256: recording_hash(bytes),
        media_type: "audio/ogg".into(),
    }
}
