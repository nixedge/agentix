# Implementation provenance

These Rust primitives are independently written using the mathematical behavior of the original Qwen model and its Hugging Face Transformers reference. No Strata source, translated kernels, runtime packs or scripts are included or required.

Reference: https://github.com/huggingface/transformers/blob/2fa1fdd5b27ae54bdcee0516848c7963393a5f5c/src/transformers/models/qwen4_exp/modeling_qwen4_exp.py
Configuration: https://huggingface.co/Qwen/Qwen3.8-Flash-Next/blob/de4b8e4d43b917e7706784d8bb445c9af86a3540/config.json

Transformers reference source is Apache-2.0, copyright the Qwen team and HuggingFace Inc. See its pinned source header and LICENSE for attribution. The Rust organization, storage abstraction, checked admission, error handling and tests are original agentix work. The SplitMix64 constants and n-gram arithmetic are required for checkpoint compatibility.

No native or third-party Rust dependencies are used by the initial foundation library. It does not load the full model, generate text, or advertise a usable inference backend. Future dependencies and adapted source must be recorded here before inclusion.
