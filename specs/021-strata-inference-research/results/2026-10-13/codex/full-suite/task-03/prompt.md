Task 3 — Multi-file refactor preserving behavior

Refactor `agentix-qwen/src/budget.rs` so host and device reservation accounting
share one checked summation helper without changing the public API or error
semantics. Keep the helper private. Add or retain tests proving overflow and
independent host/device admission behavior.

Acceptance commands:
- `nix develop --command cargo test -p agentix-qwen --test storage`

Work only in this checkout. Do not commit. Make the requested changes and stop.