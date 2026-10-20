Task 6 — Nix package/development-shell change

Add a `qwen-check` shell command to the project development shell that runs
formatting, Qwen crate tests, and Qwen Clippy with warnings denied. Keep the
existing `nix develop` interface and avoid adding global tools. Document the
command in the shell's existing help text.

Acceptance commands:
- `nix develop --command qwen-check`

Work only in this checkout. Do not commit. Make the requested changes and stop.