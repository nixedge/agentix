Task 2 — Rust API addition across library and caller

Add `Router::route_with_default(&self, model: &str, default: RouteTarget)` in
`agentix-router` so an unrecognized model can use the caller's explicit
default while all recognized routing rules keep their current behavior. Update
one existing caller in the workspace to use this API for its configured
fallback, and cover both a recognized model and an unknown model in tests.

Acceptance commands:
- `nix develop --command cargo test -p agentix-router`
- `nix develop --command cargo check --workspace`

Work only in this checkout. Do not commit. Make the requested changes and stop.