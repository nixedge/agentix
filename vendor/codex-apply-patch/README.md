# Codex patch core extraction

Source: OpenAI Codex `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`
(`rust-v0.162.1`), `codex-rs/apply-patch/src/`. Apache-2.0; upstream LICENSE and NOTICE are retained.
The user approved this extraction on 2026-10-11. This is not a separately published upstream crate.

`parser.rs`, `streaming_parser.rs`, `seek_sequence.rs` and `text_file.rs` are copied unchanged, including their upstream tests.
`file_update.rs` retains the original `compute_replacements` implementation with crate visibility and four imports. Its asynchronous executor/filesystem wrappers and diff rendering are omitted.
`lib.rs` exposes the original argument shape and a pure `update_text` adapter. It does not read or write files, invoke shell commands, or start servers. Original parser path-URI utilities remain pinned Git dependencies.

Upstream source SHA-256 values:

| File | SHA-256 |
|---|---|
| parser.rs | 6b8086467d0500f4fc9aa9a35cd33a0bce53c01bcb74b915b9efc6fcf187f7ce |
| streaming_parser.rs | 5f4b8e60fd24ada7c1b6a696155de3da2b946f7b5436da90e377c9de04b1e578 |
| seek_sequence.rs | a6d0d35b628a4f1e6f8ba77c533046c27e585a8bdd94ee6cd21d2585b39e34e0 |
| text_file.rs | 35cbbdeb91e86780d99fccd097a5dba2772f048730556b73d685e94658ddc6e3 |
| file_update.rs | 9a2e4f4ccef98e34709bec522554729bfdfd188bb92b7db87ddc7bab87f866e6 |

The upstream matcher tolerates whitespace differences. Callers must verify the expected original content before committing writes; this library does not supply optimistic concurrency or path containment. The crate-level test-only panic-lint allowance preserves upstream fixture assertions, while production retains the workspace deny rules.

Validation: `nix develop --command cargo test -p agentix-codex-patch`.
