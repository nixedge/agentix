Task 9 — Dependency boundary investigation with cited source evidence

Audit whether `agentix-qwen` can load or execute model tensors without pulling
in the native `agentix-qwen-sys` crate. Record the exact direct dependency tree
commands and their results in a short section of the Qwen quickstart, and
identify which crate currently owns GGUF parsing/dequantization.

Acceptance commands:
- `nix develop --command cargo tree -p agentix-qwen --edges normal`
- `nix develop --command cargo tree -p agentix-qwen-sys --edges normal`

Work only in this checkout. Do not commit. Make the requested changes and stop.