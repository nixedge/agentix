# Verification

Worktree: `/home/sam/home/agentix/035-shared-gpu-memory`, based on `bc91122`.
No deployment, SSH or driver change was performed.
After these initial checks, the owner authorized bounded local GPU tests. See
[local-5090/summary.md](local-5090/summary.md): fixture and optimized production
release/refill pass, returning 480 MiB, with exact matched checkpoint replay.
Initial timeout and cross-placement comparison failures are preserved there.

## CPU checks

- Initial full workspace, before the two added ignored production hardware tests:
  **362 passed, 0 failed, 11 ignored** in
  [workspace-tests.log](workspace-tests.log). Hardware tests and the IPC child
  helper are intentionally ignored in the normal parent invocation.
- Strict all-target Clippy passes for `agentix-gpu`, `agentix-infer`,
  `agentix-qwen`, `agentix-qwen-worker`, `agentix-llama` and `agentix-whisper`:
  [clippy.log](clippy.log). This is a focused lint result, not workspace-wide
  Clippy acceptance.
- `cargo fmt --all --check` passes. Alejandra checks all six changed Nix files,
  including the reproducible module fixture. Vendored upstream style is retained.
- NixOS module evaluation has **no failed assertions** and confirms all three
  worker roles share coordinator configuration and systemd ordering:
  [module-eval.json](module-eval.json). Reproduce with
  `nix eval --impure --json --file specs/035-shared-gpu-memory/results/module-fixture.nix`.
  Its hello packages, tiny budget and synthetic hashes are evaluation fixtures.

Cargo used the project `workspace-tests` Nix shell, one compiler job, one CMake
job, `AGENTIX_GPU_LAYERS=0`, and two native/Rayon/OpenMP threads. The full run
initially stopped because that shell lacked bubblewrap, required by the existing
jail regression. Adding the missing shell dependency fixed the unchanged test;
the initial evidence is [workspace-tests-missing-bwrap.log](workspace-tests-missing-bwrap.log).

## Package builds

Canonical builds pass with `--cores 2 --max-jobs 1` for
`agentix-gpu-coordinator`, `agentix-qwen`, `agentix-qwen-worker`, `agentix-llama`,
`agentix-whisper`, `agentix-daemon` and `checks.x86_64-linux.workspace-locks`.
The final source includes the cache-release recovery retry. See
[nix-build.log](nix-build.log) and [nix-build-paths.txt](nix-build-paths.txt), in
the package order listed above. The llama build includes CUDA compilation;
building it does not exercise a GPU. Installed Qwen library and worker license
and modification notices match their source files byte for byte.
These outputs precede the subsequent local hardware test-only additions.
Updated Qwen library/worker paths are in
[local-5090/nix-build-paths.txt](local-5090/nix-build-paths.txt);
production runtime code is unchanged by those additions.

The initial packaging failure involved copying the real path-patched source over
Crane's read-only generated manifests/stubs. The existing copy mechanism now
makes its destination writable before restoring the real patch. Evidence is
[nix-build-initial.log](nix-build-initial.log).

Master's original handoff and the request snapshot have the same Git blob hash:
`3cef0c094504cac3c8ef2c117fe48633a00fefc3`. Master has no tracked changes.

## Remaining physical acceptance

Follow [quickstart.md](../quickstart.md) on kursk. Calibrate model-hash/context/
batch peak profiles, measure actual VRAM released before competing GPU loads,
and verify the full coordinated coexistence sequence. The local 5090 tests prove
release and live continuation at context 128; they do not calibrate kursk's
context 4096 or establish real three-worker acceptance. No throughput claim is made.

After integrating master feature 036, stock Whisper uses CUDA when CUDA
capabilities are configured. Its shared lease lifetime and native cleanup join
are combined; CPU transcription cannot satisfy GPU-Whisper acceptance.
Admission trusts calibrated peak upper bounds; it does not intercept every
native allocation or prevent racing unmanaged GPU work.
