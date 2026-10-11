use std::path::PathBuf;

use agentix_infer::{meta::gguf::read_gguf_metadata, Capability};
use sha2::{Digest, Sha256};

#[test]
fn pinned_generation_fixture_is_small_and_readable() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::var("AGENTIX_TEST_GENERATION_MODEL_PATH")?);
    let bytes = std::fs::read(&path)?;
    assert_eq!(bytes.len(), 19_077_344);
    assert_eq!(
        hex::encode(Sha256::digest(&bytes)),
        "6151b1929d7f5aa3385d9ddef3393e55587c0a55de661562322bc51dfda93a04"
    );
    let meta = read_gguf_metadata(&path)?;
    assert_eq!(meta.architecture, "llama");
    assert!(meta.context_length > 0);
    assert!(meta.capabilities.contains(&Capability::Completion));
    Ok(())
}
