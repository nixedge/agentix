# Rendered chat prompt smoke attempt

Artifact: the locally verified IQ3_S shards at
`/home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/`.
Their expected SHA256 values are recorded in
`evidence/ista_iq3s_candidate_inventory.json`.

Command:

```sh
CARGO_TARGET_DIR=/tmp/agentix-021-bench-target nix develop --command env \
  AGENTIX_QWEN_CHAT_SMOKE=1 cargo run -p agentix-qwen-sys \
  --example validate_gguf_sample_rows -- \
  agentix-qwen/tests/fixtures/qwen3_8_flash_next_config.json \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00001.gguf \
  /home/sam/home/agentix/.cache/agentix-qwen/iq3s-ed59f92082b1e93c0e96d60a8b11aab089b52f09/iq3s-00002.gguf
```

The first attempt used `reasoning_effort = "none"`; it printed finite sampled
rows for the observed GGUF types and passed the row-backed operator smoke, then
prompt preparation rejected the unsupported reasoning effort before decoding.
The example now uses the supported `low` effort with thinking disabled.

The corrected attempt compiled and entered full-model CPU decoding of the
rendered `Reply with OK.` chat prompt. It remained at about 96% CPU and 503 MiB
RSS for more than 30 minutes without producing the example's final summary.
The process was interrupted with SIGINT (exit 130). This does not establish
rendered-prompt correctness or a successful full-model generation. See the
worker deployment instructions in `quickstart.md` for the target-host check.
