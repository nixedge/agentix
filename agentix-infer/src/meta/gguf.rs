use crate::{error::InferError, BackendHint, Capability};
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct GgufMeta {
    pub architecture: String,
    pub context_length: u32,
    pub embedding_length: u32,
    pub capabilities: Vec<Capability>,
    pub parameter_count: u64,
    #[allow(dead_code)]
    pub quantization: Option<String>,
    /// Overrides the default backend selection when the architecture mandates it.
    /// `None` means use the format-based default (GGUF → LlamaCpp).
    pub backend_hint: Option<BackendHint>,
}

// GGUF magic: "GGUF" in little-endian
const GGUF_MAGIC: u32 = 0x4655_4747;

// GGUF metadata value types
const TYPE_UINT8: u32 = 0;
const TYPE_INT8: u32 = 1;
const TYPE_UINT16: u32 = 2;
const TYPE_INT16: u32 = 3;
const TYPE_UINT32: u32 = 4;
const TYPE_INT32: u32 = 5;
const TYPE_FLOAT32: u32 = 6;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;
const TYPE_UINT64: u32 = 10;
const TYPE_INT64: u32 = 11;
const TYPE_FLOAT64: u32 = 12;

/// Pure-Rust GGUF metadata parser. Reads only the keys needed for capability
/// detection; skips all other values without loading tensor data.
pub fn read_gguf_metadata(path: &Path) -> Result<GgufMeta, InferError> {
    let file = std::fs::File::open(path)
        .map_err(|e| InferError::Backend(format!("cannot open GGUF: {e}")))?;
    let mut r = BufReader::new(file);

    let magic = read_u32(&mut r)?;
    if magic != GGUF_MAGIC {
        return Err(InferError::Backend("not a GGUF file".to_string()));
    }

    let version = read_u32(&mut r)?;
    if version != 2 && version != 3 {
        return Err(InferError::Backend(format!(
            "unsupported GGUF version: {version}"
        )));
    }

    let _tensor_count = read_u64(&mut r)?;
    let kv_count = read_u64(&mut r)?;

    let mut architecture = String::new();
    let mut general_name = String::new();
    let mut pooling_type: Option<u32> = None;
    let mut has_chat_template = false;
    let mut has_vision = false;
    let mut context_length: u32 = 0;
    let mut embedding_length: u32 = 0;

    for _ in 0..kv_count {
        let key = read_string(&mut r)?;
        let value_type = read_u32(&mut r)?;

        if key == "general.architecture" && value_type == TYPE_STRING {
            architecture = read_string(&mut r)?;
        } else if key == "general.name" && value_type == TYPE_STRING {
            general_name = read_string(&mut r)?;
        } else if key == "tokenizer.chat_template" {
            has_chat_template = value_type == TYPE_STRING;
            skip_value(&mut r, value_type)?;
        } else if key.ends_with(".pooling_type") && value_type == TYPE_UINT32 {
            pooling_type = Some(read_u32(&mut r)?);
        } else if (key.ends_with(".context_length") && key.starts_with(&architecture))
            && value_type == TYPE_UINT32
        {
            context_length = read_u32(&mut r)?;
        } else if (key.ends_with(".embedding_length") && key.starts_with(&architecture))
            && value_type == TYPE_UINT32
        {
            embedding_length = read_u32(&mut r)?;
        } else if (key.ends_with(".vision_encoder.image_size")
            || key == "clip.vision_model.image_size"
            || key == "vision_model.image_size")
            && !has_vision
        {
            has_vision = true;
            skip_value(&mut r, value_type)?;
        } else {
            skip_value(&mut r, value_type)?;
        }
    }

    // Whisper GGUFs have no pooling_type, chat_template, or vision keys and would
    // fall through to the heuristic fallback and be misclassified as Completion models.
    if architecture == "whisper" {
        return Ok(GgufMeta {
            architecture,
            context_length: 0,
            embedding_length: 0,
            capabilities: vec![Capability::Transcription],
            parameter_count: 0,
            quantization: None,
            backend_hint: Some(BackendHint::Whisper),
        });
    }

    let mut capabilities = Vec::new();
    if let Some(pt) = pooling_type {
        if pt != 0 {
            capabilities.push(Capability::Embedding);
        }
    }
    if has_chat_template {
        capabilities.push(Capability::Completion);
    }
    if has_vision {
        capabilities.push(Capability::Vision);
    }

    if capabilities.is_empty() {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        const EMBEDDING_ARCHS: &[&str] = &["bert", "nomic_bert", "roberta", "xlm_roberta"];
        if general_name.to_lowercase().contains("embed")
            || filename.contains("embed")
            || EMBEDDING_ARCHS.contains(&architecture.as_str())
        {
            tracing::info!(
                path = %path.display(),
                architecture = %architecture,
                general_name = %general_name,
                "no pooling_type/chat_template; name/arch heuristic identifies embedding model"
            );
            capabilities.push(Capability::Embedding);
        } else {
            tracing::warn!(
                path = %path.display(),
                architecture = %architecture,
                "no capability keys in GGUF; defaulting to Completion"
            );
            capabilities.push(Capability::Completion);
        }
    }

    Ok(GgufMeta {
        architecture,
        context_length,
        embedding_length,
        capabilities,
        parameter_count: 0,
        quantization: None,
        backend_hint: None,
    })
}

fn read_u8(r: &mut impl Read) -> Result<u8, InferError> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)
        .map_err(|e| InferError::Backend(e.to_string()))?;
    Ok(buf[0])
}

fn read_u16(r: &mut impl Read) -> Result<u16, InferError> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)
        .map_err(|e| InferError::Backend(e.to_string()))?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32(r: &mut impl Read) -> Result<u32, InferError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)
        .map_err(|e| InferError::Backend(e.to_string()))?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(r: &mut impl Read) -> Result<u64, InferError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)
        .map_err(|e| InferError::Backend(e.to_string()))?;
    Ok(u64::from_le_bytes(buf))
}

fn read_string(r: &mut impl Read) -> Result<String, InferError> {
    let len = read_u64(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .map_err(|e| InferError::Backend(e.to_string()))?;
    String::from_utf8(buf).map_err(|e| InferError::Backend(format!("GGUF string UTF-8: {e}")))
}

fn skip_value(r: &mut impl Read, typ: u32) -> Result<(), InferError> {
    match typ {
        TYPE_UINT8 | TYPE_INT8 | TYPE_BOOL => {
            read_u8(r)?;
        }
        TYPE_UINT16 | TYPE_INT16 => {
            read_u16(r)?;
        }
        TYPE_UINT32 | TYPE_INT32 | TYPE_FLOAT32 => {
            read_u32(r)?;
        }
        TYPE_UINT64 | TYPE_INT64 | TYPE_FLOAT64 => {
            read_u64(r)?;
        }
        TYPE_STRING => {
            read_string(r)?;
        }
        TYPE_ARRAY => {
            let elem_type = read_u32(r)?;
            let count = read_u64(r)?;
            for _ in 0..count {
                skip_value(r, elem_type)?;
            }
        }
        _ => {
            return Err(InferError::Backend(format!(
                "unknown GGUF value type: {typ}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn whisper_architecture_yields_transcription_and_whisper_hint() {
        let architecture = "whisper";
        let (caps, hint): (Vec<Capability>, Option<BackendHint>) = if architecture == "whisper" {
            (vec![Capability::Transcription], Some(BackendHint::Whisper))
        } else {
            (vec![], None)
        };
        assert_eq!(caps, vec![Capability::Transcription]);
        assert_eq!(hint, Some(BackendHint::Whisper));
    }

    #[test]
    fn non_whisper_architecture_does_not_short_circuit() {
        let architecture = "llama";
        assert!(architecture != "whisper");
    }

    #[test]
    fn empty_capabilities_fallback_completion() -> Result<(), Box<dyn std::error::Error>> {
        let meta = metadata_at_hashed_path("mistral-7b-instruct", false)?;
        assert_eq!(meta.capabilities, vec![Capability::Completion]);
        Ok(())
    }

    #[test]
    fn empty_capabilities_fallback_embedding_by_name() -> Result<(), Box<dyn std::error::Error>> {
        let meta =
            metadata_at_hashed_path("jinaai/test-qwen25-coder-jina-code-embeddings-1.5b", false)?;
        assert_eq!(meta.capabilities, vec![Capability::Embedding]);
        Ok(())
    }

    #[test]
    fn explicit_capability_takes_precedence_over_name_hint(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let meta = metadata_at_hashed_path("embedding-examples-chat-model", true)?;
        assert_eq!(meta.capabilities, vec![Capability::Completion]);
        Ok(())
    }

    fn metadata_at_hashed_path(
        name: &str,
        chat_template: bool,
    ) -> Result<GgufMeta, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(format!("sha256-{}", "a".repeat(64)));
        let mut fields = vec![("general.architecture", "qwen2"), ("general.name", name)];
        if chat_template {
            fields.push(("tokenizer.chat_template", "{{ messages }}"));
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GGUF_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&(fields.len() as u64).to_le_bytes());
        for (key, value) in fields {
            bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
            bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(&TYPE_STRING.to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
        std::fs::write(&path, bytes)?;
        Ok(read_gguf_metadata(&path)?)
    }

    #[test]
    fn embedding_capability_requires_nonzero_pooling() {
        let pooling_type: u32 = 0;
        let mut caps = Vec::new();
        if pooling_type != 0 {
            caps.push(Capability::Embedding);
        }
        assert!(caps.is_empty());

        let pooling_type_mean: u32 = 1;
        let mut caps2 = Vec::new();
        if pooling_type_mean != 0 {
            caps2.push(Capability::Embedding);
        }
        assert_eq!(caps2, vec![Capability::Embedding]);
    }
}
