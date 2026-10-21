# ai.lan chat retest and GGUF correctness investigation

Endpoint: `http://ai.lan.disasm.us:11434`. Local baseline HEAD: `49cbc5c`;
the gateway's deployed revision and compute mode were not reported by this probe.

`GET /v1/models` returned HTTP 200 in 0.135706 seconds and advertised
`qwen3.8-flash-next`.

Request:

```json
{"model":"qwen3.8-flash-next","messages":[{"role":"user","content":"Reply with OK."}],"temperature":0,"max_tokens":4}
```

Response: HTTP 200; 6.375715 seconds total, 6.375596 seconds to first byte.

```json
{"choices":[{"finish_reason":"length","index":0,"message":{"content":"6\n(@-Ch","role":"assistant"}}],"created":1792541939,"id":"chatcmpl-46e8fa788ec14ad79881183590f8db41","model":"qwen3.8-flash-next","object":"chat.completion","usage":{"completion_tokens":4,"prompt_tokens":16,"total_tokens":20}}
```

The earlier prompt-preparation error no longer occurs, but this output does not
satisfy the prompt. It is not evidence of usable model generation or performance.

Source inspection identified three mismatches between the decoder and the pinned
GGUF conversion contract (llama.cpp `6c84c7d5d8833c6e0df69628f75a0f599797934e`,
`conversion/qwen.py` and `conversion/qwen4exp.py`):

- Non-gated RMS norm vectors already contain the added one; the decoder added
  it again. This affects residual, final, PLE, QSA and indexer norms. The gated
  recurrent output norm is a direct scale in both representations.
- `ssm_a` contains `-exp(A_log)`; the decoder treated it as source `A_log` and
  exponentiated it again.
- Value-axis tensors use tiled heads, while Q/K retain source order. The decoder
  still associated each value head with a contiguous grouped key head.

The correction translates small norm vectors at load time, selects explicit
GGUF recurrence/decay semantics, and preserves encoded matrix storage. Tests
compare converted weights against source equations with two key heads, six value
heads, distinct parameters and three tokens, including row-backed prefill and
state. GGUF loader coverage and native CPU/CUDA compact decoder fixtures now
exercise unequal key/value-head counts.

The focused `qwen-check` and CUDA-enabled worker/device-check Nix builds pass.
The corrected worker is `/nix/store/qfgzkaqrix0ib3mirxn4ca5yh479n21g-agentix-qwen-worker`.
No corrected-worker deployment, device execution or post-deployment real-model
request has been performed. These source-level fixes do not yet prove that all
causes of incorrect full-model output have been resolved.

Investigation tooling: incremental ingestion stopped at 230/231 files with
HTTP 500 from `/api/embed`. The user explicitly approved direct local source
reads after that failure. No deployment, SSH access or push was performed.
