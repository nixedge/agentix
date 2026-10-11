use agentix_codex_patch::{Hunk, parse_patch, update_text};

#[test]
fn update_preserves_context_and_uses_upstream_final_newline()
-> Result<(), Box<dyn std::error::Error>> {
    let patch = parse_patch(
        "*** Begin Patch\n*** Update File: test\n@@\n first\n-old\n+new\n last\n*** End Patch",
    )?;
    let Hunk::UpdateFile { chunks, .. } = &patch.hunks[0] else {
        return Err("expected update".into());
    };
    assert_eq!(
        update_text("test", "first\r\nold\nlast", chunks)?,
        "first\r\nnew\r\nlast\r\n"
    );
    assert!(update_text("test", "first\r\nchanged\nlast", chunks).is_err());
    Ok(())
}
