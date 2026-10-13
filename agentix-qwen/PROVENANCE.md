# Implementation provenance

These Rust primitives are independently written using the mathematical behavior of the original Qwen model and its Hugging Face Transformers reference. No Strata source, translated kernels, runtime packs or scripts are included or required.

Reference: https://github.com/huggingface/transformers/blob/2fa1fdd5b27ae54bdcee0516848c7963393a5f5c/src/transformers/models/qwen4_exp/modeling_qwen4_exp.py
Configuration: https://huggingface.co/Qwen/Qwen3.8-Flash-Next/blob/de4b8e4d43b917e7706784d8bb445c9af86a3540/config.json

Transformers reference source is Apache-2.0, copyright the Qwen team and HuggingFace Inc. See its pinned source header and LICENSE for attribution. The Rust organization, storage abstraction, checked admission, error handling and tests are original agentix work. The SplitMix64 constants and n-gram arithmetic are required for checkpoint compatibility.

The CPU foundation uses `minijinja` 2.24.0 for the pinned chat template, `tokenizers` 0.22.2 for the tokenizer JSON, `serde_json` for template input, and `sha2` 0.10.9 for artifact identity. The native GGUF/GGML binding remains isolated in the separate `agentix-qwen-sys` crate, which pins `llama-cpp-sys-2` 0.1.154. The CPU crate still does not load the full model or generate text and does not advertise a usable inference backend. Future dependencies and adapted source must be recorded here before inclusion.

`tests/ngram.rs` fixes integer row IDs calculated separately with JavaScript
BigInt arithmetic from the pinned reference equations. It is not a run of the
Transformers model. `tests/reference.rs` uses analytical softmax, normalization,
and orthogonal-key recurrence cases, with absolute tolerance 1e-6. These tests
validate the CPU primitives only; accelerated and full-model comparisons remain
outstanding.
