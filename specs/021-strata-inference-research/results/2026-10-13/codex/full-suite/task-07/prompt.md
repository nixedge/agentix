Task 7 — Configuration and documentation update

Update the Qwen quickstart to document that the text tokenizer has 248,077
addressable IDs while model embeddings are padded to 248,320, and explain that
these sizes must not be used interchangeably. Keep the values consistent with
the pinned configuration and tokenizer evidence.

Acceptance commands:
- `rg -n '248,077|248,320' specs/021-strata-inference-research/quickstart.md`

Work only in this checkout. Do not commit. Make the requested changes and stop.