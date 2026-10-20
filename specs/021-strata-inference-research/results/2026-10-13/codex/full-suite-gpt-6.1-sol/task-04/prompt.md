Task 4 — Failing-test diagnosis

Add a focused regression assertion that the mixed-case reference
`hf.co/Qwen/model:Q4_K_M` routes locally, while `anthropic/model` routes to
OpenRouter. Run it before changing implementation; use the observed behavior
to decide whether a code fix is needed. Keep the test as a permanent case.

Acceptance commands:
- `nix develop --command cargo test -p agentix-router`

Work only in this checkout. Do not commit. Make the requested changes and stop.