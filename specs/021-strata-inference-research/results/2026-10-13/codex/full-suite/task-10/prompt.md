Task 10 — Multi-step shell, test, and Git-diff task

Extend `agentix-qwen/tests/storage.rs` to cover both context-limit rejection
without reducing the prompt/output request and checked-add overflow at
`usize::MAX`. Run the focused test, inspect the final Git diff, and remove
unrelated edits before finishing.

Acceptance commands:
- `nix develop --command cargo test -p agentix-qwen --test storage`
- `git diff --check`

Work only in this checkout. Do not commit. Make the requested changes and stop.