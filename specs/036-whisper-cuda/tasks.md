# Tasks

- [x] T001 Document scope, acceptance, master base, and implementation plan.
- [x] T002 Verify pinned whisper-rs CUDA feature and native build behavior.
- [x] T003 Fix Cargo forwarding, remove ineffective build script, and include Whisper in Nix CUDA package selection.
- [x] T004 Run CPU tests, Clippy, formatting, canonical Nix build, startup and lock checks; inspect CUDA linkage.
- [x] T005 Run a bounded physical tiny-fixture smoke when available; record deployment instructions and validation limits.
- [x] T006 Retain and join the worker thread after closing its channel, without blocking Tokio workers; explicitly order native cleanup.
- [x] T007 Add worker shutdown regression coverage and rerun CPU/Clippy/Nix and bounded CUDA acceptance, rejecting teardown errors.
