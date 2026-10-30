# C++ runtime packaging fix

Kursk's Whisper service could not load `libstdc++.so.6`. Inspection of the exact reported binary confirmed it needed both `libstdc++.so.6` and `libgcc_s.so.1`, with an empty RPATH. Scoped build inputs already included the C++ runtime, but only ONNX closures enabled autoPatchelf.

Enable autoPatchelf on final member packages whose scoped inputs include the C++ runtime, including the CPU-only llama variant. Retain the existing hook for ONNX packages without adding it twice. Dependency artifact builds stay unchanged; the hook operates on installed binaries. Existing CUDA driver exclusions remain intact.

Verification:

- Canonical `agentix-whisper` build passed, reusing the existing dependency artifact build.
- autoPatchelf found both C++ runtime libraries and recorded their store paths in the installed binary's RPATH, with zero unresolved dependencies.
- `checks.whisper-runtime` passed: with `LD_LIBRARY_PATH` unset and no models configured, start the installed binary, query `/v1/models` over its Unix socket, request shutdown and verify a clean exit.
- `checks.package-binary-selection`, Alejandra and whitespace checks pass.

No deployment or model transcription test was performed. Other native package variants receive the same packaging hook but were not rebuilt in this verification run.
