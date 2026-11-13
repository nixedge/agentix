# Provenance

Uses RustingStrata by Vasyl Trefilov.

RustingStrata 1.2.1 is used as a Rust library, based on revision
`9dd6340d25c7c6770f7918134a27984cf20d7ce3` from
https://github.com/RustingStudio/RustingStrata.
Its RustingStrata License 1.0 is included in RUSTINGSTRATA-LICENSE.md.
Feature 035 applies the documented source extension in `patches/rusting-strata`:
leased expert-cache ceilings, synchronized buffer release/refill, and GPU graph
invalidation. See the installed AGENTIX-MODIFICATIONS.md for the changes.
Agentix wrapper code is Apache-2.0; no claim is made over upstream authorship.

Tokenizer, context admission and worker protocol/tool parsing are adapted from
Agentix feature 021 (`72b6786895f6cb9fde3e6bec847af85e38f67845`). No custom
inference, GGML FFI, loader or expert scheduler from that feature is used here.
