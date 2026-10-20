Task 5 — Clippy repair

Run Clippy for `agentix-harness`. Fix any warning introduced by the
`StagnationDetector` implementation or tests without weakening lint settings.
If the crate is already clean, make no source change and report that evidence.

Acceptance commands:
- `nix develop --command cargo clippy -p agentix-harness -- -D warnings`

Work only in this checkout. Do not commit. Make the requested changes and stop.