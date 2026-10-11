# Deterministic protocol fixtures

`tool-turn.json` and `tool-result.json` form a complete tool-call/result pair with a stable invocation ID. They test protocol handling without assuming a tiny generation model can reliably call tools. Streaming tests should split the function arguments at every byte boundary and test truncated/duplicate IDs separately.

The independent generation fixture is Karpathy TinyStories 15M, quantized GGUF Q4_0, 19,077,344 bytes. Nix exposes it as `AGENTIX_TEST_GENERATION_MODEL_PATH`; it has no validated tool template and must never be marked coding-compatible just because generation succeeds. The existing embedding-only fixture remains `AGENTIX_TEST_MODEL_PATH`.
