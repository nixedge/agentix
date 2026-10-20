Task 1 — Rust bug fix with regression test

In `agentix-harness/src/stagnation.rs`, make a zero-capacity
`StagnationDetector` safe: repeated pushes must not retain results or panic,
and the detector must never report stagnation when its history capacity is
zero. Add a unit test that checks the internal history remains empty after
repeated pushes. Preserve current behavior for positive capacities.

Acceptance commands:
- `nix develop --command cargo test -p agentix-harness`

Work only in this checkout. Do not commit. Make the requested changes and stop.