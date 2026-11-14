# Findings

- Master bc91122 and the original standalone Whisper commit 719984e both declare an empty local CUDA feature. The build script's cargo:rustc-env does not configure the dependency build, and stock Nix applies CUDA only to llama.
- Pinned whisper-rs 0.14.4 Cargo.toml confirms `cuda = ["whisper-rs-sys/cuda", "_gpu"]`; `WhisperContextParameters::default` enables use_gpu only when `_gpu` is enabled. Source retrieved using agentic-nix for the exact Cargo.lock version.
- Existing configured Nix architectures are 86 and 120, using CUDA 12.9. Driver library remains host-provided at /run/opengl-driver/lib.
- CPU worker/startup checks must stay runnable in Nix build sandboxes without a host GPU driver. An explicit CPU package preserves that check and provides an operational CPU fallback.
- The tiny.en model fixture retains the SHA-256 previously pinned by commit 719984e (about 75 MB); no production Whisper model is needed for acceptance.
- Physical acceptance exposed native cudaFree/cuMemUnmap errors during process teardown because the native worker JoinHandle was discarded. Retaining it, explicitly dropping state before context, and scheduling its join on Tokio's blocking pool eliminates the reproduced error while keeping async tasks responsive. Compiler-derived Send/Sync suffices for the worker wrapper, so the old unsafe marker implementations were removed.
